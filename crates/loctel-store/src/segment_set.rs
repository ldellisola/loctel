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
//!   file is the lowest number (what the disk cap deletes first).
//! - **Dead bytes.** A record whose data went somewhere else (a trace that came
//!   back to memory, or was removed or evicted) cannot be taken out of the
//!   file it is in. [`mark_dead`](SegmentSet::mark_dead) counts it: it still
//!   takes disk until its whole segment is deleted.
//! - **Reading.** A map ([`Reader`]) per file, made the first time a record of
//!   the file is read, and made again when the file has grown past the part it
//!   mapped (a map covers the file as it was when it was made, and a file that
//!   is still being written grows).
//! - **Deleting.** The disk cap deletes the oldest segment whole
//!   ([`plan_eviction`](SegmentSet::plan_eviction), then
//!   [`delete`](SegmentSet::delete)): the file, its map and its writer if it is
//!   still the one being written. What was in it is the caller's to forget first
//!   (the store forgets the traces and chunks whose records were in it).
//! - **A failing disk.** A write that fails is the caller's to report. The set
//!   can be told to [`suspend`](SegmentSet::suspend) writes, which makes every
//!   append fail at once with the reason it was suspended for, without touching
//!   the disk, until [`resume`](SegmentSet::resume): the store's way of retrying
//!   once a minute and not on every call.
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
    /// The sum of the sizes of `segments`, kept as they change: the disk cap
    /// asks for it after every write.
    total: u64,
    /// Behind a lock because readers take `&self`.
    readers: Mutex<HashMap<u64, Mapped>>,
    /// Why writes are suspended, while they are.
    suspended: Option<String>,
    /// Test hook: every write fails as a full volume's would.
    inject_failure: bool,
    /// How many writes have failed, for real or by the hook, while not
    /// suspended (a suspended write fails without trying).
    failed_attempts: u64,
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
            total: 0,
            readers: Mutex::new(HashMap::new()),
            suspended: None,
            inject_failure: false,
            failed_attempts: 0,
        })
    }

    /// Appends `payload` as a record of a segment of `kind`, and says where it
    /// went. On an error nothing was added: the record is not in the file and
    /// the next append is as good as this one (unless writes are
    /// [suspended](Self::suspend), when it fails at once).
    pub(crate) fn append(&mut self, kind: Kind, payload: &[u8]) -> io::Result<Location> {
        self.check_writable()?;
        let result = if self.inject_failure {
            Err(io::Error::from_raw_os_error(libc::ENOSPC))
        } else {
            self.write(kind, payload)
        };
        if result.is_err() {
            self.failed_attempts += 1;
        }
        result
    }

    fn write(&mut self, kind: Kind, payload: &[u8]) -> io::Result<Location> {
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
            self.total = self
                .total
                .saturating_sub(segment.bytes)
                .saturating_add(bytes);
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
        self.total
    }

    /// The segments the disk cap deletes to bring the files down to `cap`
    /// bytes: the oldest ones (the lowest numbers), as many as it takes, in
    /// order. Empty when the files are under the cap already. A segment that is
    /// still being written is one of them like any other, when it is the
    /// oldest.
    pub(crate) fn plan_eviction(&self, cap: u64) -> Vec<u64> {
        let mut left = self.total;
        let mut doomed = Vec::new();
        for (seq, segment) in &self.segments {
            if left <= cap {
                break;
            }
            left = left.saturating_sub(segment.bytes);
            doomed.push(*seq);
        }
        doomed
    }

    /// Deletes the segments `doomed`: their files and their maps, and their
    /// writers if they are still open (the next append of that kind starts a new
    /// file). Their numbers are not used again.
    pub(crate) fn delete(&mut self, doomed: &[u64]) {
        self.open
            .retain(|_, writer| !doomed.contains(&writer.seq()));
        let readers = self.readers.get_mut();
        for seq in doomed {
            readers.remove(seq);
            if let Some(segment) = self.segments.remove(seq) {
                self.total = self.total.saturating_sub(segment.bytes);
                remove_file(&segment.path);
            }
        }
    }

    /// From now on every append fails at once with `reason`, and nothing is
    /// written, until [`resume`](Self::resume). Reads, dead marks and deletes
    /// are not affected.
    pub(crate) fn suspend(&mut self, reason: &str) {
        self.suspended = Some(reason.to_owned());
    }

    /// Lets appends try the disk again.
    pub(crate) fn resume(&mut self) {
        self.suspended = None;
    }

    /// `Err` with the reason while writes are suspended: what an append would
    /// say, for a caller that would rather not make its payload first.
    pub(crate) fn check_writable(&self) -> io::Result<()> {
        match &self.suspended {
            Some(reason) => Err(io::Error::other(reason.clone())),
            None => Ok(()),
        }
    }

    /// Test hook: makes every append fail with "no space left on device" (or,
    /// with `false`, work again).
    pub(crate) fn inject_failure(&mut self, on: bool) {
        self.inject_failure = on;
    }

    /// How many appends failed that tried the disk (or the test hook).
    pub(crate) fn failed_attempts(&self) -> u64 {
        self.failed_attempts
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
        self.total = 0;
        self.suspended = None;
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A set that rolls after every record, so that each record is a segment.
    fn one_record_per_segment(dir: &Path) -> SegmentSet {
        SegmentSet::new(dir.to_path_buf(), 1).unwrap()
    }

    fn files(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn eviction_plan_takes_the_oldest_files_until_the_rest_fit() {
        let dir = tempfile::tempdir().unwrap();
        let mut set = one_record_per_segment(dir.path());
        let mut sizes = Vec::new();
        for n in 0..5u8 {
            let location = set.append(Kind::Traces, &[n; 100]).unwrap();
            sizes.push(location.disk_bytes());
        }
        let total: u64 = sizes.iter().sum();
        assert_eq!(set.disk_bytes(), total, "the total follows the appends");
        assert_eq!(files(dir.path()).len(), 5);

        assert_eq!(set.plan_eviction(total), Vec::<u64>::new(), "it fits");
        assert_eq!(set.plan_eviction(total - 1), vec![0], "one file is enough");
        assert_eq!(set.plan_eviction(total - sizes[0]), vec![0]);
        assert_eq!(set.plan_eviction(total - sizes[0] - 1), vec![0, 1]);
        assert_eq!(set.plan_eviction(0), vec![0, 1, 2, 3, 4], "all of them");

        set.delete(&[0, 1]);
        assert_eq!(set.disk_bytes(), total - sizes[0] - sizes[1]);
        assert_eq!(
            files(dir.path()),
            [
                "traces-000002.seg",
                "traces-000003.seg",
                "traces-000004.seg"
            ]
        );
        // What is left still reads, and a number is not used again.
        let location = set.append(Kind::Traces, &[9; 10]).unwrap();
        assert_eq!(location.seq, 5);
        assert_eq!(set.read(location).unwrap(), vec![9; 10]);
        assert!(
            set.read(Location {
                seq: 0,
                offset: 0,
                len: 4
            })
            .is_err()
        );
    }

    #[test]
    fn deleting_the_open_segment_makes_the_next_append_start_another() {
        let dir = tempfile::tempdir().unwrap();
        let mut set = SegmentSet::new(dir.path().to_path_buf(), u64::MAX).unwrap();
        let first = set.append(Kind::Logs, b"one").unwrap();
        set.delete(&[first.seq]);
        assert_eq!(set.disk_bytes(), 0);
        assert!(files(dir.path()).is_empty());

        let second = set.append(Kind::Logs, b"two").unwrap();
        assert_ne!(second.seq, first.seq);
        assert_eq!(set.read(second).unwrap(), b"two");
        assert_eq!(files(dir.path()), [format!("logs-{:06}.seg", second.seq)]);
    }

    #[test]
    fn suspended_writes_fail_at_once_with_the_reason_and_count_as_no_attempt() {
        let dir = tempfile::tempdir().unwrap();
        let mut set = SegmentSet::new(dir.path().to_path_buf(), u64::MAX).unwrap();
        set.append(Kind::Traces, b"before").unwrap();

        set.inject_failure(true);
        let error = set.append(Kind::Traces, b"x").unwrap_err();
        assert_eq!(error.raw_os_error(), Some(libc::ENOSPC));
        assert_eq!(set.failed_attempts(), 1);

        set.suspend("No space left on device (os error 28)");
        let error = set.append(Kind::Traces, b"x").unwrap_err();
        assert_eq!(error.to_string(), "No space left on device (os error 28)");
        assert!(set.check_writable().is_err());
        assert_eq!(set.failed_attempts(), 1, "it did not try the disk");

        set.resume();
        set.inject_failure(false);
        assert!(set.check_writable().is_ok());
        let after = set.append(Kind::Traces, b"after").unwrap();
        assert_eq!(set.read(after).unwrap(), b"after");
    }
}
