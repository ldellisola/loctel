//! Segment files and the run directory (Milestone 5, Task 1).
//!
//! A segment is a file of records, `[u32 len][u32 crc32][zstd(payload)]`, that
//! the store appends to and reads back by position. These tests pin the record
//! format, what a damaged file does (a bad checksum or a cut-off record is
//! reported and skipped, never a panic: Review Focus 2) and the run directory
//! that holds the segments of one run.

mod fixtures;

use std::collections::HashSet;
use std::fs::{self, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use fixtures::{scenario_logs, scenario_spans};
use loctel_meshgen::Scenario;
use loctel_model::{AttrValue, LogRecord, Resource, Span};
use loctel_store::LogId;
use loctel_store::run_dir::RunDir;
use loctel_store::segment::{
    HEADER_BYTES, Kind, Location, LogRecordWithId, Reader, SegmentError, Writer, decode_logs,
    decode_trace, encode_logs, encode_trace,
};
use tempfile::TempDir;

const BASE: u64 = 1_700_000_000_000_000_000;

/// `n` bytes that zstd cannot shrink (a fixed xorshift stream), so a payload's
/// stored size stays close to its size.
fn noise(n: usize, seed: u64) -> Vec<u8> {
    let mut state = seed | 1;
    (0..n)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state >> 24) as u8
        })
        .collect()
}

fn count(haystack: &[u8], needle: &[u8]) -> usize {
    haystack
        .windows(needle.len())
        .filter(|window| *window == needle)
        .count()
}

/// A traces segment 7 holding `payloads`, sealed. Returns its path and where
/// each payload went.
fn write_segment(dir: &Path, payloads: &[Vec<u8>]) -> (PathBuf, Vec<Location>) {
    let mut writer = Writer::create(dir, Kind::Traces, 7).unwrap();
    let locations = payloads
        .iter()
        .map(|payload| writer.append(payload).unwrap())
        .collect();
    let path = writer.path().to_path_buf();
    writer.seal().unwrap();
    (path, locations)
}

fn three_payloads() -> Vec<Vec<u8>> {
    vec![noise(300, 1), noise(5_000, 2), noise(40, 3)]
}

// ---- the file format --------------------------------------------------------

#[test]
fn segment_append_and_read_back() {
    let dir = TempDir::new().unwrap();
    let payloads = vec![
        noise(1_000, 1),
        Vec::new(),
        vec![0u8; 1 << 20],
        noise(1, 2),
        b"hello segment".to_vec(),
    ];

    let mut writer = Writer::create(dir.path(), Kind::Traces, 3).unwrap();
    assert_eq!(writer.bytes(), 0);
    let path = writer.path().to_path_buf();
    assert_eq!(path, dir.path().join("traces-000003.seg"));

    let mut locations = Vec::new();
    let mut expected_offset = 0;
    for payload in &payloads {
        let location = writer.append(payload).unwrap();
        // Records follow each other from the first byte: header, then body.
        assert_eq!(location.seq, 3);
        assert_eq!(location.offset, expected_offset);
        expected_offset += HEADER_BYTES + u64::from(location.len);
        assert_eq!(writer.bytes(), expected_offset);
        locations.push(location);
    }
    // `bytes` is the size of the file, and compression took the zeros down.
    assert_eq!(fs::metadata(&path).unwrap().len(), writer.bytes());
    assert!(
        locations[2].len < 1_000,
        "1 MiB of zeros is stored in {} bytes",
        locations[2].len
    );

    // A reader opened before the segment is sealed already sees every record
    // appended so far: each append is a single write.
    let early = Reader::open(&path).unwrap();
    assert_eq!(early.read(locations[4]).unwrap(), payloads[4]);
    writer.seal().unwrap();

    let reader = Reader::open(&path).unwrap();
    assert_eq!((reader.kind(), reader.seq()), (Kind::Traces, 3));
    for (location, payload) in locations.iter().zip(&payloads) {
        assert_eq!(&reader.read(*location).unwrap(), payload);
    }
    // Reads in any order, and again.
    assert_eq!(reader.read(locations[3]).unwrap(), payloads[3]);
    assert_eq!(reader.read(locations[0]).unwrap(), payloads[0]);

    // `iter` walks the file in order and says where each record is.
    let walked: Vec<(Location, Vec<u8>)> = reader.iter().map(Result::unwrap).collect();
    let expected: Vec<(Location, Vec<u8>)> = locations.into_iter().zip(payloads).collect();
    assert_eq!(walked, expected);
}

