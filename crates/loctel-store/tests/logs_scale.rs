//! Log chunks, and counts, offsets and the histogram from their metadata
//! (Milestone 5, Task 3).
//!
//! The existing log tests (`logs.rs`, `logs_m4.rs`, `removal.rs`, ...) already
//! run in both modes through `modes::each_mode`, and the spilling mode seals a
//! chunk every 7 logs, so they hold the old behaviour of the log API over
//! sealed chunks. These tests are about what is new: when a chunk seals, how
//! chunks that overlap in time (late logs) read as one list, and the three
//! calls that answer from the chunks' metadata (`log_count`, `logs_at`,
//! `log_histogram`) giving what a scan gives.
//!
//! Every test builds its stores itself, through [`envs`]: one in memory and one
//! that spills, with the same chunk size, fed the same data.

mod fixtures;
mod modes;

use std::fs::{self, OpenOptions};
use std::os::unix::fs::FileExt;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use fixtures::*;
use loctel_model::{AttrValue, LogRecord, Resource, Severity};
use loctel_store::{
    HistogramBucket, LevelSet, LogFilter, LogId, ManualClock, PageRequest, Store, StoreConfig,
};
use tempfile::TempDir;

const SEC: u64 = 1_000_000_000;
const MS: u64 = 1_000_000;
/// A whole second, so that windows built from it line up with the per-second
/// counts of the chunks.
const BASE: u64 = 1_700_000_000 * SEC;

const SERVICES: [&str; 5] = [
    "web-bff",
    "orders-api",
    "payments-api",
    "inventory",
    "notifier",
];

const SEVERITIES: [Severity; 7] = [
    Severity::Unspecified,
    Severity::Trace,
    Severity::Debug,
    Severity::Info,
    Severity::Warn,
    Severity::Error,
    Severity::Fatal,
];

// ---- builders --------------------------------------------------------------------

/// A small deterministic generator (xorshift64*), so that the data of a test is
/// the same on every run and no crate is needed for it.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Rng {
        Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }

    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// A number in `0..n` (`n` above 0).
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

fn resource(service: &str) -> Arc<Resource> {
    Arc::new(Resource {
        attributes: vec![(
            "service.name".to_string(),
            AttrValue::Str(service.to_string()),
        )],
    })
}

fn record(
    resource: &Arc<Resource>,
    body: String,
    time_unix_nano: u64,
    severity: Severity,
) -> LogRecord {
    LogRecord {
        time_unix_nano,
        observed_unix_nano: time_unix_nano,
        severity,
        severity_text: severity.level_label().to_uppercase(),
        body,
        attributes: vec![],
        trace_id: None,
        span_id: None,
        resource: Arc::clone(resource),
        scope_name: "test".to_string(),
    }
}

/// `n` logs, numbered in the order they arrive, from the five services at all
/// seven levels. Log `i` is at `BASE + jitter + i * step - r` where `r` is a
/// random number up to `jitter`, so with a jitter the logs arrive out of order
/// and chunks overlap in time; with none, time only goes forward and chunks do
/// not overlap. Bodies are all different, so nothing is a retry of another.
fn generate(n: usize, seed: u64, step: u64, jitter: u64) -> Vec<LogRecord> {
    let resources: Vec<Arc<Resource>> = SERVICES.iter().map(|name| resource(name)).collect();
    let mut rng = Rng::new(seed);
    (0..n)
        .map(|i| {
            let service = rng.below(SERVICES.len() as u64) as usize;
            let severity = SEVERITIES[rng.below(SEVERITIES.len() as u64) as usize];
            let time = BASE + jitter + i as u64 * step - rng.below(jitter + 1);
            record(&resources[service], format!("line {i}"), time, severity)
        })
        .collect()
}

fn ingest(store: &Store, logs: &[LogRecord]) {
    for batch in logs.chunks(1_000) {
        store.ingest_logs(batch.to_vec());
    }
}

/// A store with the same logs in the default config's way and in the
/// spilling one.
struct Env {
    name: &'static str,
    store: Store,
    clock: Arc<ManualClock>,
    spilling: bool,
    dir: TempDir,
}

/// Two stores: one in memory and one that spills (its segment files in the
/// directory of the [`Env`]). `configure` changes each starting config, so
/// both get the same chunk size and limits.
fn envs(configure: impl Fn(StoreConfig) -> StoreConfig) -> Vec<Env> {
    [("memory", false), ("spilling", true)]
        .into_iter()
        .map(|(name, spilling)| {
            let dir = TempDir::new().unwrap();
            let base = if spilling {
                StoreConfig::spilling_for_tests(dir.path().to_path_buf())
            } else {
                StoreConfig::default()
            };
            let (store, clock) = store_with_config(configure(base));
            Env {
                name,
                store,
                clock,
                spilling,
                dir,
            }
        })
        .collect()
}

/// The chunk settings of the plan, whatever the starting config says.
fn plan_chunks(config: StoreConfig) -> StoreConfig {
    StoreConfig {
        log_chunk_max: 65_536,
        log_chunk_age: Duration::from_secs(10),
        ..config
    }
}

fn sealed(store: &Store) -> Vec<usize> {
    store.log_chunks().sealed
}

fn page(limit: usize, cursor: Option<String>) -> PageRequest {
    PageRequest { limit, cursor }
}

/// Every log matching `f`, newest first, read a page at a time: what a keyed
/// walk of the Logs list sees.
fn walk(store: &Store, f: &LogFilter, page_size: usize) -> Vec<(LogId, LogRecord)> {
    let mut all = Vec::new();
    let mut cursor = None;
    loop {
        let page = store.search_logs_keyed(f, &page(page_size, cursor.take()));
        all.extend(page.items);
        match page.next_cursor {
            Some(next) => cursor = Some(next),
            None => return all,
        }
    }
}

fn bodies(items: &[(LogId, LogRecord)]) -> Vec<&str> {
    items.iter().map(|(_, log)| log.body.as_str()).collect()
}

fn log_time(log: &LogRecord) -> u64 {
    if log.time_unix_nano != 0 {
        log.time_unix_nano
    } else {
        log.observed_unix_nano
    }
}

/// Whether `log` passes the conditions an indexed filter can have, judged
/// straight from the log: the oracle the store's metadata has to agree with.
fn indexed_match(f: &LogFilter, log: &LogRecord) -> bool {
    let time = log_time(log);
    f.since_unix_nano.is_none_or(|since| time >= since)
        && f.until_unix_nano.is_none_or(|until| time < until)
        && f.levels.is_none_or(|levels| levels.contains(log.severity))
        && f.service
            .as_deref()
            .is_none_or(|service| log.service() == service)
}

fn levels_of(mask: u64) -> LevelSet {
    SEVERITIES
        .iter()
        .enumerate()
        .filter(|(index, _)| mask & (1 << index) != 0)
        .fold(LevelSet::default(), |set, (_, severity)| {
            set.with(*severity)
        })
}

/// An indexed filter with a random window (it may start and end outside the
/// data, and may be backwards), levels and service.
fn random_filter(rng: &mut Rng, from: u64, to: u64) -> LogFilter {
    let time = |rng: &mut Rng| from - 2 * SEC + rng.below(to - from + 4 * SEC);
    LogFilter {
        since_unix_nano: (rng.below(3) != 0).then(|| time(rng)),
        until_unix_nano: (rng.below(3) != 0).then(|| time(rng)),
        levels: (rng.below(2) == 0).then(|| levels_of(rng.below(128))),
        service: match rng.below(8) {
            0..=3 => None,
            4 => Some("ghost".to_string()),
            n => Some(SERVICES[(n - 5) as usize].to_string()),
        },
        ..LogFilter::default()
    }
}

// ---- configuration ----------------------------------------------------------------

