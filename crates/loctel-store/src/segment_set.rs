//! The segment files of one store: which exist, how big each is, how much of
//! each is dead, and the maps that read them back.
//!
//! [`segment`](crate::segment) writes and reads one file. This module is the
//! store's view of all of them:
//!
//! - **Writing.** [`append`](SegmentSet::append) puts a payload at the end of
//!   the segment being written for its [`Kind`], and *rolls* (finishes the
//!   file, so the next record starts a new one) once the file has reached
//!   `segment_max_bytes`. A record is never split, so a file is a little over
//!   that size. Segment numbers count up over every kind and are never reused,
//!   so the order of the numbers is the order of the files' birth: the oldest
//!   file is the lowest number (what the disk cap will delete first).
//! - **Dead bytes.** A record whose data went somewhere else (a trace that came
//!   back to memory, or was removed or evicted) cannot be taken out of the
//!   file it is in. [`mark_dead`](SegmentSet::mark_dead) counts it: it still
//!   takes disk until its whole segment is deleted.
//! - **Reading.** A map ([`Reader`]) per file, made the first time a record of
//!   the file is read, and made again when the file has grown past the part it
//!   mapped (a map covers the file as it was when it was made, and a file that
//!   is still being written grows).
//!
//! Every file belongs to the store that made it, which deletes them when it is
//! cleared or dropped.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use parking_lot::Mutex;

use crate::segment::{Kind, Location, Reader, SegmentError, Writer};

/// One segment file.
struct Segment {
    path: PathBuf,
    /// The size of the file: every record in it, dead or alive.
    bytes: u64,
    /// The part of `bytes` that belongs to records nobody refers to any more.
    dead_bytes: u64,
}

/// A segment's map, and how much of the file it covers.
struct Mapped {
    covers: u64,
    reader: Arc<Reader>,
}

pub(crate) struct SegmentSet {
    dir: PathBuf,
    /// The size at which a segment is finished.
    max_bytes: u64,
    /// The number the next segment gets.
    next_seq: u64,
    /// The segment being written, for each kind that has one.
    open: HashMap<Kind, Writer>,
    segments: BTreeMap<u64, Segment>,
    /// Behind a lock because readers take `&self`.
    readers: Mutex<HashMap<u64, Mapped>>,
}

impl SegmentSet {
    /// A set of segments in `dir`, which is made if it is missing.
    pub(crate) fn new(dir: PathBuf, max_bytes: u64) -> io::Result<SegmentSet> {
        fs::create_dir_all(&dir)?;
        Ok(SegmentSet {
            dir,
            max_bytes,
            next_seq: 0,
            open: HashMap::new(),
            segments: BTreeMap::new(),
            readers: Mutex::new(HashMap::new()),
        })
    }

    /// Appends `payload` as a record of a segment of `kind`, and says where it
    /// went. On an error nothing was added: the record is not in the file and
    /// the next append is as good as this one.
    pub(crate) fn append(&mut self, kind: Kind, payload: &[u8]) -> io::Result<Location> {
        if !self.open.contains_key(&kind) {
            let writer = self.create(kind)?;
            self.open.insert(kind, writer);
        }
        let Some(writer) = self.open.get_mut(&kind) else {
            return Err(io::Error::other("no segment is open"));
        };
        let location = writer.append(payload)?;
        let (seq, bytes) = (writer.seq(), writer.bytes());
        if let Some(segment) = self.segments.get_mut(&seq) {
            segment.bytes = bytes;
        }
        if bytes >= self.max_bytes {
            // Finished. Nothing is left to flush, and a failure to say so does
            // not undo a record that was written: the next append opens a new
            // file either way.
            if let Some(writer) = self.open.remove(&kind) {
                let _ = writer.seal();
            }
        }
        Ok(location)
    }

    /// Makes the next segment file. A name that is taken (a stray file) is
    /// skipped, since a number is only ever used once.
    fn create(&mut self, kind: Kind) -> io::Result<Writer> {
        loop {
            let seq = self.next_seq;
            self.next_seq += 1;
            match Writer::create(&self.dir, kind, seq) {
                Ok(writer) => {
                    self.segments.insert(
                        seq,
                        Segment {
                            path: writer.path().to_path_buf(),
                            bytes: 0,
                            dead_bytes: 0,
                        },
                    );
                    return Ok(writer);
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
        }
    }

    /// The payload of the record at `location`.
    pub(crate) fn read(&self, location: Location) -> Result<Vec<u8>, SegmentError> {
        self.reader(location)?.read(location)
    }

    /// A map of the file of `location` that covers the record.
    fn reader(&self, location: Location) -> Result<Arc<Reader>, SegmentError> {
        let segment = self
            .segments
            .get(&location.seq)
            .ok_or(SegmentError::Truncated)?;
        let needed = location.offset.saturating_add(location.disk_bytes());
        let mut readers = self.readers.lock();
        if let Some(mapped) = readers.get(&location.seq)
            && mapped.covers >= needed
        {
            return Ok(Arc::clone(&mapped.reader));
        }
        let reader = Reader::open(&segment.path)
            .map(Arc::new)
            .map_err(|_| SegmentError::Truncated)?;
        readers.insert(
            location.seq,
            Mapped {
                covers: segment.bytes,
                reader: Arc::clone(&reader),
            },
        );
        Ok(reader)
    }

    /// The record at `location` is no longer wanted. Its bytes stay in the file
    /// and count as disk until the file goes.
    pub(crate) fn mark_dead(&mut self, location: Location) {
        if let Some(segment) = self.segments.get_mut(&location.seq) {
            segment.dead_bytes = segment
                .dead_bytes
                .saturating_add(location.disk_bytes())
                .min(segment.bytes);
        }
    }

    /// What the files take on disk, dead records included.
    pub(crate) fn disk_bytes(&self) -> u64 {
        self.segments.values().map(|segment| segment.bytes).sum()
    }

    /// The part of [`disk_bytes`](Self::disk_bytes) that is dead.
    pub(crate) fn dead_bytes(&self) -> u64 {
        self.segments
            .values()
            .map(|segment| segment.dead_bytes)
            .sum()
    }

    /// Deletes every file. Numbers are not reused afterwards.
    pub(crate) fn clear(&mut self) {
        self.open.clear();
        self.readers.get_mut().clear();
        for segment in std::mem::take(&mut self.segments).into_values() {
            remove_file(&segment.path);
        }
    }
}

impl Drop for SegmentSet {
    fn drop(&mut self) {
        self.clear();
    }
}

/// Deletes a file that is ours to delete. One that is already gone is fine (the
/// run directory may have been removed under us), and one that cannot be
/// deleted is left for the run directory's own cleanup.
fn remove_file(path: &Path) {
    let _ = fs::remove_file(path);
}