#[test]
fn segment_files_are_named_by_kind_and_sequence_and_never_overwritten() {
    let dir = TempDir::new().unwrap();
    for (kind, seq, name) in [
        (Kind::Traces, 0, "traces-000000.seg"),
        (Kind::Logs, 7, "logs-000007.seg"),
        (Kind::Logs, 1_234_567, "logs-1234567.seg"),
    ] {
        let writer = Writer::create(dir.path(), kind, seq).unwrap();
        assert_eq!(writer.path(), dir.path().join(name));
        writer.seal().unwrap();
        let reader = Reader::open(dir.path().join(name)).unwrap();
        assert_eq!((reader.kind(), reader.seq()), (kind, seq));
    }
    // A sequence number is used once: creating it again fails, and the first
    // file is untouched.
    let mut first = Writer::create(dir.path(), Kind::Traces, 9).unwrap();
    let location = first.append(b"kept").unwrap();
    let again = Writer::create(dir.path(), Kind::Traces, 9);
    assert_eq!(
        again.err().map(|e| e.kind()),
        Some(ErrorKind::AlreadyExists)
    );
    first.seal().unwrap();
    let reader = Reader::open(dir.path().join("traces-000009.seg")).unwrap();
    assert_eq!(reader.read(location).unwrap(), b"kept");

    // A file that is not named like a segment is not opened as one.
    let stray = dir.path().join("notes.seg");
    fs::write(&stray, b"x").unwrap();
    assert_eq!(
        Reader::open(&stray).err().map(|e| e.kind()),
        Some(ErrorKind::InvalidInput)
    );
    // A missing file is the plain not-found error.
    assert_eq!(
        Reader::open(dir.path().join("traces-000042.seg"))
            .err()
            .map(|e| e.kind()),
        Some(ErrorKind::NotFound)
    );
}

#[test]
fn trace_blob_roundtrip_interns_resources() {
    let spans = scenario_spans(&Scenario::CheckoutFailure, 1, BASE);
    assert_eq!(spans.len(), 15);
    let services: HashSet<&str> = spans.iter().map(Span::service).collect();
    assert!(services.len() > 1 && services.len() < spans.len());

    let blob = encode_trace(&spans);
    // Every resource carries `deployment.environment`, and spans do not: the
    // key shows up once per service, not once per span.
    assert_eq!(count(&blob, b"deployment.environment"), services.len());
    assert_eq!(count(&blob, b"service.version"), services.len());

    let back = decode_trace(&blob).unwrap();
    assert_eq!(back, spans);
    // Back in memory the spans of one service share one `Arc` again.
    for a in &back {
        for b in &back {
            if a.service() == b.service() {
                assert!(Arc::ptr_eq(&a.resource, &b.resource), "{}", a.service());
            }
        }
    }

    // Through a segment file and back.
    let dir = TempDir::new().unwrap();
    let (path, locations) = write_segment(dir.path(), std::slice::from_ref(&blob));
    let stored = Reader::open(path).unwrap().read(locations[0]).unwrap();
    assert_eq!(decode_trace(&stored).unwrap(), spans);
}