#[test]
fn chunk_defaults_match_the_plan() {
    let config = StoreConfig::default();
    assert_eq!(config.log_chunk_max, 65_536);
    assert_eq!(config.log_chunk_age, Duration::from_secs(10));
    // The test config seals small chunks, so every log test of the crate meets
    // sealed chunks, and nothing else about it changed.
    let spilling = StoreConfig::spilling_for_tests("/some/dir".into());
    assert_eq!(spilling.log_chunk_max, 7);
    assert_eq!(spilling.log_chunk_age, Duration::from_secs(10));
}

// ---- when a chunk seals -------------------------------------------------------------

#[test]
fn chunk_seals_at_count_and_time() {
    for env in envs(plan_chunks) {
        let name = env.name;
        let store = &env.store;
        let logs = generate(65_536 + 6, 1, MS, 0);

        // One short of the chunk size: all in the open chunk.
        ingest(store, &logs[..65_535]);
        let info = store.log_chunks();
        assert_eq!((info.open, info.sealed.len()), (65_535, 0), "{name}");

        // The log that makes it 65,536 seals it, as part of the same call.
        ingest(store, &logs[65_535..65_536]);
        let info = store.log_chunks();
        assert_eq!(
            (info.open, info.sealed.clone()),
            (0, vec![65_536]),
            "{name}"
        );
        assert_eq!(info.on_disk, usize::from(env.spilling), "{name}");
        assert_eq!(store.stats().logs, 65_536, "{name}");

        // A few more stay open, however many calls it takes.
        ingest(store, &logs[65_536..65_539]);
        assert_eq!(store.log_chunks().open, 3, "{name}");

        // Ten seconds on the store's clock from its first log seal it: not a
        // nanosecond early. Ingest looks first, so the log that comes at 10 s
        // starts a chunk of its own.
        env.clock.advance(Duration::from_nanos(10 * SEC - 1));
        ingest(store, &logs[65_539..65_540]);
        let info = store.log_chunks();
        assert_eq!(
            (info.open, info.sealed.clone()),
            (4, vec![65_536]),
            "{name}: 9.999999999 s"
        );
        env.clock.advance(Duration::from_nanos(1));
        ingest(store, &logs[65_540..65_541]);
        let info = store.log_chunks();
        assert_eq!(
            (info.open, info.sealed.clone()),
            (1, vec![65_536, 4]),
            "{name}: 10 s"
        );
        assert_eq!(info.on_disk, 2 * usize::from(env.spilling), "{name}");

        // A store nobody writes to seals too: the step that notices runs ahead
        // of a read, at most once a second.
        env.clock.advance(Duration::from_secs(10));
        store.stats();
        let info = store.log_chunks();
        assert_eq!(
            (info.open, info.sealed),
            (0, vec![65_536, 4, 1]),
            "{name}: sealed by a read"
        );
        ingest(store, &logs[65_541..65_542]);

        // Nothing is lost in sealing: every log is there, in order.
        assert_eq!(store.stats().logs, 65_542, "{name}");
        let all = walk(store, &LogFilter::default(), 10_000);
        assert_eq!(all.len(), 65_542, "{name}");
        assert!(all.windows(2).all(|pair| pair[0].0 > pair[1].0), "{name}");
        assert_eq!(all[0].1.body, "line 65541", "{name}");
        assert_eq!(all[65_541].1.body, "line 0", "{name}");
    }
}

#[test]
fn an_empty_open_chunk_does_not_seal_and_the_age_counts_from_its_first_log() {
    for env in envs(plan_chunks) {
        let name = env.name;
        env.clock.advance(Duration::from_secs(3_600));
        env.store.stats();
        assert!(sealed(&env.store).is_empty(), "{name}: nothing to seal");

        // The age is the time since the chunk's first log, not since the store
        // began or since its last log.
        env.store.ingest_logs(vec![log("a", "first", 1)]);
        env.clock.advance(Duration::from_secs(6));
        env.store.ingest_logs(vec![log("a", "second", 2)]);
        env.clock.advance(Duration::from_secs(3));
        env.store.stats();
        assert!(sealed(&env.store).is_empty(), "{name}: 9 s old");
        env.clock.advance(Duration::from_secs(1));
        env.store.stats();
        assert_eq!(sealed(&env.store), vec![2], "{name}: 10 s old");
    }
}

// ---- late logs ------------------------------------------------------------------------

#[test]
fn late_log_lands_in_time_order_across_chunks() {
    for env in envs(|config| StoreConfig {
        log_chunk_max: 4,
        ..config
    }) {
        let name = env.name;
        let store = &env.store;
        // Twelve logs, 10 ms apart: three sealed chunks, 10..40, 50..80 and
        // 90..120 ms.
        let on_time: Vec<LogRecord> = (1..=12)
            .map(|i| log("orders-api", &format!("l{i}"), i * 10))
            .collect();
        store.ingest_logs(on_time);
        assert_eq!(sealed(store), vec![4, 4, 4], "{name}");

        // A log from a slow exporter, in the middle of the second chunk's time.
        store.ingest_logs(vec![log("orders-api", "late", 55)]);
        let info = store.log_chunks();
        assert_eq!((info.open, info.sealed.len()), (1, 3), "{name}");

        let expected = [
            "l12", "l11", "l10", "l9", "l8", "l7", "l6", "late", "l5", "l4", "l3", "l2", "l1",
        ];
        let all = walk(store, &LogFilter::default(), 100);
        assert_eq!(bodies(&all), expected, "{name}: time order");
        // The same through pages of 3: no log twice, none missed, whichever
        // chunk the page boundary falls in.
        let paged = walk(store, &LogFilter::default(), 3);
        assert_eq!(bodies(&paged), expected, "{name}: paged");

        // Its ID reads it, and the lines around it come from three chunks.
        let late_id = all[7].0;
        assert_eq!(late_id.time_unix_nano, 55 * MS, "{name}");
        assert_eq!(store.log(late_id).unwrap().body, "late", "{name}");
        let around = store.logs_around(late_id, 2, 2);
        let lines: Vec<&str> = around.lines.iter().map(|(_, l)| l.body.as_str()).collect();
        assert_eq!(lines, ["l4", "l5", "late", "l6", "l7"], "{name}");
        assert_eq!(around.target, Some(2), "{name}");

        // Counts, offsets and windows see it too.
        let none = LogFilter::default();
        assert_eq!(store.log_count(&none), Some(13), "{name}");
        assert_eq!(
            bodies(&store.logs_at(&none, 6, 3)),
            ["l6", "late", "l5"],
            "{name}"
        );
        let window = LogFilter {
            since_unix_nano: Some(55 * MS),
            until_unix_nano: Some(65 * MS),
            ..LogFilter::default()
        };
        assert_eq!(store.log_count(&window), Some(2), "{name}: late and l6");
        assert_eq!(
            bodies(&store.logs_at(&window, 0, 5)),
            ["l6", "late"],
            "{name}"
        );

        // Three more late logs fill the open chunk and seal it: a fourth chunk
        // that overlaps all three.
        store.ingest_logs(vec![
            log("orders-api", "later", 105),
            log("orders-api", "early", 15),
            log("orders-api", "mid", 75),
        ]);
        assert_eq!(sealed(store), vec![4, 4, 4, 4], "{name}");
        assert_eq!(store.stats().logs, 16, "{name}");
        let expected = [
            "l12", "l11", "later", "l10", "l9", "l8", "mid", "l7", "l6", "late", "l5", "l4", "l3",
            "l2", "early", "l1",
        ];
        for page_size in [1, 3, 5, 100] {
            let paged = walk(store, &LogFilter::default(), page_size);
            assert_eq!(bodies(&paged), expected, "{name}: pages of {page_size}");
        }
        for (offset, want) in expected.iter().enumerate() {
            let at = store.logs_at(&none, offset as u64, 1);
            assert_eq!(bodies(&at), [*want], "{name}: offset {offset}");
        }
        assert!(store.logs_at(&none, 16, 5).is_empty(), "{name}");
        assert_eq!(store.log_count(&none), Some(16), "{name}");
        assert_eq!(store.oldest_log_unix_nano(), Some(10 * MS), "{name}");
    }
}

