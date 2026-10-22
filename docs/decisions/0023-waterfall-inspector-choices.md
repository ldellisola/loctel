# 0023: Waterfall and inspector model choices

Date: 2026-10-22
Status: accepted

## Context

Milestone 3 Task 3 fixes the interfaces of `loctel_ui::model::waterfall`
(`WaterfallRow`, `Waterfall`, `initial_selection`) and `model::inspector`
(`SpanDetails`, `span_details`), the order of the tree, the placeholder for
spans whose parent has not arrived and the labels of the ruler. It leaves open
what a placeholder row is made of, what a peer note says for spans that are not
database or HTTP calls, what happens to a selection a rebuild takes away, what
find searches and what a bar does at the edge of the trace. These are choices a
later agent might be tempted to "fix", so they are written down with the reason.

## Decision

- **The placeholder row is a row like the others, with an ID no span has.** Its
  `span_id` is all zeros, which OTLP uses for "no span" and ingest drops, so it
  cannot collide with a real span; `WaterfallRow::is_placeholder()` tells it
  apart (not the text, which an attribute could spell). It has the name and the
  `peer_note` `missing parent` (the plan names the row and its note with the
  same words; the view says it once), no service, colour 0, no bar (`0.0`,
  `0.0`) and an empty duration. It folds like any row, and with the errors
  filter on it is shown only if an orphan's subtree has an error.
- **Every span with no parent is a root.** The plan has one root, but the store
  keeps the extra ones too, and a span that is its own parent counts as one. All
  of them are at depth 0 in start order, so the root the store names (the
  earliest) comes first, and the placeholder comes after the last of their
  subtrees. A span that no root and no missing parent leads to (a parent loop,
  which no exporter produces) goes under the placeholder as well, after the
  orphans, so that every span of the trace has exactly one row and the walk ends.
- **The peer note follows the service map.** A client span's note is its
  database (`db.system.name`, else `db.system`), else its broker
  (`messaging.system`), else the host it calls (`server.address`, else
  `peer.service`); a producer's is its broker, else the host; a consumer's is
  its broker; everything else has none. So `SELECT carts` reads `postgresql`,
  `MGET` `redis`, `POST` to the gateway `card-gateway`, and the publish and
  process spans `rabbitmq`. A server span has none even when it has a
  `server.address` of its own.
- **A bar stays on its track.** `width_frac` is at least `MIN_WIDTH_FRAC`
  (0.003), and `start_frac` is moved left when it would push the bar past the
  end, so `start_frac + width_frac <= 1`. Without that a span with no length at
  the very end of the trace would be a bar drawn outside the track, which is no
  bar. A trace with no length at all (every span at one instant) draws slivers
  at the start and has a ruler of five `0`s. Exception markers are clamped to 0
  to 1 the same way, so a skewed clock cannot put one off the bar.
- **`color` is the unreduced index**, as in `TraceRow` and `PlacedNode` (decision
  0021). Services are asked of `ServiceColors` in start order, the order of
  `TraceSummary::services`, so a service has one number in the table and the
  waterfall whichever was drawn first.
- **A rebuild keeps what is still there.** The folded set, the selection and
  the errors filter are kept by span ID. A span that has gone from the trace is
  forgotten: it is no longer folded, and it is no longer selected (`selected()`
  gives `None` rather than an ID the inspector cannot show). The set is a state,
  not a rule: `collapse_all` folds the spans that have children now, and a span
  that arrives later is open. A selected span that a fold or the errors filter
  hides stays selected, and `row_of` says it has no row.
- **`select` of a span that is not in the trace does nothing.** It does not clear
  the selection either. It opens the folded spans above the one selected and
  leaves the errors filter alone, so with the filter on a span off the path can
  be selected and still have no row.
- **The errors filter and folding combine.** A folded span hides its errors, and
  a span with an error below it is on the path whether or not the span itself is
  erroring. A trace with no error shows no row with the filter on.
- **Find looks at values only.** Name, service and the value of every attribute,
  a number or boolean as the inspector writes it, a list or map as JSON. Keys,
  the status message, events (the exception) and the resource are not searched:
  the plan says "any attribute value", and a search for `timeout` that matched
  every span whose status message says so would hide the one that has the
  attribute. Only the rows shown are matched, and no text matches nothing. The
  text each row is matched against is built when the tree is, in lower case, so
  a find is a scan of strings without allocating; the cost is a copy of the
  values in memory, as much as the attributes of the trace itself.
- **A depth stops at 65,535**, the largest the `u16` of the interface holds. The
  walk is iterative and does not depend on depth otherwise; a test builds a chain
  of 70,000.
- **The details say what the spans do not.** `kind` is the word (`Server`,
  `Client`, ...), `status` is `Unset`, `OK` or `Error`, followed by ` · ` and the
  message when the span has one (`Error · HTTP 500`, since the interface has no
  field for the message and the inspector shows both together), and the offset
  of a span that starts with the trace reads `starts at +0 ms` (a zero has no
  natural unit, and `+0 µs` looks like an error on every root). Attributes and
  resource are sorted by key with a stable sort and values are whole; the view
  cuts them.
- **`SpanDetails` and `span_details` are in `model::inspector`**, beside the
  waterfall's file rather than in it; `initial_selection` is in
  `model::waterfall`, with the type it selects in.
- **The budget has margin.** Building 100,000 spans takes about 0.3 s and
  rebuilding the same waterfall about 0.5 s on this container in a debug test
  build (the rebuild also frees the old tree), against the 1.5 s the plan allows.

## Consequences

- Each rule is pinned by a test in `tests/model_waterfall.rs` that names it,
  including the shapes the mesh does not make (two roots, a loop, a span that is
  its own parent, a chain deeper than a `u16`, a trace with no length).
- The trace view (Task 8) draws what `rows()` returns, must not draw the
  placeholder's note twice, and asks `is_placeholder()` before it offers a row to
  `span_details` (which answers `None` for it) or to the copy buttons.
- Memory for a very large live trace is about twice its attributes while it is
  open. If that ever matters, the search text can be built on the first find
  instead of in `build`, with no change to the interface.