#[test]
fn trace_blob_interns_equal_resources_by_value() {
    // The same service reached the store in four batches, so its spans hold
    // four separate `Arc`s that are equal. One of them differs by an
    // attribute and is a resource of its own.
    let mut spans = scenario_spans(&Scenario::CheckoutFailure, 1, BASE);
    spans.truncate(4);
    let template = spans[0].resource.clone();
    for span in &mut spans {
        span.resource = Arc::new(Resource::clone(&template));
    }
    let mut other = Resource::clone(&template);
    other.attributes.push((
        "host.name".to_string(),
        AttrValue::Str("laptop".to_string()),
    ));
    spans[3].resource = Arc::new(other);

    let blob = encode_trace(&spans);
    assert_eq!(count(&blob, b"deployment.environment"), 2);
    assert_eq!(count(&blob, b"host.name"), 1);
    let back = decode_trace(&blob).unwrap();
    assert_eq!(back, spans);
    assert!(Arc::ptr_eq(&back[0].resource, &back[1].resource));
    assert!(Arc::ptr_eq(&back[0].resource, &back[2].resource));
    assert!(!Arc::ptr_eq(&back[0].resource, &back[3].resource));
}

#[test]
fn an_empty_trace_blob_roundtrips() {
    assert_eq!(
        decode_trace(&encode_trace(&[])).unwrap(),
        Vec::<Span>::new()
    );
    assert_eq!(
        decode_logs(&encode_logs(&[])).unwrap(),
        Vec::<LogRecordWithId>::new()
    );
}

fn with_ids(records: Vec<LogRecord>) -> Vec<LogRecordWithId> {
    records
        .into_iter()
        .enumerate()
        .map(|(n, record)| LogRecordWithId {
            id: LogId {
                time_unix_nano: record.time_unix_nano,
                seq: 100 + n as u64,
            },
            record,
        })
        .collect()
}

#[test]
fn log_chunk_roundtrip() {
    let records = with_ids(scenario_logs(&Scenario::CheckoutFailure, 1, BASE));
    assert_eq!(records.len(), 7);
    let services: HashSet<&str> = records.iter().map(|r| r.record.service()).collect();
    assert!(services.len() > 1 && services.len() < records.len());

    let blob = encode_logs(&records);
    assert_eq!(count(&blob, b"deployment.environment"), services.len());
    let back = decode_logs(&blob).unwrap();
    assert_eq!(back, records);
    // The ids come back as they went in, including a time that differs from
    // the record's own (a log with no timestamp is held under its observed
    // time).
    let mut odd = records.clone();
    odd[0].id.time_unix_nano += 5;
    odd[1].record.trace_id = None;
    odd[2].record.span_id = None;
    assert_eq!(decode_logs(&encode_logs(&odd)).unwrap(), odd);

    // A big chunk of the log flood, through a segment file.
    let flood = with_ids(scenario_logs(
        &Scenario::LogFlood { records: 2_000 },
        3,
        BASE,
    ));
    assert_eq!(flood.len(), 2_000);
    let blob = encode_logs(&flood);
    let dir = TempDir::new().unwrap();
    let mut writer = Writer::create(dir.path(), Kind::Logs, 1).unwrap();
    let location = writer.append(&blob).unwrap();
    writer.seal().unwrap();
    let reader = Reader::open(dir.path().join("logs-000001.seg")).unwrap();
    assert_eq!(reader.kind(), Kind::Logs);
    assert_eq!(decode_logs(&reader.read(location).unwrap()).unwrap(), flood);
}

#[test]
fn blobs_that_are_not_blobs_are_decode_errors() {
    let spans = scenario_spans(&Scenario::CheckoutFailure, 1, BASE);
    let blob = encode_trace(&spans);
    assert_eq!(decode_trace(&[]), Err(SegmentError::Decode));
    assert_eq!(
        decode_trace(&blob[..blob.len() / 2]),
        Err(SegmentError::Decode)
    );
    assert_eq!(decode_trace(&[0xff; 64]), Err(SegmentError::Decode));
    let mut padded = blob.clone();
    padded.push(0);
    assert_eq!(decode_trace(&padded), Err(SegmentError::Decode));
    assert_eq!(decode_logs(&blob), Err(SegmentError::Decode));
    let logs = encode_logs(&with_ids(scenario_logs(
        &Scenario::CheckoutFailure,
        1,
        BASE,
    )));
    assert_eq!(
        decode_logs(&logs[..logs.len() - 1]),
        Err(SegmentError::Decode)
    );
}

