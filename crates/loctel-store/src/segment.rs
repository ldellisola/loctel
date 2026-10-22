//! Segment files: where traces and logs go when they leave memory.
//!
//! A segment is an append-only file of **records**, and a record is
//!
//! ```text
//! [u32 len][u32 crc32 of the compressed bytes][zstd(postcard(payload))]
//! ```
//!
//! with both numbers little-endian and `len` the length of the compressed
//! bytes. There is no file header and no footer: record 0 starts at byte 0, the
//! next one starts where the last ended, and the size of the file is the sum of
//! its records. A [`Location`] (segment number, offset, length) finds one
//! record again.
//!
//! The store keeps a trace as one record (an encoded [`Span`] list, see
//! [`encode_trace`]) and a sealed chunk of logs as one record
//! ([`encode_logs`]). Both intern the resources: a service's resource is
//! written once per record and every span or log points at it, which is what
//! lets 100,000 spans of a few services compress to a fraction of their size.
//!
//! A [`Writer`] appends to one segment, a [`Reader`] maps a finished one and
//! reads it back. Damage is reported, never fatal: a record whose checksum does
//! not match, or that stops short of the end of the file (a crash in the middle
//! of a write), is an error value for that record only, and the others stay
//! readable ([`Reader::iter`] says so and goes on). The first problem in a
//! segment is also printed once on stderr.
//!
//! Segment files are temporary. They live in the run's directory
//! ([`RunDir`](crate::run_dir::RunDir)), are deleted when the run ends, and are
//! never read by another version, so the format has no version number: change
//! it freely, as long as writer and reader agree (decision 0044).
//!
//! This module is public only so that tests can reach it; callers use
//! [`Store`](crate::Store).

use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use loctel_model::{
    AttrValue, LogRecord, Resource, Severity, Span, SpanEvent, SpanId, SpanKind, StatusCode,
    TraceId,
};
use memmap2::Mmap;
use serde::{Deserialize, Serialize};
use zstd::bulk::Compressor;

use crate::logs::LogId;

/// The zstd level every record is compressed at. Level 1 is the fast one: the
/// store compresses on the way to disk while it is still taking traffic.
pub const ZSTD_LEVEL: i32 = 1;

/// Bytes before a record's compressed body: its length and its checksum.
pub const HEADER_BYTES: u64 = 8;

/// The most a record may claim to decompress to. A body that passes its
/// checksum always came from [`Writer::append`], so this only stops a damaged
/// size from turning into a huge allocation.
const MAX_PAYLOAD_BYTES: u64 = 1 << 32;

/// What a segment holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    /// Whole traces, one record each.
    Traces,
    /// Sealed chunks of logs, one record each.
    Logs,
}

impl Kind {
    fn prefix(self) -> &'static str {
        match self {
            Kind::Traces => "traces",
            Kind::Logs => "logs",
        }
    }
}

/// The file name of segment `seq`: `traces-000007.seg` or `logs-000007.seg`
/// (the number is at least six digits wide).
pub fn file_name(kind: Kind, seq: u64) -> String {
    format!("{}-{seq:06}.seg", kind.prefix())
}

/// The inverse of [`file_name`].
fn parse_file_name(name: &str) -> Option<(Kind, u64)> {
    let stem = name.strip_suffix(".seg")?;
    let (prefix, digits) = stem.split_once('-')?;
    let kind = [Kind::Traces, Kind::Logs]
        .into_iter()
        .find(|kind| kind.prefix() == prefix)?;
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    Some((kind, digits.parse().ok()?))
}

/// Where one record is: which segment, and where in it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Location {
    /// The segment's number (the `seq` it was created with).
    pub seq: u64,
    /// The byte the record starts at (its header).
    pub offset: u64,
    /// The length of the record's compressed body, as its header says. The
    /// record takes [`HEADER_BYTES`] more than this on disk.
    pub len: u32,
}

