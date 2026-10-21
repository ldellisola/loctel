//! Which spans of a trace `get_trace` shows, and how it pages through them.
//!
//! A trace can have 100,000 spans, and an agent should not be handed that by
//! default. In `summary` detail the spans come in this order:
//!
//! 1. the spans on the path from the root to the failed-in span (rank 0),
//! 2. every other span at depth 2 or less (rank 1),
//! 3. everything else (rank 2),
//!
//! each rank oldest start first. A page takes the next `limit` spans of that
//! order, but never runs on from the first two ranks into the third, so the
//! first page is the collapsed view (path and top levels) however small it is,
//! and the rest is one cursor away. In `full` detail there is one rank and the
//! order is plain start order.
//!
//! The cursor is a keyset over `(rank, start, span id)`, so a page boundary
//! holds still while spans arrive. A span that arrives later and sorts before
//! the boundary is not on a page already read: a live trace is read as of the
//! moment of each call.

use std::collections::HashMap;
use std::fmt;

use loctel_model::{Span, SpanId};
use loctel_store::TraceSnapshot;
use schemars::JsonSchema;
use serde::Deserialize;

/// How much of a trace `get_trace` returns.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Detail {
    /// The path to the failure and the top levels first (the default).
    #[default]
    Summary,
    /// Every span, in start order.
    Full,
}

impl Detail {
    fn marker(self) -> char {
        match self {
            Detail::Summary => 's',
            Detail::Full => 'f',
        }
    }

    fn name(self) -> &'static str {
        match self {
            Detail::Summary => "summary",
            Detail::Full => "full",
        }
    }
}

/// Spans at this depth or less (the root is 0) are in the collapsed view.
const TOP_DEPTH: u32 = 2;

/// One page of spans.
pub(crate) struct Page<'a> {
    /// In start order, whatever order they were picked in.
    pub spans: Vec<&'a Span>,
    /// How many spans come after this page.
    pub omitted: usize,
    pub next_cursor: Option<String>,
}

/// A cursor that cannot be used.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum CursorError {
    /// Not something `get_trace` produced.
    Invalid,
    /// Made for the other `detail`.
    OtherDetail(Detail),
}

impl fmt::Display for CursorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CursorError::Invalid => {
                f.write_str("invalid cursor: pass the next_cursor of the previous page, unchanged")
            }
            CursorError::OtherDetail(made_for) => write!(
                f,
                "this cursor belongs to detail={}: pass it with the same detail, or leave the cursor out",
                made_for.name()
            ),
        }
    }
}

/// The position of a span in the order: `(rank, start, span id)`.
type Key = (u8, u64, SpanId);

struct Ranked<'a> {
    rank: u8,
    span: &'a Span,
}

impl Ranked<'_> {
    fn key(&self) -> Key {
        (self.rank, self.span.start_unix_nano, self.span.span_id)
    }

    /// Pages never cross from the collapsed view (ranks 0 and 1) into the rest.
    fn group(&self) -> bool {
        self.rank >= 2
    }
}

/// Picks the page of `snapshot` that follows `cursor` (the first page when
/// `None`), at most `limit` spans.
pub(crate) fn select<'a>(
    snapshot: &'a TraceSnapshot,
    detail: Detail,
    limit: usize,
    cursor: Option<&str>,
) -> Result<Page<'a>, CursorError> {
    let limit = limit.max(1);
    let after = cursor.map(|text| decode_cursor(text, detail)).transpose()?;

    // `snapshot.spans` is in start order already, which is the whole order in
    // full detail.
    let mut ranked: Vec<Ranked<'a>> = match detail {
        Detail::Full => snapshot
            .spans
            .iter()
            .map(|span| Ranked { rank: 0, span })
            .collect(),
        Detail::Summary => {
            let mut ranked: Vec<Ranked<'a>> = ranks(snapshot)
                .into_iter()
                .zip(&snapshot.spans)
                .map(|(rank, span)| Ranked { rank, span })
                .collect();
            ranked.sort_by_key(Ranked::key);
            ranked
        }
    };
    // After `after`: the keys are unique (span IDs are), so this is a
    // boundary.
    let begin = after.map_or(0, |key| ranked.partition_point(|r| r.key() <= key));
    ranked.drain(..begin);

    let first_group = ranked.first().map(Ranked::group);
    let taken = ranked
        .iter()
        .take(limit)
        .take_while(|r| Some(r.group()) == first_group)
        .count();
    let omitted = ranked.len() - taken;
    let next_cursor = (omitted > 0)
        .then(|| ranked[taken - 1].key())
        .map(|key| encode_cursor(detail, key));

    let mut spans: Vec<&Span> = ranked[..taken].iter().map(|r| r.span).collect();
    spans.sort_by_key(|span| (span.start_unix_nano, span.span_id));
    Ok(Page {
        spans,
        omitted,
        next_cursor,
    })
}