// ---- damaged files ----------------------------------------------------------

/// Flips one bit of the file at `at`.
fn flip(path: &Path, at: u64) {
    let mut bytes = fs::read(path).unwrap();
    bytes[at as usize] ^= 0x01;
    fs::write(path, bytes).unwrap();
}

#[test]
fn checksum_mismatch_is_reported() {
    let dir = TempDir::new().unwrap();
    let payloads = three_payloads();
    let (path, locations) = write_segment(dir.path(), &payloads);

    // A flipped bit in the middle of the second record's body.
    flip(&path, locations[1].offset + HEADER_BYTES + 100);
    let reader = Reader::open(&path).unwrap();
    assert_eq!(reader.read(locations[1]), Err(SegmentError::Checksum));
    // The records either side are fine, one at a time and in a walk, which
    // reports the bad one and carries on with the next.
    assert_eq!(reader.read(locations[0]).unwrap(), payloads[0]);
    assert_eq!(reader.read(locations[2]).unwrap(), payloads[2]);
    let walked: Vec<_> = reader.iter().collect();
    assert_eq!(walked.len(), 3);
    assert_eq!(walked[0].as_ref().unwrap().1, payloads[0]);
    assert_eq!(walked[1], Err(SegmentError::Checksum));
    assert_eq!(walked[2].as_ref().unwrap().1, payloads[2]);

    // A flipped bit in the stored checksum itself is the same finding.
    flip(&path, locations[1].offset + HEADER_BYTES + 100); // back
    flip(&path, locations[0].offset + 4);
    let reader = Reader::open(&path).unwrap();
    assert_eq!(reader.read(locations[0]), Err(SegmentError::Checksum));
    assert_eq!(reader.read(locations[1]).unwrap(), payloads[1]);
}

#[test]
fn a_record_that_is_not_zstd_is_a_decode_error() {
    // A correct checksum over bytes that are not a zstd frame: the record was
    // written by something else, or by a different version.
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("traces-000001.seg");
    let body = b"this is not a zstd frame";
    let mut file = fs::File::create(&path).unwrap();
    file.write_all(&(body.len() as u32).to_le_bytes()).unwrap();
    file.write_all(&crc32fast::hash(body).to_le_bytes())
        .unwrap();
    file.write_all(body).unwrap();
    drop(file);

    let reader = Reader::open(&path).unwrap();
    let location = Location {
        seq: 1,
        offset: 0,
        len: body.len() as u32,
    };
    assert_eq!(reader.read(location), Err(SegmentError::Decode));
    assert_eq!(
        reader.iter().collect::<Vec<_>>(),
        vec![Err(SegmentError::Decode)]
    );
}

#[test]
fn truncated_record_is_skipped() {
    let dir = TempDir::new().unwrap();
    let payloads = three_payloads();
    let (path, locations) = write_segment(dir.path(), &payloads);
    let full = fs::metadata(&path).unwrap().len();

    // A crash mid-write: the last record is cut short, in its body, in its
    // header, or after a single byte of it.
    for cut_to in [
        full - 5,
        locations[2].offset + HEADER_BYTES,
        locations[2].offset + 3,
        locations[2].offset + 1,
    ] {
        OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap()
            .set_len(cut_to)
            .unwrap();
        let reader = Reader::open(&path).unwrap();
        let walked: Vec<_> = reader.iter().collect();
        assert_eq!(walked.len(), 3, "cut to {cut_to}");
        assert_eq!(
            walked[0].as_ref().unwrap(),
            &(locations[0], payloads[0].clone())
        );
        assert_eq!(
            walked[1].as_ref().unwrap(),
            &(locations[1], payloads[1].clone())
        );
        assert_eq!(walked[2], Err(SegmentError::Truncated), "cut to {cut_to}");
        // Reading the cut record directly says the same, and the others
        // still read.
        assert_eq!(reader.read(locations[2]), Err(SegmentError::Truncated));
        assert_eq!(reader.read(locations[0]).unwrap(), payloads[0]);
        assert_eq!(reader.read(locations[1]).unwrap(), payloads[1]);
    }

    // Cut exactly between two records: nothing is wrong, the file is shorter.
    OpenOptions::new()
        .write(true)
        .open(&path)
        .unwrap()
        .set_len(locations[2].offset)
        .unwrap();
    let reader = Reader::open(&path).unwrap();
    assert_eq!(reader.iter().filter(Result::is_ok).count(), 2);
    assert_eq!(reader.iter().filter(Result::is_err).count(), 0);
}