impl Location {
    /// What the record takes on disk, header included.
    pub fn disk_bytes(&self) -> u64 {
        HEADER_BYTES + u64::from(self.len)
    }
}

/// Why a record could not be read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SegmentError {
    /// The file ends before the record does (or before its header does), which
    /// is what a crash in the middle of a write leaves.
    Truncated,
    /// The record's bytes do not match its checksum.
    Checksum,
    /// The record's bytes are intact but are not a record this version wrote:
    /// not zstd, not a blob, or not the record the [`Location`] describes.
    Decode,
}

impl fmt::Display for SegmentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            SegmentError::Truncated => "the record is cut off",
            SegmentError::Checksum => "the record does not match its checksum",
            SegmentError::Decode => "the record cannot be decoded",
        })
    }
}

impl std::error::Error for SegmentError {}

thread_local! {
    /// One zstd compression context per thread, reused by every [`Writer`]:
    /// making one costs more than compressing a small trace.
    static COMPRESSOR: RefCell<Option<Compressor<'static>>> = const { RefCell::new(None) };
}

fn compress(payload: &[u8]) -> io::Result<Vec<u8>> {
    COMPRESSOR.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot.is_none() {
            *slot = Some(Compressor::new(ZSTD_LEVEL)?);
        }
        match slot.as_mut() {
            Some(compressor) => compressor.compress(payload),
            None => Err(io::Error::other("no compression context")),
        }
    })
}

fn decompress(body: &[u8]) -> Result<Vec<u8>, SegmentError> {
    // `Writer` records the size of the payload in the frame, so the output can
    // be allocated once.
    match zstd::zstd_safe::get_frame_content_size(body) {
        Ok(Some(size)) if size <= MAX_PAYLOAD_BYTES => {
            let size = usize::try_from(size).map_err(|_| SegmentError::Decode)?;
            // A frame of nothing: `bulk::decompress` does not take capacity 0.
            if size == 0 {
                return zstd::stream::decode_all(body).map_err(|_| SegmentError::Decode);
            }
            zstd::bulk::decompress(body, size).map_err(|_| SegmentError::Decode)
        }
        Ok(Some(_)) | Err(_) => Err(SegmentError::Decode),
        Ok(None) => zstd::stream::decode_all(body).map_err(|_| SegmentError::Decode),
    }
}

/// Appends records to one segment file.
///
/// Every [`append`](Self::append) is a single write of a whole record, so the
/// file always ends on a record boundary, except in the middle of a write that
/// fails or a crash. A write that fails is taken back (the file is cut to the
/// last good record), and if even that fails the writer refuses further
/// appends instead of writing records nobody could find.
#[derive(Debug)]
pub struct Writer {
    file: File,
    path: PathBuf,
    kind: Kind,
    seq: u64,
    /// The size of the file: where the next record goes.
    bytes: u64,
    failed: bool,
}

impl Writer {
    /// Creates segment `seq` of `kind` in `dir` (which must exist). The file is
    /// `traces-{seq:06}.seg` or `logs-{seq:06}.seg`. A segment that already
    /// exists is an `AlreadyExists` error and is left alone.
    pub fn create(dir: &Path, kind: Kind, seq: u64) -> io::Result<Writer> {
        let path = dir.join(file_name(kind, seq));
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
        Ok(Writer {
            file,
            path,
            kind,
            seq,
            bytes: 0,
            failed: false,
        })
    }