#[test]
fn chunks_that_overlap_a_lot_still_read_as_one_list() {
    // Logs 3 s out of order at 1 ms apart: every chunk of 1,000 overlaps ten
    // or more of its neighbours.
    let logs = generate(20_000, 5, MS, 3 * SEC);
    for env in envs(|config| StoreConfig {
        log_chunk_max: 1_000,
        ..config
    }) {
        let name = env.name;
        let store = &env.store;
        ingest(store, &logs);
        assert_eq!(sealed(store).len(), 20, "{name}");

        // The oracle: every log by (time, arrival), newest first.
        let mut order: Vec<(u64, usize)> = logs
            .iter()
            .enumerate()
            .map(|(i, log)| (log_time(log), i))
            .collect();
        order.sort_unstable_by(|a, b| b.cmp(a));
        let want: Vec<String> = order.iter().map(|(_, i)| format!("line {i}")).collect();

        let all = walk(store, &LogFilter::default(), 777);
        let got: Vec<&str> = bodies(&all);
        assert_eq!(
            got,
            want.iter().map(String::as_str).collect::<Vec<_>>(),
            "{name}"
        );

        // Random offsets: `logs_at` is that position of the walk.
        let mut rng = Rng::new(3);
        for _ in 0..60 {
            let offset = rng.below(20_000 + 10) as usize;
            let at = store.logs_at(&LogFilter::default(), offset as u64, 4);
            let end = (offset + 4).min(all.len());
            let want_at = if offset < all.len() {
                &all[offset..end]
            } else {
                &[]
            };
            assert_eq!(at, want_at, "{name}: offset {offset}");
        }
    }
}

// ---- counts ------------------------------------------------------------------------------

#[test]
fn log_count_indexed_matches_scan() {
    // 200,000 logs over ten minutes, a little out of order, in 100 chunks.
    let logs = generate(200_000, 7, 3 * MS, 400 * MS);
    let (from, to) = (BASE, BASE + 200_000 * 3 * MS + 400 * MS);
    for env in envs(|config| StoreConfig {
        log_chunk_max: 2_000,
        ..config
    }) {
        let name = env.name;
        let store = &env.store;
        ingest(store, &logs);
        assert_eq!(sealed(store).len(), 100, "{name}");
        assert_eq!(store.stats().logs, 200_000, "{name}");

        let mut rng = Rng::new(99);
        for round in 0..200 {
            let f = random_filter(&mut rng, from, to);
            let want = logs.iter().filter(|log| indexed_match(&f, log)).count() as u64;
            assert_eq!(
                store.log_count(&f),
                Some(want),
                "{name}, round {round}: {f:?}"
            );
        }

        // The filters that have no window, and the ones that match nothing.
        let none = LogFilter::default();
        assert_eq!(store.log_count(&none), Some(200_000), "{name}");
        let nothing = LogFilter {
            levels: Some(LevelSet::default()),
            ..LogFilter::default()
        };
        assert_eq!(store.log_count(&nothing), Some(0), "{name}: no level");
        let ghost = LogFilter {
            service: Some("ghost".to_string()),
            ..LogFilter::default()
        };
        assert_eq!(store.log_count(&ghost), Some(0), "{name}: unknown service");
        let backwards = LogFilter {
            since_unix_nano: Some(to),
            until_unix_nano: Some(from),
            ..LogFilter::default()
        };
        assert_eq!(store.log_count(&backwards), Some(0), "{name}: backwards");
    }
}

#[test]
fn log_count_decodes_only_the_chunks_at_the_edges_of_the_window() {
    // 100 chunks of 1,000 logs, 1 ms apart and in order, so chunk `k` holds
    // the logs of seconds `k` and are disjoint in time.
    let logs = generate(100_000, 11, MS, 0);
    let mut env = envs(|config| StoreConfig {
        log_chunk_max: 1_000,
        ..config
    });
    let env = env.remove(1);
    assert!(env.spilling);
    let store = &env.store;
    ingest(store, &logs);
    assert_eq!(store.log_chunks().on_disk, 100);

    // The whole store, and a window that starts and ends at chunk boundaries:
    // answered from the metadata, nothing decoded.
    let before = store.log_chunks().decodes;
    assert_eq!(store.log_count(&LogFilter::default()), Some(100_000));
    let boundaries = LogFilter {
        since_unix_nano: Some(BASE + 10_000 * MS),
        until_unix_nano: Some(BASE + 90_000 * MS),
        ..LogFilter::default()
    };
    assert_eq!(store.log_count(&boundaries), Some(80_000));
    assert_eq!(store.log_chunks().decodes, before, "no chunk was decoded");

    // A window with both edges inside chunks decodes those two, and no more.
    let inside = LogFilter {
        since_unix_nano: Some(BASE + 10_500 * MS),
        until_unix_nano: Some(BASE + 89_500 * MS),
        levels: Some(LevelSet::from_groups(true, true, false, false)),
        service: Some("payments-api".to_string()),
        ..LogFilter::default()
    };
    let want = logs
        .iter()
        .filter(|log| indexed_match(&inside, log))
        .count();
    assert_eq!(store.log_count(&inside), Some(want as u64));
    assert_eq!(
        store.log_chunks().decodes,
        before + 2,
        "the two edge chunks"
    );
    // Asked again, the edge chunks are in the cache.
    assert_eq!(store.log_count(&inside), Some(want as u64));
    assert_eq!(store.log_chunks().decodes, before + 2, "from the cache");
}

#[test]
fn log_count_none_for_body_filter() {
    for env in envs(|config| StoreConfig {
        log_chunk_max: 50,
        ..config
    }) {
        let name = env.name;
        let store = &env.store;
        ingest(store, &generate(500, 2, MS, 0));

        // Indexed: the window, levels and service, alone or together.
        let indexed = [
            LogFilter::default(),
            LogFilter {
                since_unix_nano: Some(BASE),
                until_unix_nano: Some(BASE + SEC),
                ..LogFilter::default()
            },
            LogFilter {
                levels: Some(LevelSet::ALL),
                service: Some("web-bff".to_string()),
                ..LogFilter::default()
            },
        ];
        for f in &indexed {
            assert!(f.is_indexed(), "{name}: {f:?}");
            assert!(store.log_count(f).is_some(), "{name}: {f:?}");
        }

        // Anything else needs the logs themselves, and has no count.
        let query = loctel_query::logql::parse("{service_name=\"web-bff\"}").unwrap();
        let others = [
            LogFilter {
                contains: Some("line".to_string()),
                ..LogFilter::default()
            },
            LogFilter {
                contains: Some(String::new()),
                ..LogFilter::default()
            },
            LogFilter {
                min_severity: Some(Severity::Warn),
                ..LogFilter::default()
            },
            LogFilter {
                trace_id: Some(tid(1)),
                ..LogFilter::default()
            },
            LogFilter {
                span_id: Some(sid(1)),
                ..LogFilter::default()
            },
            LogFilter {
                query: Some(Arc::new(query)),
                ..LogFilter::default()
            },
            // One of the others with the indexed ones is not indexed either.
            LogFilter {
                since_unix_nano: Some(BASE),
                service: Some("web-bff".to_string()),
                contains: Some("line".to_string()),
                ..LogFilter::default()
            },
        ];
        for f in &others {
            assert!(!f.is_indexed(), "{name}: {f:?}");
            assert_eq!(store.log_count(f), None, "{name}: {f:?}");
        }
    }
}

// ---- offsets --------------------------------------------------------------------------------