/// The rank of each span of the snapshot, in `snapshot.spans` order.
fn ranks(snapshot: &TraceSnapshot) -> Vec<u8> {
    let spans = &snapshot.spans;
    let parents = parent_indexes(spans);
    let depths = depths(&parents);
    let mut ranks: Vec<u8> = depths
        .iter()
        .map(|&depth| if depth <= TOP_DEPTH { 1 } else { 2 })
        .collect();

    // The path: the failed-in span and its ancestors. At most one step per
    // span, so a parent cycle cannot loop.
    let start = snapshot
        .failed_in
        .as_ref()
        .and_then(|failed| spans.iter().position(|span| span.span_id == failed.span_id));
    let mut up = start;
    for _ in 0..spans.len() {
        let Some(i) = up else { break };
        ranks[i] = 0;
        up = parents[i];
    }
    ranks
}

/// For each span, the position of its parent in `spans`, if the trace has
/// the parent. A span is never its own parent here, and a duplicate span ID
/// means its first holder.
fn parent_indexes(spans: &[Span]) -> Vec<Option<usize>> {
    let mut position: HashMap<SpanId, usize> = HashMap::with_capacity(spans.len());
    for (i, span) in spans.iter().enumerate() {
        position.entry(span.span_id).or_insert(i);
    }
    spans
        .iter()
        .enumerate()
        .map(|(i, span)| {
            span.parent_span_id
                .and_then(|id| position.get(&id).copied())
                .filter(|&parent| parent != i)
        })
        .collect()
}

/// The depth of each span: 0 for a span with no parent in the trace (a root,
/// or an orphan whose parent has not arrived), one more than its parent's
/// otherwise. Walks iteratively, so a deep trace cannot overflow the stack,
/// and a parent cycle (malformed input) ends with its first span read as a
/// root.
fn depths(parents: &[Option<usize>]) -> Vec<u32> {
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum State {
        Unvisited,
        Walking,
        Done,
    }
    let mut state = vec![State::Unvisited; parents.len()];
    let mut depth = vec![0u32; parents.len()];
    let mut chain: Vec<usize> = Vec::new();
    for start in 0..parents.len() {
        if state[start] == State::Done {
            continue;
        }
        // Climb until something already known, a root or a cycle.
        let mut current = Some(start);
        let mut next_depth = 0;
        while let Some(i) = current {
            match state[i] {
                State::Done => {
                    next_depth = depth[i] + 1;
                    break;
                }
                // Back on the way up: a cycle. Its top reads as a root.
                State::Walking => break,
                State::Unvisited => {
                    state[i] = State::Walking;
                    chain.push(i);
                    current = parents[i];
                }
            }
        }
        // The last one climbed to is the highest: number down from it.
        while let Some(i) = chain.pop() {
            depth[i] = next_depth;
            state[i] = State::Done;
            next_depth += 1;
        }
    }
    depth
}

fn encode_cursor(detail: Detail, (rank, start, span): Key) -> String {
    format!("{}{rank}-{start}-{span}", detail.marker())
}