    /// Compresses `payload` and appends it as one record.
    pub fn append(&mut self, payload: &[u8]) -> io::Result<Location> {
        if self.failed {
            return Err(io::Error::other(
                "the segment cannot be written to after an earlier write to it failed",
            ));
        }
        let body = compress(payload)?;
        let len = u32::try_from(body.len())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "a record over 4 GiB"))?;
        let mut record = Vec::with_capacity(body.len() + HEADER_BYTES as usize);
        record.extend_from_slice(&len.to_le_bytes());
        record.extend_from_slice(&crc32fast::hash(&body).to_le_bytes());
        record.extend_from_slice(&body);
        if let Err(error) = self.file.write_all_at(&record, self.bytes) {
            // Take the half-written record off again, so the file ends on a
            // record boundary and the next append lands where the Location
            // says.
            if self.file.set_len(self.bytes).is_err() {
                self.failed = true;
            }
            return Err(error);
        }
        let location = Location {
            seq: self.seq,
            offset: self.bytes,
            len,
        };
        self.bytes += record.len() as u64;
        Ok(location)
    }

    /// The size of the file so far: what this segment costs on disk.
    pub fn bytes(&self) -> u64 {
        self.bytes
    }

    /// The segment's number.
    pub fn seq(&self) -> u64 {
        self.seq
    }

    /// What the segment holds.
    pub fn kind(&self) -> Kind {
        self.kind
    }

    /// The file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Ends the segment: nothing can be appended after this. Every append
    /// already wrote its record out, so there is nothing left to flush, and
    /// nothing is synced to disk (segments never outlive the run, so a crash
    /// does not need them to be durable).
    pub fn seal(mut self) -> io::Result<()> {
        self.file.flush()
    }
}

/// Reads a segment file back through a memory map.
///
/// Open a segment that is sealed (or that no longer changes): the map covers
/// the file as it was when it was opened.
#[derive(Debug)]
pub struct Reader {
    /// `None` for an empty file, which cannot be mapped.
    map: Option<Mmap>,
    path: PathBuf,
    kind: Kind,
    seq: u64,
    problems: AtomicU64,
    reported: AtomicBool,
}

/// A record that read fine: the length of its compressed body, and its payload.
type Decoded = (u32, Vec<u8>);

/// A record's place in the map, before its checksum is looked at.
struct Frame<'a> {
    body: &'a [u8],
    stored_crc: u32,
    /// Where the next record starts.
    next: u64,
}