#[test]
fn logs_at_matches_paged_search() {
    // 140,000 logs in order: two sealed chunks of 65,536 and 8,928 in the
    // open one, so the offsets below fall inside a chunk and on the edge of
    // one.
    let logs = generate(140_000, 21, MS, 0);
    for env in envs(plan_chunks) {
        let name = env.name;
        let store = &env.store;
        ingest(store, &logs);
        assert_eq!(
            (store.log_chunks().open, sealed(store)),
            (8_928, vec![65_536, 65_536]),
            "{name}"
        );

        let none = LogFilter::default();
        let all = walk(store, &none, 5_000);
        assert_eq!(all.len(), 140_000, "{name}");
        let offsets = [
            0u64, 1, 4_999, 8_927, 8_928, 65_535, 65_536, 74_463, 74_464, 139_998, 139_999,
        ];
        for offset in offsets {
            let at = store.logs_at(&none, offset, 3);
            let end = (offset as usize + 3).min(all.len());
            assert_eq!(at, all[offset as usize..end], "{name}: offset {offset}");
        }
        assert!(
            store.logs_at(&none, 140_000, 3).is_empty(),
            "{name}: past the end"
        );
        assert!(store.logs_at(&none, 0, 0).is_empty(), "{name}: no rows");

        // With a filter, the offset counts the logs that pass it.
        let filtered = LogFilter {
            levels: Some(LevelSet::from_groups(true, true, false, false)),
            service: Some("orders-api".to_string()),
            since_unix_nano: Some(BASE + 20_000 * MS),
            until_unix_nano: Some(BASE + 130_000 * MS),
            ..LogFilter::default()
        };
        let walked = walk(store, &filtered, 5_000);
        assert!(walked.len() > 1_000, "{name}: a real filter");
        assert_eq!(
            store.log_count(&filtered),
            Some(walked.len() as u64),
            "{name}"
        );
        let mut rng = Rng::new(8);
        let mut tries = vec![0, 1, walked.len() as u64 - 1, walked.len() as u64];
        tries.extend((0..40).map(|_| rng.below(walked.len() as u64 + 5)));
        for offset in tries {
            let at = store.logs_at(&filtered, offset, 4);
            let end = (offset as usize + 4).min(walked.len());
            let want = if (offset as usize) < walked.len() {
                &walked[offset as usize..end]
            } else {
                &[]
            };
            assert_eq!(at, want, "{name}: filtered offset {offset}");
        }
    }
}

#[test]
fn logs_at_decodes_at_most_two_chunks_when_chunks_do_not_overlap() {
    let logs = generate(100_000, 31, MS, 0);
    let mut env = envs(|config| StoreConfig {
        log_chunk_max: 1_000,
        ..config
    });
    let env = env.remove(1);
    let store = &env.store;
    ingest(store, &logs);
    let none = LogFilter::default();
    let mut rng = Rng::new(4);
    for _ in 0..40 {
        let offset = rng.below(99_990);
        let before = store.log_chunks().decodes;
        let rows = store.logs_at(&none, offset, 50);
        assert_eq!(rows.len(), 50);
        let decoded = store.log_chunks().decodes - before;
        assert!(decoded <= 2, "offset {offset} decoded {decoded} chunks");
        assert_eq!(
            rows[0].1.body,
            format!("line {}", 99_999 - offset),
            "the newest first"
        );
    }
}

#[test]
fn logs_at_scans_for_a_filter_that_is_not_indexed() {
    for env in envs(|config| StoreConfig {
        log_chunk_max: 100,
        ..config
    }) {
        let name = env.name;
        let store = &env.store;
        ingest(store, &generate(3_000, 41, MS, 20 * MS));
        let f = LogFilter {
            contains: Some("3".to_string()),
            levels: Some(LevelSet::from_groups(true, true, true, false)),
            ..LogFilter::default()
        };
        let walked = walk(store, &f, 200);
        assert!(walked.len() > 100, "{name}");
        for offset in [0usize, 1, 57, walked.len() - 1] {
            let at = store.logs_at(&f, offset as u64, 3);
            let end = (offset + 3).min(walked.len());
            assert_eq!(at, walked[offset..end], "{name}: offset {offset}");
        }
        assert!(
            store.logs_at(&f, walked.len() as u64, 3).is_empty(),
            "{name}"
        );
    }
}

// ---- the histogram -----------------------------------------------------------------------------

/// The buckets of `[since, until)` counted from the logs themselves.
fn oracle_histogram(
    logs: &[LogRecord],
    f: &LogFilter,
    since: u64,
    until: u64,
    buckets: usize,
) -> Vec<HistogramBucket> {
    let mut out = vec![HistogramBucket::default(); buckets];
    let width = u128::from(until - since);
    for log in logs {
        let time = log_time(log);
        let in_window = time >= since && time < until;
        let others = LogFilter {
            since_unix_nano: None,
            until_unix_nano: None,
            ..f.clone()
        };
        if in_window && indexed_match(&others, log) {
            let index = (u128::from(time - since) * buckets as u128 / width) as usize;
            out[index].total += 1;
            if log.severity >= Severity::Error {
                out[index].errors += 1;
            }
        }
    }
    out
}

#[test]
fn histogram_from_meta_equals_scan() {
    // 60,000 logs in order, 10 ms apart: ten minutes, in 60 chunks of 1,000.
    let logs = generate(60_000, 51, 10 * MS, 0);
    let filters = [
        LogFilter::default(),
        LogFilter {
            levels: Some(LevelSet::from_groups(true, true, false, false)),
            ..LogFilter::default()
        },
        LogFilter {
            service: Some("payments-api".to_string()),
            ..LogFilter::default()
        },
        LogFilter {
            levels: Some(LevelSet::from_groups(true, false, true, false)),
            service: Some("web-bff".to_string()),
            ..LogFilter::default()
        },
        LogFilter {
            levels: Some(LevelSet::default()),
            ..LogFilter::default()
        },
    ];
    for env in envs(|config| StoreConfig {
        log_chunk_max: 1_000,
        ..config
    }) {
        let name = env.name;
        let store = &env.store;
        ingest(store, &logs);

        // The window is the ten minutes, in 40 buckets of 15 s: every bucket
        // edge is a whole second, and so is every edge of a chunk's seconds. The
        // answer is the metadata's, and nothing is decoded.
        let (since, until, buckets) = (BASE, BASE + 600 * SEC, 40);
        let before = store.log_chunks().decodes;
        for f in &filters {
            let want = oracle_histogram(&logs, f, since, until, buckets);
            assert_eq!(
                store.log_histogram(f, since, until, buckets),
                want,
                "{name}: {f:?}"
            );
        }
        assert_eq!(
            store.log_chunks().decodes,
            before,
            "{name}: from the counts, nothing decoded"
        );
        // The same filters made scans (the empty text matches every log): the
        // old way of counting, which must give the same bars, and has to read the
        // logs to do it.
        for f in &filters {
            let scan = LogFilter {
                contains: Some(String::new()),
                ..f.clone()
            };
            let want = oracle_histogram(&logs, f, since, until, buckets);
            assert_eq!(
                store.log_histogram(&scan, since, until, buckets),
                want,
                "{name}: scan {f:?}"
            );
        }
        if env.spilling {
            assert!(
                store.log_chunks().decodes > before,
                "{name}: the scans read the chunks"
            );
        }

        // A window whose bucket edges fall inside a second, and inside a chunk,
        // is still exact: what cannot be answered from the counts is read.
        let (since, until) = (
            BASE + 17 * SEC + 123_456_789,
            BASE + 431 * SEC + 987_654_321,
        );
        for f in &filters {
            let want = oracle_histogram(&logs, f, since, until, 48);
            assert_eq!(
                store.log_histogram(f, since, until, 48),
                want,
                "{name}: odd {f:?}"
            );
        }
        // Few buckets, so that each chunk is inside one: the whole-chunk counts.
        for buckets in [1, 2, 3, 7] {
            for f in &filters {
                let want = oracle_histogram(&logs, f, BASE, BASE + 700 * SEC, buckets);
                assert_eq!(
                    store.log_histogram(f, BASE, BASE + 700 * SEC, buckets),
                    want,
                    "{name}: {buckets} buckets {f:?}"
                );
            }
        }
    }
}

