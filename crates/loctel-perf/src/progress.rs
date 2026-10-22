//! What the run says on stderr while it works. A full run takes minutes and
//! most of it is building the data, so each step says what it is doing and how
//! long the last one took.

use std::sync::OnceLock;
use std::time::Instant;

static START: OnceLock<Instant> = OnceLock::new();

/// Starts the clock the lines are stamped with (the first call wins).
pub fn begin() {
    let _ = START.get_or_init(Instant::now);
}

/// One line on stderr: `perf [  12.3s] message`.
pub fn say(message: impl AsRef<str>) {
    let seconds = START.get_or_init(Instant::now).elapsed().as_secs_f64();
    eprintln!("perf [{seconds:>7.1}s] {}", message.as_ref());
}