impl Reader {
    /// Maps the segment at `path`. Its name must be a segment's
    /// (`traces-000007.seg`, see [`file_name`]), which tells the reader what it
    /// holds and what its number is.
    pub fn open(path: impl AsRef<Path>) -> io::Result<Reader> {
        let path = path.as_ref();
        let (kind, seq) = path
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(parse_file_name)
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("{} is not named like a segment file", path.display()),
                )
            })?;
        let file = File::open(path)?;
        let map = if file.metadata()?.len() == 0 {
            None
        } else {
            // SAFETY: the file belongs to this run's private directory, and
            // only the `Writer` that made it ever changes it. A file that
            // somebody else cut short while it is mapped would make a read
            // fault, which is the price of reading without copying.
            Some(unsafe { Mmap::map(&file)? })
        };
        Ok(Reader {
            map,
            path: path.to_path_buf(),
            kind,
            seq,
            problems: AtomicU64::new(0),
            reported: AtomicBool::new(false),
        })
    }

    /// What the segment holds.
    pub fn kind(&self) -> Kind {
        self.kind
    }

    /// The segment's number.
    pub fn seq(&self) -> u64 {
        self.seq
    }

    /// How many times a record failed to read through this reader.
    pub fn problems(&self) -> u64 {
        self.problems.load(Ordering::Relaxed)
    }

    fn data(&self) -> &[u8] {
        self.map.as_deref().unwrap_or(&[])
    }

    /// The record that starts at `offset`, or `Truncated` when the file ends
    /// before it does.
    fn frame(&self, offset: u64) -> Result<Frame<'_>, SegmentError> {
        let data = self.data();
        let start = usize::try_from(offset).map_err(|_| SegmentError::Truncated)?;
        let body_start = start
            .checked_add(HEADER_BYTES as usize)
            .ok_or(SegmentError::Truncated)?;
        let header = data.get(start..body_start).ok_or(SegmentError::Truncated)?;
        let (len, crc) = header.split_at(4);
        let len = u32::from_le_bytes(len.try_into().map_err(|_| SegmentError::Truncated)?);
        let stored_crc = u32::from_le_bytes(crc.try_into().map_err(|_| SegmentError::Truncated)?);
        let body_end = body_start
            .checked_add(len as usize)
            .ok_or(SegmentError::Truncated)?;
        let body = data
            .get(body_start..body_end)
            .ok_or(SegmentError::Truncated)?;
        Ok(Frame {
            body,
            stored_crc,
            next: body_end as u64,
        })
    }

    /// Counts a problem, and says so on stderr the first time.
    fn note(&self, error: SegmentError, offset: u64) {
        self.problems.fetch_add(1, Ordering::Relaxed);
        if !self.reported.swap(true, Ordering::Relaxed) {
            eprintln!(
                "loctel: {}: the record at byte {offset}: {error}; skipping it \
                 (this is said once per segment)",
                self.path.display()
            );
        }
    }

    /// The record at `offset`, checked and decompressed, with where the next
    /// one starts (when that is known).
    fn record(&self, offset: u64) -> (Result<Decoded, SegmentError>, Option<u64>) {
        let frame = match self.frame(offset) {
            Ok(frame) => frame,
            Err(error) => return (Err(error), None),
        };
        let next = Some(frame.next);
        if crc32fast::hash(frame.body) != frame.stored_crc {
            return (Err(SegmentError::Checksum), next);
        }
        let len = frame.body.len() as u32;
        (decompress(frame.body).map(|payload| (len, payload)), next)
    }

    /// The payload of the record at `loc`.
    pub fn read(&self, loc: Location) -> Result<Vec<u8>, SegmentError> {
        let result = match self.record(loc.offset).0 {
            // The header at that offset is not the one the location was made
            // from: the caller holds a location for another file or another
            // version of this one.
            Ok((len, _)) if len != loc.len => Err(SegmentError::Decode),
            Ok((_, payload)) => Ok(payload),
            Err(error) => Err(error),
        };
        if let Err(error) = result {
            self.note(error, loc.offset);
        }
        result
    }

    /// Every record in the file, in order, with where it is. A record that
    /// fails its checksum or does not decode is an `Err` item and the walk goes
    /// on with the next one; a cut-off record is an `Err(Truncated)` item and
    /// ends the walk, since nothing can follow it.
    pub fn iter(&self) -> impl Iterator<Item = Result<(Location, Vec<u8>), SegmentError>> + '_ {
        let mut offset = 0u64;
        let mut finished = false;
        std::iter::from_fn(move || {
            if finished || offset >= self.data().len() as u64 {
                return None;
            }
            let at = offset;
            let (result, next) = self.record(at);
            match next {
                Some(next) => offset = next,
                None => finished = true,
            }
            if let Err(error) = result {
                self.note(error, at);
            }
            Some(result.map(|(len, payload)| {
                let location = Location {
                    seq: self.seq,
                    offset: at,
                    len,
                };
                (location, payload)
            }))
        })
    }
}

// ---- blobs ------------------------------------------------------------------

/// Hands out the index of each distinct resource of a blob.
#[derive(Default)]
struct Interner {
    /// Resources already seen by address: spans from one request share one
    /// `Arc`, so most lookups end here.
    by_pointer: HashMap<usize, u32>,
    /// Resources by their encoded bytes, for equal resources behind different
    /// `Arc`s (the same service, in another batch).
    by_value: HashMap<Vec<u8>, u32>,
    table: Vec<Resource>,
}

impl Interner {
    fn intern(&mut self, resource: &Arc<Resource>) -> u32 {
        let pointer = Arc::as_ptr(resource) as usize;
        if let Some(&index) = self.by_pointer.get(&pointer) {
            return index;
        }
        let key = encode(&**resource);
        let next = self.table.len() as u32;
        let index = *self.by_value.entry(key).or_insert_with(|| {
            self.table.push(Resource::clone(resource));
            next
        });
        self.by_pointer.insert(pointer, index);
        index
    }
}