#[test]
fn histogram_of_overlapping_chunks_and_odd_windows_equals_the_oracle() {
    let logs = generate(30_000, 61, 5 * MS, 2 * SEC);
    let to = BASE + 30_000 * 5 * MS + 2 * SEC;
    for env in envs(|config| StoreConfig {
        log_chunk_max: 700,
        ..config
    }) {
        let name = env.name;
        let store = &env.store;
        ingest(store, &logs);
        let mut rng = Rng::new(17);
        for round in 0..40 {
            let f = random_filter(&mut rng, BASE, to);
            let since = BASE - SEC + rng.below(to - BASE);
            let until = since + 1 + rng.below(to - since + 5 * SEC);
            let buckets = 1 + rng.below(60) as usize;
            let want = oracle_histogram(&logs, &f, since, until, buckets);
            assert_eq!(
                store.log_histogram(&f, since, until, buckets),
                want,
                "{name}, round {round}: {f:?} [{since}, {until}) in {buckets}"
            );
        }
    }
}

// ---- the cache -------------------------------------------------------------------------------------

#[test]
fn chunk_cache_bounded() {
    let logs = generate(4_000, 71, MS, 0);
    let mut env = envs(|config| StoreConfig {
        log_chunk_max: 100,
        ..config
    });
    let env = env.remove(1);
    let store = &env.store;
    ingest(store, &logs);
    let info = store.log_chunks();
    assert_eq!((info.sealed.len(), info.on_disk, info.cached), (40, 40, 0));
    let all = walk(store, &LogFilter::default(), 4_000);
    assert_eq!(all.len(), 4_000);
    let after_walk = store.log_chunks();
    assert!(after_walk.cached <= 8, "{after_walk:?}");

    // Read a log of every chunk: each is a decode, and the cache never holds
    // more than eight.
    let start = store.log_chunks().decodes;
    for chunk in 0..40usize {
        let (id, want) = &all[chunk * 100 + 50];
        assert_eq!(&store.log(*id).unwrap(), want);
        let info = store.log_chunks();
        assert!(info.cached <= 8, "chunk {chunk}: {info:?}");
    }
    assert!(
        store.log_chunks().decodes - start >= 32,
        "most reads decoded"
    );
    assert_eq!(store.log_chunks().cached, 8);

    // The eight newest read are in it: reading them again decodes nothing.
    let decodes = store.log_chunks().decodes;
    for chunk in 32..40usize {
        let (id, want) = &all[chunk * 100 + 50];
        assert_eq!(&store.log(*id).unwrap(), want);
    }
    assert_eq!(store.log_chunks().decodes, decodes, "all from the cache");
    // An older one decodes, and pushes the least recently used one out.
    let (id, want) = &all[3 * 100 + 50];
    assert_eq!(&store.log(*id).unwrap(), want);
    assert_eq!(store.log_chunks().decodes, decodes + 1);
    assert_eq!(store.log_chunks().cached, 8);
    let (id, _) = &all[32 * 100 + 50];
    store.log(*id).unwrap();
    assert_eq!(
        store.log_chunks().decodes,
        decodes + 2,
        "it was the one pushed out"
    );
}

// ---- eviction, removal and retries across chunks -----------------------------------------

#[test]
fn eviction_trims_the_oldest_chunk_and_drops_whole_ones() {
    for env in envs(|config| StoreConfig {
        log_chunk_max: 10,
        max_log_records: 25,
        ..config
    }) {
        let name = env.name;
        let store = &env.store;
        let logs = generate(100, 81, MS, 0);
        for (done, batch) in logs.chunks(7).enumerate() {
            store.ingest_logs(batch.to_vec());
            let held = ((done + 1) * 7).min(100).min(25);
            let stats = store.stats();
            assert_eq!(stats.logs, held, "{name}: after batch {done}");
            assert_eq!(
                store.log_count(&LogFilter::default()),
                Some(held as u64),
                "{name}: the counts follow the trimming"
            );
        }
        // Exactly the newest 25 remain, whichever chunks hold them.
        let all = walk(store, &LogFilter::default(), 6);
        let want: Vec<String> = (75..100).rev().map(|i| format!("line {i}")).collect();
        assert_eq!(
            bodies(&all),
            want.iter().map(String::as_str).collect::<Vec<_>>(),
            "{name}"
        );
        assert_eq!(
            store.oldest_log_unix_nano(),
            Some(log_time(&logs[75])),
            "{name}"
        );
        assert_eq!(store.stats().logs_received, 100, "{name}");
        // A log that was evicted is gone for its ID and for a window.
        let window = LogFilter {
            until_unix_nano: Some(log_time(&logs[75])),
            ..LogFilter::default()
        };
        assert_eq!(store.log_count(&window), Some(0), "{name}");
        assert_eq!(
            store.logs_at(&LogFilter::default(), 24, 5).len(),
            1,
            "{name}: the oldest held is the last row"
        );

        // Lowering the number of logs held is the same trimming, through a
        // fresh store: the limit applies after each batch, and a retry of an
        // evicted log is new again.
        let retry = logs[3].clone();
        store.ingest_logs(vec![retry]);
        assert_eq!(store.stats().logs_received, 101, "{name}: taken as new");
        assert_eq!(store.stats().logs, 25, "{name}: and the first to go");
    }
}

#[test]
fn removal_reaches_sealed_chunks() {
    for env in envs(|config| StoreConfig {
        log_chunk_max: 5,
        ..config
    }) {
        let name = env.name;
        let store = &env.store;
        // 40 logs, every fourth in span 1 of trace 1, every fifth in span 2 of
        // trace 1, every seventh in trace 2 without a span.
        let logs: Vec<LogRecord> = (0..40u64)
            .map(|i| {
                let mut line = log("orders-api", &format!("l{i}"), 10 + i);
                if i % 4 == 0 {
                    line = in_span(line, 1, 1);
                } else if i % 5 == 0 {
                    line = in_span(line, 1, 2);
                } else if i % 7 == 0 {
                    line = in_trace(line, 2);
                }
                line
            })
            .collect();
        store.ingest_logs(logs.clone());
        assert_eq!(sealed(store).len(), 8, "{name}");

        // `logs_for_span` finds a span's logs in every chunk, oldest first.
        let span1: Vec<String> = store
            .logs_for_span(tid(1), sid(1))
            .iter()
            .map(|l| l.body.clone())
            .collect();
        let want1: Vec<String> = (0..40)
            .filter(|i| i % 4 == 0)
            .map(|i| format!("l{i}"))
            .collect();
        assert_eq!(span1, want1, "{name}");

        // Removing the logs of a span takes them out of every chunk, and
        // leaves the counts right.
        let removed = store.remove_spans(tid(1), &[sid(1)]);
        assert_eq!((removed.spans, removed.logs), (0, 10), "{name}");
        assert_eq!(store.stats().logs, 30, "{name}");
        assert_eq!(store.log_count(&LogFilter::default()), Some(30), "{name}");
        assert!(store.logs_for_span(tid(1), sid(1)).is_empty(), "{name}");
        let left = walk(store, &LogFilter::default(), 7);
        assert_eq!(left.len(), 30, "{name}");
        assert!(
            left.iter().all(|(_, l)| l.span_id != Some(sid(1))),
            "{name}"
        );
        assert!(left.windows(2).all(|pair| pair[0].0 > pair[1].0), "{name}");

        // A removed log is forgotten whole: a retry is new, and a retry of one
        // that stayed is not.
        store.ingest_logs(vec![logs[4].clone(), logs[1].clone()]);
        assert_eq!(store.stats().logs, 31, "{name}");
        assert_eq!(store.stats().logs_received, 41, "{name}");

        // `remove_trace` takes every log of the trace, with or without a span,
        // from chunks that are sealed and the one that is open: the six logs of
        // span 2, and the retry of l4 that sits in the open chunk.
        let removed = store.remove_trace(tid(1));
        assert_eq!(removed.logs, 7, "{name}");
        let held = walk(store, &LogFilter::default(), 9);
        assert_eq!(held.len(), 24, "{name}");
        assert!(
            held.iter().all(|(_, l)| l.trace_id != Some(tid(1))),
            "{name}"
        );
        assert_eq!(store.stats().logs, 24, "{name}");
        assert_eq!(store.log_count(&LogFilter::default()), Some(24), "{name}");
        // The logs of trace 2 (the ones that were not in trace 1 as well) are
        // all still there.
        let trace2 = LogFilter {
            trace_id: Some(tid(2)),
            ..LogFilter::default()
        };
        let expected2: Vec<String> = (0..40u64)
            .filter(|i| i % 4 != 0 && i % 5 != 0 && i % 7 == 0)
            .map(|i| format!("l{i}"))
            .rev()
            .collect();
        let got2 = walk(store, &trace2, 3);
        assert_eq!(
            bodies(&got2),
            expected2.iter().map(String::as_str).collect::<Vec<_>>(),
            "{name}"
        );
        assert_eq!(store.remove_trace(tid(2)).logs, expected2.len(), "{name}");
        assert_eq!(store.remove_trace(tid(2)).logs, 0, "{name}: nothing twice");
    }
}