fn decode_cursor(text: &str, detail: Detail) -> Result<Key, CursorError> {
    let mut chars = text.chars();
    let made_for = match chars.next() {
        Some('s') => Detail::Summary,
        Some('f') => Detail::Full,
        _ => return Err(CursorError::Invalid),
    };
    // Ranks are 0, 1 and 2: the digits of radix 3.
    let rank = chars
        .next()
        .and_then(|c| c.to_digit(3))
        .and_then(|digit| u8::try_from(digit).ok())
        .ok_or(CursorError::Invalid)?;
    let rest = chars
        .as_str()
        .strip_prefix('-')
        .ok_or(CursorError::Invalid)?;
    let (start, span) = rest.split_once('-').ok_or(CursorError::Invalid)?;
    // `parse` alone would also take a leading `+`.
    if start.is_empty() || !start.bytes().all(|b| b.is_ascii_digit()) {
        return Err(CursorError::Invalid);
    }
    let start = start.parse().map_err(|_| CursorError::Invalid)?;
    let span = SpanId::from_hex(span).map_err(|_| CursorError::Invalid)?;
    if made_for != detail {
        return Err(CursorError::OtherDetail(made_for));
    }
    if made_for == Detail::Full && rank != 0 {
        return Err(CursorError::Invalid);
    }
    Ok((rank, start, span))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn depth_counts_parents_within_the_trace() {
        // 0 <- 1 <- 2, 0 <- 3, and 4 whose parent is not in the trace.
        let parents = [None, Some(0), Some(1), Some(0), None];
        assert_eq!(depths(&parents), [0, 1, 2, 1, 0]);
    }

    #[test]
    fn depth_does_not_depend_on_the_order_of_the_spans() {
        // A child listed before its parent (clock skew sorts that way).
        let parents = [Some(1), Some(2), None];
        assert_eq!(depths(&parents), [2, 1, 0]);
    }

    #[test]
    fn a_parent_cycle_terminates_with_a_root() {
        // 0 -> 1 -> 0, and 2 hangs from 1.
        let parents = [Some(1), Some(0), Some(1)];
        let depth = depths(&parents);
        assert_eq!(depth.len(), 3);
        assert!(depth.contains(&0), "{depth:?}");
        assert!(depth.iter().all(|&d| d < 3), "{depth:?}");
    }

    #[test]
    fn a_very_deep_chain_does_not_overflow_the_stack() {
        let n = 200_000;
        let parents: Vec<Option<usize>> = (0..n)
            .map(|i| if i == 0 { None } else { Some(i - 1) })
            .collect();
        let depth = depths(&parents);
        assert_eq!(depth[n - 1], (n - 1) as u32);
    }

    #[test]
    fn cursors_round_trip() {
        let key = (
            1,
            1_800_000_000_000_000_000,
            SpanId([0, 0, 0, 0, 0, 0, 1, 2]),
        );
        for detail in [Detail::Summary, Detail::Full] {
            let rank_ok = if detail == Detail::Full { 0 } else { key.0 };
            let key = (rank_ok, key.1, key.2);
            let text = encode_cursor(detail, key);
            assert_eq!(decode_cursor(&text, detail), Ok(key), "{text}");
        }
        assert_eq!(
            encode_cursor(Detail::Summary, key),
            "s1-1800000000000000000-0000000000000102"
        );
    }

    #[test]
    fn a_cursor_for_the_other_detail_or_made_up_is_refused() {
        let text = encode_cursor(Detail::Summary, (0, 5, SpanId([1; 8])));
        assert_eq!(
            decode_cursor(&text, Detail::Full),
            Err(CursorError::OtherDetail(Detail::Summary))
        );
        for bad in [
            "",
            "s",
            "s0",
            "s0-",
            "s0-5",
            "s0-5-",
            "s0-5-xyz",
            "s0--0101010101010101",
            "s0-+5-0101010101010101",
            "s9-5-0101010101010101",
            "x0-5-0101010101010101",
            "s0-99999999999999999999-0101010101010101",
            "f1-5-0101010101010101",
            "garbage",
        ] {
            assert!(
                decode_cursor(bad, Detail::Summary).is_err(),
                "{bad:?} was accepted"
            );
        }
    }
}