/// The resources of a decoded blob, shared again: spans that had one resource
/// come back holding one `Arc`.
fn shared(table: Vec<Resource>) -> Vec<Arc<Resource>> {
    table.into_iter().map(Arc::new).collect()
}

fn resource_at(table: &[Arc<Resource>], index: u32) -> Result<Arc<Resource>, SegmentError> {
    table
        .get(index as usize)
        .cloned()
        .ok_or(SegmentError::Decode)
}

fn encode<T: Serialize + ?Sized>(value: &T) -> Vec<u8> {
    // The model types serialize to postcard without a length that is unknown
    // up front or a type it cannot write, and a `Vec` has no limit to hit.
    postcard::to_stdvec(value).expect("postcard serializes the model types")
}

/// Decodes a whole blob: bytes left over mean it is not the blob that was
/// asked for.
fn decode<'a, T: Deserialize<'a>>(bytes: &'a [u8]) -> Result<T, SegmentError> {
    match postcard::take_from_bytes::<T>(bytes) {
        Ok((value, [])) => Ok(value),
        _ => Err(SegmentError::Decode),
    }
}

/// One span of a trace blob: a [`Span`] whose resource is an index into the
/// blob's table. Borrows what it can when encoding, and owns it when decoded.
#[derive(Serialize, Deserialize)]
struct WireSpan<'a> {
    trace_id: TraceId,
    span_id: SpanId,
    parent_span_id: Option<SpanId>,
    name: Cow<'a, str>,
    kind: SpanKind,
    start_unix_nano: u64,
    end_unix_nano: u64,
    status: StatusCode,
    status_message: Cow<'a, str>,
    attributes: Cow<'a, [(String, AttrValue)]>,
    events: Cow<'a, [SpanEvent]>,
    resource: u32,
    scope_name: Cow<'a, str>,
}

#[derive(Serialize, Deserialize)]
struct TraceBlob<'a> {
    resources: Vec<Resource>,
    spans: Vec<WireSpan<'a>>,
}

/// The bytes of one trace's spans, to give to [`Writer::append`]. Spans keep
/// their order, and each distinct resource is written once.
pub fn encode_trace(spans: &[Span]) -> Vec<u8> {
    let mut resources = Interner::default();
    let spans = spans
        .iter()
        .map(|span| WireSpan {
            trace_id: span.trace_id,
            span_id: span.span_id,
            parent_span_id: span.parent_span_id,
            name: Cow::Borrowed(&span.name),
            kind: span.kind,
            start_unix_nano: span.start_unix_nano,
            end_unix_nano: span.end_unix_nano,
            status: span.status,
            status_message: Cow::Borrowed(&span.status_message),
            attributes: Cow::Borrowed(&span.attributes),
            events: Cow::Borrowed(&span.events),
            resource: resources.intern(&span.resource),
            scope_name: Cow::Borrowed(&span.scope_name),
        })
        .collect();
    encode(&TraceBlob {
        resources: resources.table,
        spans,
    })
}

/// The spans [`encode_trace`] was given, equal and in the same order. Spans of
/// one resource share one `Arc` again.
pub fn decode_trace(bytes: &[u8]) -> Result<Vec<Span>, SegmentError> {
    let blob: TraceBlob<'_> = decode(bytes)?;
    let table = shared(blob.resources);
    blob.spans
        .into_iter()
        .map(|span| {
            Ok(Span {
                trace_id: span.trace_id,
                span_id: span.span_id,
                parent_span_id: span.parent_span_id,
                name: span.name.into_owned(),
                kind: span.kind,
                start_unix_nano: span.start_unix_nano,
                end_unix_nano: span.end_unix_nano,
                status: span.status,
                status_message: span.status_message.into_owned(),
                attributes: span.attributes.into_owned(),
                events: span.events.into_owned(),
                resource: resource_at(&table, span.resource)?,
                scope_name: span.scope_name.into_owned(),
            })
        })
        .collect()
}