#[test]
fn a_retry_of_a_sealed_log_is_ignored() {
    for env in envs(|config| StoreConfig {
        log_chunk_max: 3,
        ..config
    }) {
        let name = env.name;
        let store = &env.store;
        let batch: Vec<LogRecord> = (0..9u64)
            .map(|i| log("a", &format!("l{i}"), 10 + i))
            .collect();
        store.ingest_logs(batch.clone());
        assert_eq!(sealed(store), vec![3, 3, 3], "{name}");
        store.ingest_logs(batch.clone());
        assert_eq!(store.stats().logs, 9, "{name}");
        assert_eq!(store.stats().logs_received, 9, "{name}");
        assert_eq!(sealed(store), vec![3, 3, 3], "{name}: nothing new to hold");
    }
}

#[test]
fn clear_drops_every_chunk_and_its_files() {
    let mut env = envs(|config| StoreConfig {
        log_chunk_max: 20,
        ..config
    });
    let env = env.remove(1);
    let store = &env.store;
    ingest(store, &generate(500, 91, MS, 0));
    assert_eq!(store.log_chunks().sealed.len(), 25);
    assert!(!segment_files(env.dir.path()).is_empty());
    store.clear();
    let info = store.log_chunks();
    assert_eq!((info.open, info.sealed.len(), info.cached), (0, 0, 0));
    assert!(segment_files(env.dir.path()).is_empty(), "the files went");
    assert_eq!(store.stats().logs, 0);
    assert_eq!(store.log_count(&LogFilter::default()), Some(0));
    assert!(store.logs_at(&LogFilter::default(), 0, 5).is_empty());
    // And it takes logs again.
    ingest(store, &generate(30, 92, MS, 0));
    assert_eq!(store.stats().logs, 30);
}

fn segment_files(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .filter(|name| name.ends_with(".seg"))
        .collect();
    names.sort();
    names
}

// ---- searching a chunk only when it can hold the answer ---------------------------------

#[test]
fn search_skips_chunks_that_cannot_match() {
    let mut env = envs(|config| StoreConfig {
        log_chunk_max: 100,
        ..config
    });
    let env = env.remove(1);
    let store = &env.store;
    // Ten chunks from "inventory", then one chunk with the only logs of
    // "rare-service" (and of trace 7), then ten more from "inventory".
    let mut logs = Vec::new();
    for i in 0..1_000u64 {
        logs.push(log("inventory", &format!("a{i}"), i));
    }
    for i in 0..100u64 {
        logs.push(in_trace(
            log("rare-service", &format!("r{i}"), 1_000 + i),
            7,
        ));
    }
    for i in 0..1_000u64 {
        logs.push(log("inventory", &format!("b{i}"), 1_100 + i));
    }
    store.ingest_logs(logs);
    assert_eq!(store.log_chunks().sealed.len(), 21);

    // A service that one chunk has: that chunk is read, the 20 others are not.
    let before = store.log_chunks().decodes;
    let rare = LogFilter {
        service: Some("rare-service".to_string()),
        ..LogFilter::default()
    };
    let found = store.search_logs_keyed(&rare, &page(1_000, None));
    assert_eq!(found.items.len(), 100);
    assert_eq!(store.log_chunks().decodes - before, 1);

    // The same for a trace, and for the service of a LogQL selector.
    let before = store.log_chunks().decodes;
    let trace = LogFilter {
        trace_id: Some(tid(7)),
        ..LogFilter::default()
    };
    assert_eq!(
        store
            .search_logs_keyed(&trace, &page(1_000, None))
            .items
            .len(),
        100
    );
    assert_eq!(
        store.log_chunks().decodes - before,
        0,
        "the chunk is in the cache"
    );
    let query = loctel_query::logql::parse("{service_name=\"rare-service\"} |= \"r5\"").unwrap();
    let selector = LogFilter {
        query: Some(Arc::new(query)),
        ..LogFilter::default()
    };
    let found = store.search_logs_keyed(&selector, &page(1_000, None));
    assert_eq!(found.items.len(), 11, "r5 and r50 to r59");
    assert_eq!(store.log_chunks().decodes - before, 0);

    // A service nobody logged from reads nothing at all.
    let before = store.log_chunks().decodes;
    let ghost = LogFilter {
        service: Some("ghost".to_string()),
        ..LogFilter::default()
    };
    assert!(
        store
            .search_logs_keyed(&ghost, &page(10, None))
            .items
            .is_empty()
    );
    assert_eq!(store.log_chunks().decodes, before);
}

// ---- reading the logs in place ------------------------------------------------------------

#[test]
fn scan_logs_is_oldest_first_across_overlapping_chunks() {
    let logs = generate(5_000, 101, MS, SEC);
    for env in envs(|config| StoreConfig {
        log_chunk_max: 300,
        ..config
    }) {
        let name = env.name;
        let store = &env.store;
        ingest(store, &logs);
        let mut want: Vec<(u64, usize)> = logs
            .iter()
            .enumerate()
            .map(|(i, log)| (log_time(log), i))
            .collect();
        want.sort_unstable();

        let mut seen = Vec::new();
        store.scan_logs(0, |log| seen.push(log.body.clone()));
        let expected: Vec<String> = want.iter().map(|(_, i)| format!("line {i}")).collect();
        assert_eq!(seen, expected, "{name}");

        // From a time on: the logs at or after it, still oldest first.
        let since = BASE + 2 * SEC;
        let mut after = Vec::new();
        store.scan_logs(since, |log| after.push(log.body.clone()));
        let expected: Vec<String> = want
            .iter()
            .filter(|(time, _)| *time >= since)
            .map(|(_, i)| format!("line {i}"))
            .collect();
        assert_eq!(after, expected, "{name}");
    }
}

// ---- damage and a full disk ---------------------------------------------------------------------