#[test]
fn a_first_record_that_is_cut_off_and_an_empty_file_do_not_panic() {
    let dir = TempDir::new().unwrap();
    // A file that was created and never written to.
    let empty = Writer::create(dir.path(), Kind::Logs, 1).unwrap();
    empty.seal().unwrap();
    let reader = Reader::open(dir.path().join("logs-000001.seg")).unwrap();
    assert_eq!(reader.iter().count(), 0);
    let nowhere = Location {
        seq: 1,
        offset: 0,
        len: 10,
    };
    assert_eq!(reader.read(nowhere), Err(SegmentError::Truncated));

    // Only part of the first record's header.
    fs::write(dir.path().join("logs-000002.seg"), [1u8, 0, 0]).unwrap();
    let reader = Reader::open(dir.path().join("logs-000002.seg")).unwrap();
    assert_eq!(
        reader.iter().collect::<Vec<_>>(),
        vec![Err(SegmentError::Truncated)]
    );

    // A header that promises more bytes than the file has, or more than fits
    // in the address space.
    let mut huge = vec![0xff, 0xff, 0xff, 0xff, 0, 0, 0, 0];
    huge.extend_from_slice(b"short");
    fs::write(dir.path().join("logs-000003.seg"), &huge).unwrap();
    let reader = Reader::open(dir.path().join("logs-000003.seg")).unwrap();
    assert_eq!(
        reader.iter().collect::<Vec<_>>(),
        vec![Err(SegmentError::Truncated)]
    );
    let beyond = Location {
        seq: 3,
        offset: u64::MAX - 3,
        len: 1,
    };
    assert_eq!(reader.read(beyond), Err(SegmentError::Truncated));
}

#[test]
fn a_location_from_another_record_is_refused() {
    let dir = TempDir::new().unwrap();
    let payloads = three_payloads();
    let (path, locations) = write_segment(dir.path(), &payloads);
    let reader = Reader::open(&path).unwrap();
    // Right offset, wrong length: not the record the caller thinks it is.
    let wrong = Location {
        len: locations[0].len + 1,
        ..locations[0]
    };
    assert_eq!(reader.read(wrong), Err(SegmentError::Decode));
}

// ---- the run directory ------------------------------------------------------

/// A pid that no process has: the pid of a child that has finished and been
/// waited for.
fn dead_pid() -> u32 {
    let mut child = std::process::Command::new("true")
        .spawn()
        .expect("`true` runs");
    let pid = child.id();
    child.wait().unwrap();
    pid
}