/// A log record and the [`LogId`] it is held under, which is what a chunk of
/// logs is made of.
#[derive(Clone, Debug, PartialEq)]
pub struct LogRecordWithId {
    pub id: LogId,
    pub record: LogRecord,
}

/// One log of a chunk blob: like [`WireSpan`], with the id spelled out.
#[derive(Serialize, Deserialize)]
struct WireLog<'a> {
    id_time_unix_nano: u64,
    id_seq: u64,
    time_unix_nano: u64,
    observed_unix_nano: u64,
    severity: Severity,
    severity_text: Cow<'a, str>,
    body: Cow<'a, str>,
    attributes: Cow<'a, [(String, AttrValue)]>,
    trace_id: Option<TraceId>,
    span_id: Option<SpanId>,
    resource: u32,
    scope_name: Cow<'a, str>,
}

#[derive(Serialize, Deserialize)]
struct LogBlob<'a> {
    resources: Vec<Resource>,
    records: Vec<WireLog<'a>>,
}

/// The bytes of a chunk of logs, to give to [`Writer::append`]. The records keep
/// their order and their ids, and each distinct resource is written once.
pub fn encode_logs(records: &[LogRecordWithId]) -> Vec<u8> {
    let mut resources = Interner::default();
    let records = records
        .iter()
        .map(|entry| WireLog {
            id_time_unix_nano: entry.id.time_unix_nano,
            id_seq: entry.id.seq,
            time_unix_nano: entry.record.time_unix_nano,
            observed_unix_nano: entry.record.observed_unix_nano,
            severity: entry.record.severity,
            severity_text: Cow::Borrowed(&entry.record.severity_text),
            body: Cow::Borrowed(&entry.record.body),
            attributes: Cow::Borrowed(&entry.record.attributes),
            trace_id: entry.record.trace_id,
            span_id: entry.record.span_id,
            resource: resources.intern(&entry.record.resource),
            scope_name: Cow::Borrowed(&entry.record.scope_name),
        })
        .collect();
    encode(&LogBlob {
        resources: resources.table,
        records,
    })
}

/// The records [`encode_logs`] was given, equal and in the same order.
pub fn decode_logs(bytes: &[u8]) -> Result<Vec<LogRecordWithId>, SegmentError> {
    let blob: LogBlob<'_> = decode(bytes)?;
    let table = shared(blob.resources);
    blob.records
        .into_iter()
        .map(|log| {
            Ok(LogRecordWithId {
                id: LogId {
                    time_unix_nano: log.id_time_unix_nano,
                    seq: log.id_seq,
                },
                record: LogRecord {
                    time_unix_nano: log.time_unix_nano,
                    observed_unix_nano: log.observed_unix_nano,
                    severity: log.severity,
                    severity_text: log.severity_text.into_owned(),
                    body: log.body.into_owned(),
                    attributes: log.attributes.into_owned(),
                    trace_id: log.trace_id,
                    span_id: log.span_id,
                    resource: resource_at(&table, log.resource)?,
                    scope_name: log.scope_name.into_owned(),
                },
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `decompress` allocates once, from the size the frame states, so every
    /// record the writer makes must state its size (empty ones too).
    #[test]
    fn compressed_records_state_the_size_of_their_payload() {
        for size in [0usize, 1, 1000, 1 << 20] {
            let payload = vec![7u8; size];
            let body = compress(&payload).unwrap();
            let stated = zstd::zstd_safe::get_frame_content_size(&body);
            assert!(
                matches!(stated, Ok(Some(stated)) if stated == size as u64),
                "{size} bytes"
            );
            assert_eq!(decompress(&body).unwrap(), payload);
        }
    }
}