#[test]
fn a_damaged_log_chunk_is_skipped_not_fatal() {
    let dir = TempDir::new().unwrap();
    let (store, _clock) = store_with_config(StoreConfig {
        log_chunk_max: 10,
        segment_max_bytes: 64 * 1024 * 1024,
        ..StoreConfig::spilling_for_tests(dir.path().to_path_buf())
    });
    let logs: Vec<LogRecord> = (0..30u64)
        .map(|i| log("a", &format!("l{i}"), 10 + i))
        .collect();
    store.ingest_logs(logs);
    assert_eq!(sealed(&store), vec![10, 10, 10]);

    // Flip a byte in the first record (the oldest chunk) before it is read.
    let files = segment_files(dir.path());
    assert_eq!(files, vec!["logs-000000.seg".to_string()]);
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(dir.path().join(&files[0]))
        .unwrap();
    let mut byte = [0u8; 1];
    file.read_at(&mut byte, 20).unwrap();
    file.write_at(&[byte[0] ^ 0xFF], 20).unwrap();

    // The other chunks read as before; the damaged one is not there, and
    // nothing panics.
    let all = walk(&store, &LogFilter::default(), 7);
    let want: Vec<String> = (10..30).rev().map(|i| format!("l{i}")).collect();
    assert_eq!(
        bodies(&all),
        want.iter().map(String::as_str).collect::<Vec<_>>()
    );
    assert!(store.logs_at(&LogFilter::default(), 0, 100).len() <= 30);
    let _ = store.log_count(&LogFilter::default());
    let _ = store.log_histogram(&LogFilter::default(), 0, 100 * MS, 10);
    let mut scanned = 0;
    store.scan_logs(0, |_| scanned += 1);
    assert_eq!(scanned, 20);
    assert_eq!(store.logs_for_span(tid(1), sid(1)).len(), 0);

    // New logs go on, and when the cap comes the unreadable chunk is dropped
    // whole, as the oldest.
    store.ingest_logs(vec![log("a", "new", 100)]);
    assert!(store.stats().logs >= 21);
}

#[test]
fn a_failed_chunk_write_keeps_the_logs_in_memory() {
    let dir = TempDir::new().unwrap();
    let (store, _clock) = store_with_config(StoreConfig {
        log_chunk_max: 10,
        ..StoreConfig::spilling_for_tests(dir.path().to_path_buf())
    });
    assert_eq!(store.spill_error(), None);

    // The directory goes away under the store, so no segment can be made.
    fs::remove_dir_all(dir.path()).unwrap();
    let logs: Vec<LogRecord> = (0..25u64)
        .map(|i| log("a", &format!("l{i}"), 10 + i))
        .collect();
    store.ingest_logs(logs);

    // Two chunks sealed, in memory, and the store says why they are not on disk.
    let info = store.log_chunks();
    assert_eq!(
        (info.open, info.sealed.clone(), info.on_disk),
        (5, vec![10, 10], 0)
    );
    assert!(store.spill_error().is_some());
    let all = walk(&store, &LogFilter::default(), 6);
    assert_eq!(all.len(), 25);
    assert_eq!(store.log_count(&LogFilter::default()), Some(25));
    assert_eq!(store.logs_at(&LogFilter::default(), 12, 1)[0].1.body, "l12");

    // Space comes back: the next chunk goes to disk, and the error is gone.
    fs::create_dir(dir.path()).unwrap();
    let more: Vec<LogRecord> = (25..40u64)
        .map(|i| log("a", &format!("l{i}"), 10 + i))
        .collect();
    store.ingest_logs(more);
    let info = store.log_chunks();
    assert_eq!(
        (info.open, info.sealed.clone(), info.on_disk),
        (0, vec![10, 10, 10, 10], 2),
        "the two sealed since are on disk"
    );
    assert_eq!(store.spill_error(), None);
    assert_eq!(walk(&store, &LogFilter::default(), 11).len(), 40);
}

// ---- a plain model to compare with ------------------------------------------------------------

/// What the store is supposed to hold, kept the plain way: every held log with
/// its id in one sorted list, found and removed by looking at all of them.
struct Model {
    held: Vec<(LogId, LogRecord)>,
    next_seq: u64,
    cap: usize,
}

impl Model {
    fn new(cap: usize) -> Model {
        Model {
            held: Vec::new(),
            next_seq: 0,
            cap,
        }
    }

    /// A batch, the way the store takes it: a retry of a held log is ignored,
    /// the others get the next arrival numbers, and the oldest logs beyond the
    /// cap go once the batch is in.
    fn ingest(&mut self, logs: &[LogRecord]) {
        for log in logs {
            let key = log.dedup_key();
            if self.held.iter().any(|(_, held)| held.dedup_key() == key) {
                continue;
            }
            let id = LogId {
                time_unix_nano: log_time(log),
                seq: self.next_seq,
            };
            self.next_seq += 1;
            self.held.push((id, log.clone()));
        }
        self.held.sort_by_key(|(id, _)| *id);
        let excess = self.held.len().saturating_sub(self.cap);
        self.held.drain(..excess);
    }

    fn remove_where(&mut self, doomed: impl Fn(&LogRecord) -> bool) -> usize {
        let before = self.held.len();
        self.held.retain(|(_, log)| !doomed(log));
        before - self.held.len()
    }

    fn newest_first(&self) -> Vec<(LogId, LogRecord)> {
        self.held.iter().rev().cloned().collect()
    }

    fn logs(&self) -> Vec<LogRecord> {
        self.held.iter().map(|(_, log)| log.clone()).collect()
    }
}

/// Everything the store can be asked about its logs, compared with the model.
fn assert_agrees(store: &Store, model: &Model, rng: &mut Rng, context: &str) {
    let want = model.newest_first();
    let all = LogFilter::default();
    assert_eq!(walk(store, &all, 40), want, "{context}: the list");
    assert_eq!(store.stats().logs, want.len(), "{context}: stats");
    assert_eq!(
        store.log_count(&all),
        Some(want.len() as u64),
        "{context}: count"
    );
    assert_eq!(
        store.oldest_log_unix_nano(),
        model.held.first().map(|(id, _)| id.time_unix_nano),
        "{context}: oldest"
    );

    // An indexed filter: its count, the rows at some offsets and its histogram.
    let (from, to) = (BASE - 2 * SEC, BASE + 80 * SEC);
    let f = random_filter(rng, from, to);
    let matching: Vec<(LogId, LogRecord)> = want
        .iter()
        .filter(|(_, log)| indexed_match(&f, log))
        .cloned()
        .collect();
    assert_eq!(
        store.log_count(&f),
        Some(matching.len() as u64),
        "{context}: count of {f:?}"
    );
    let last = matching.len() as u64;
    for offset in [
        0,
        1,
        last / 2,
        last.saturating_sub(1),
        last,
        last + 3,
        rng.below(last + 2),
    ] {
        let end = (offset as usize + 3).min(matching.len());
        let rows = if (offset as usize) < matching.len() {
            &matching[offset as usize..end]
        } else {
            &[]
        };
        assert_eq!(
            store.logs_at(&f, offset, 3),
            rows,
            "{context}: offset {offset} of {f:?}"
        );
    }
    let since = from + rng.below(to - from);
    let until = since + 1 + rng.below(30 * SEC);
    let buckets = 1 + rng.below(50) as usize;
    assert_eq!(
        store.log_histogram(&f, since, until, buckets),
        oracle_histogram(&model.logs(), &f, since, until, buckets),
        "{context}: histogram of {f:?} [{since}, {until}) in {buckets}"
    );
    // A filter that is not indexed pages through the same list.
    let service = LogFilter {
        service: Some(SERVICES[rng.below(5) as usize].to_string()),
        contains: Some("1".to_string()),
        ..LogFilter::default()
    };
    let by_service: Vec<(LogId, LogRecord)> = want
        .iter()
        .filter(|(_, log)| {
            service.service.as_deref() == Some(log.service()) && log.body.contains('1')
        })
        .cloned()
        .collect();
    assert_eq!(
        walk(store, &service, 4),
        by_service,
        "{context}: scan filter"
    );
    let offset = rng.below(by_service.len() as u64 + 2);
    let end = (offset as usize + 2).min(by_service.len());
    let rows = if (offset as usize) < by_service.len() {
        &by_service[offset as usize..end]
    } else {
        &[]
    };
    assert_eq!(
        store.logs_at(&service, offset, 2),
        rows,
        "{context}: scan offset"
    );

    // One log by its id, and an id that is not held.
    if !want.is_empty() {
        let at = rng.below(want.len() as u64) as usize;
        let (id, log) = &want[at];
        assert_eq!(store.log(*id).as_ref(), Some(log), "{context}: by id");
        // The lines around it.
        let around = store.logs_around(*id, 2, 2);
        let oldest_first_at = want.len() - 1 - at;
        let held = model.newest_first().into_iter().rev().collect::<Vec<_>>();
        let start = oldest_first_at.saturating_sub(2);
        let end = (oldest_first_at + 3).min(held.len());
        assert_eq!(around.lines, held[start..end], "{context}: around");
        assert_eq!(
            around.target,
            Some(oldest_first_at - start),
            "{context}: target"
        );
    }
    let missing = LogId {
        time_unix_nano: BASE + rng.below(80 * SEC),
        seq: u64::MAX - 1,
    };
    assert_eq!(store.log(missing), None, "{context}: not held");
    let around = store.logs_around(missing, 1, 1);
    let below = model.held.iter().rev().find(|(id, _)| *id < missing);
    let above = model.held.iter().find(|(id, _)| *id > missing);
    let near: Vec<(LogId, LogRecord)> = below.into_iter().chain(above).cloned().collect();
    assert_eq!(around.lines, near, "{context}: around a log that is gone");
    assert_eq!(around.target, None, "{context}");

    // A span's logs, oldest first.
    let (trace, span) = (tid(1 + rng.below(3) as u8), sid(1 + rng.below(2) as u8));
    let of_span: Vec<LogRecord> = model
        .held
        .iter()
        .filter(|(_, log)| log.trace_id == Some(trace) && log.span_id == Some(span))
        .map(|(_, log)| log.clone())
        .collect();
    assert_eq!(store.logs_for_span(trace, span), of_span, "{context}: span");

    // The scan, oldest first, from a time on.
    let since = BASE + rng.below(60 * SEC);
    let mut scanned = Vec::new();
    store.scan_logs(since, |log| scanned.push(log.clone()));
    let want_scan: Vec<LogRecord> = model
        .held
        .iter()
        .filter(|(id, _)| id.time_unix_nano >= since)
        .map(|(_, log)| log.clone())
        .collect();
    assert_eq!(scanned, want_scan, "{context}: scan");
}