#[test]
fn run_dir_deletes_stale_siblings_and_itself_on_drop() {
    let base = TempDir::new().unwrap();
    let base = base.path().join("loctel");
    fs::create_dir_all(&base).unwrap();
    let dead = dead_pid();
    let own = std::process::id();
    let parent = std::os::unix::process::parent_id();

    let make = |name: String, file: &str| {
        let dir = base.join(name);
        fs::create_dir(&dir).unwrap();
        fs::write(dir.join(file), b"spilled").unwrap();
        dir
    };
    // Left behind by runs that crashed: gone after the next start. The second
    // has data in nested directories, and 999999 is the plan's own example.
    let stale = make(format!("run-{dead}-1700000000"), "traces-000001.seg");
    let stale_nested = make("run-999999-1".to_string(), "logs-000001.seg");
    fs::create_dir(stale_nested.join("deeper")).unwrap();
    fs::write(stale_nested.join("deeper").join("x"), b"x").unwrap();
    // Runs whose process is alive: this one (another start time) and the one
    // that started this test.
    let alive_own = make(format!("run-{own}-1"), "traces-000001.seg");
    let alive_parent = make(format!("run-{parent}-5"), "traces-000001.seg");
    // Things that are not run directories are left alone, even with a pid that
    // is dead: other names, a pid that is not a number, 0 (never signalled) or
    // too big for a pid, and a file.
    let others = [
        make("scratch".to_string(), "keep"),
        make("run-x-1".to_string(), "keep"),
        make("run--1".to_string(), "keep"),
        make("run-0-1".to_string(), "keep"),
        make("run-99999999999-1".to_string(), "keep"),
        make(format!("run-{dead}"), "keep"),
    ];
    let file = base.join(format!("run-{dead}-2"));
    fs::write(&file, b"a file").unwrap();
    // A symbolic link with a stale name is not followed and not removed.
    let precious = base.join("precious");
    fs::create_dir(&precious).unwrap();
    fs::write(precious.join("keep"), b"keep").unwrap();
    let link = base.join(format!("run-{dead}-3"));
    std::os::unix::fs::symlink(&precious, &link).unwrap();

    let run = RunDir::create(&base).unwrap();

    assert!(!stale.exists(), "a dead pid's directory is removed");
    assert!(!stale_nested.exists(), "with everything in it");
    assert!(alive_own.exists());
    assert!(alive_parent.exists());
    for other in &others {
        assert!(other.exists(), "{} was left alone", other.display());
    }
    assert!(file.exists());
    assert!(precious.join("keep").exists());
    assert!(link.symlink_metadata().is_ok());

    // The run's own directory: under the base, named by pid and start time,
    // empty, and private to the user.
    let path = run.path().to_path_buf();
    assert_eq!(path.parent(), Some(base.as_path()));
    let name = path.file_name().unwrap().to_str().unwrap();
    let parts: Vec<&str> = name.split('-').collect();
    assert_eq!(parts.len(), 3, "{name}");
    assert_eq!(parts[0], "run");
    assert_eq!(parts[1], own.to_string());
    assert!(parts[2].parse::<u64>().unwrap() > 1_700_000_000);
    assert_eq!(fs::read_dir(&path).unwrap().count(), 0);
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700);
    }

    // The segments of the run live in it, and go with it on drop.
    let mut writer = Writer::create(run.path(), Kind::Traces, 0).unwrap();
    writer.append(b"data").unwrap();
    writer.seal().unwrap();
    assert!(path.join("traces-000000.seg").exists());
    drop(run);
    assert!(!path.exists(), "the run's directory is deleted on drop");
    // Nothing else was touched by the drop.
    assert!(alive_own.exists());
    assert!(base.exists());
}

#[test]
fn run_dirs_in_one_process_and_one_second_get_distinct_directories() {
    let base = TempDir::new().unwrap();
    // The base does not exist yet: creating a run directory makes it.
    let base = base.path().join("a").join("loctel");
    let first = RunDir::create(&base).unwrap();
    let second = RunDir::create(&base).unwrap();
    let third = RunDir::create(&base).unwrap();
    let paths: HashSet<PathBuf> = [&first, &second, &third]
        .iter()
        .map(|run| run.path().to_path_buf())
        .collect();
    assert_eq!(paths.len(), 3);
    for run in [&first, &second, &third] {
        assert!(run.path().is_dir());
    }
    // The same process is alive, so creating one run never removes another.
    let path = first.path().to_path_buf();
    drop(second);
    assert!(path.is_dir());
    assert!(third.path().is_dir());
}