/// A batch of logs near `now`: most on time, some late, a few retries of logs
/// sent before, some with trace context, one in twelve without a timestamp.
fn random_batch(
    rng: &mut Rng,
    resources: &[Arc<Resource>],
    now: u64,
    sent: &mut Vec<LogRecord>,
    serial: &mut u64,
) -> Vec<LogRecord> {
    let size = 1 + rng.below(25);
    (0..size)
        .map(|_| {
            if !sent.is_empty() && rng.below(10) == 0 {
                return sent[rng.below(sent.len() as u64) as usize].clone();
            }
            *serial += 1;
            let late = if rng.below(5) == 0 {
                rng.below(6 * SEC)
            } else {
                0
            };
            let time = now.saturating_sub(late) + rng.below(SEC);
            let severity = SEVERITIES[rng.below(7) as usize];
            let mut log = record(
                &resources[rng.below(5) as usize],
                format!("line {serial}{}", if rng.below(3) == 0 { " 1" } else { "" }),
                time,
                severity,
            );
            if rng.below(12) == 0 {
                log.time_unix_nano = 0;
            }
            if rng.below(5) < 2 {
                log.trace_id = Some(tid(1 + rng.below(3) as u8));
                if rng.below(3) != 0 {
                    log.span_id = Some(sid(1 + rng.below(2) as u8));
                }
            }
            sent.push(log.clone());
            log
        })
        .collect()
}

#[test]
fn random_operations_agree_with_a_plain_model() {
    let resources: Vec<Arc<Resource>> = SERVICES.iter().map(|name| resource(name)).collect();
    for (chunk_max, cap) in [(3, 90), (7, 400), (7, 60), (40, 150), (500, 300)] {
        for env in envs(|config| StoreConfig {
            log_chunk_max: chunk_max,
            max_log_records: cap,
            ..config
        }) {
            let context = format!("{} chunks of {chunk_max}, cap {cap}", env.name);
            let mut rng = Rng::new((chunk_max as u64 * 31).wrapping_add(cap as u64));
            let mut model = Model::new(cap);
            let store = &env.store;
            let (mut sent, mut serial) = (Vec::new(), 0u64);
            let mut now = BASE;
            for step in 0..100 {
                let context = format!("{context}, step {step}");
                match rng.below(12) {
                    0 => {
                        let trace = tid(1 + rng.below(3) as u8);
                        let removed = store.remove_trace(trace);
                        let want = model.remove_where(|log| log.trace_id == Some(trace));
                        assert_eq!(removed.logs, want, "{context}: remove_trace");
                    }
                    1 => {
                        let trace = tid(1 + rng.below(3) as u8);
                        let spans = [sid(1 + rng.below(2) as u8), sid(9)];
                        let removed = store.remove_spans(trace, &spans);
                        let want = model.remove_where(|log| {
                            log.trace_id == Some(trace)
                                && log.span_id.is_some_and(|span| spans.contains(&span))
                        });
                        assert_eq!(removed.logs, want, "{context}: remove_spans");
                    }
                    2 => {
                        // Time passes (and a chunk that is old enough seals).
                        let by = Duration::from_millis(rng.below(12_000));
                        env.clock.advance(by);
                        now += by.as_nanos() as u64 / 4;
                        store.stats();
                    }
                    _ => {
                        let batch = random_batch(&mut rng, &resources, now, &mut sent, &mut serial);
                        now += rng.below(2 * SEC);
                        store.ingest_logs(batch.clone());
                        model.ingest(&batch);
                    }
                }
                assert_agrees(store, &model, &mut rng, &context);
            }
            assert!(
                !model.held.is_empty(),
                "{context}: the run ended with something held"
            );
            assert!(
                store.stats().logs_received > cap as u64,
                "{context}: the cap was reached"
            );
        }
    }
}

// ---- stats and concurrency ---------------------------------------------------------------------------

#[test]
fn readers_and_a_writer_share_chunks() {
    let dir = TempDir::new().unwrap();
    let (store, _clock) = store_with_config(StoreConfig {
        log_chunk_max: 200,
        ..StoreConfig::spilling_for_tests(dir.path().to_path_buf())
    });
    let writer = {
        let store = store.clone();
        std::thread::spawn(move || {
            let logs = generate(6_000, 111, MS, 50 * MS);
            for batch in logs.chunks(100) {
                store.ingest_logs(batch.to_vec());
            }
        })
    };
    let readers: Vec<_> = (0..3)
        .map(|n| {
            let store = store.clone();
            std::thread::spawn(move || {
                let f = LogFilter {
                    levels: Some(LevelSet::from_groups(true, true, true, true)),
                    ..LogFilter::default()
                };
                for _ in 0..60 {
                    let count = store.log_count(&f).unwrap();
                    let rows = store.logs_at(&f, 0, 20);
                    // A reader sees the store as of one moment: the rows are
                    // in order, whatever else arrived.
                    assert!(rows.windows(2).all(|pair| pair[0].0 > pair[1].0));
                    assert!(rows.len() as u64 <= count.max(20));
                    store.log_histogram(&f, BASE, BASE + 10 * SEC, 20 + n);
                    store.search_logs(&f, &page(30, None));
                }
            })
        })
        .collect();
    writer.join().unwrap();
    for reader in readers {
        reader.join().unwrap();
    }
    assert_eq!(store.stats().logs, 6_000);
    assert_eq!(store.log_count(&LogFilter::default()), Some(6_000));
}

#[test]
fn stats_count_logs_in_every_chunk() {
    for env in envs(|config| StoreConfig {
        log_chunk_max: 10,
        ..config
    }) {
        let name = env.name;
        let store = &env.store;
        store.ingest_logs(generate(35, 121, MS, 0));
        let info = store.log_chunks();
        assert_eq!((info.open, info.sealed), (5, vec![10, 10, 10]), "{name}");
        let stats = store.stats();
        assert_eq!((stats.logs, stats.logs_received), (35, 35), "{name}");
    }
}
