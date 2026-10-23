//! The resident size of this process.

/// What this process has in memory right now, in bytes: `VmRSS` of
/// `/proc/self/status`. `None` off Linux, which has no such file.
pub fn resident_bytes() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    parse_vm_rss(&status)
}

/// `VmRSS:   123456 kB` as bytes.
fn parse_vm_rss(status: &str) -> Option<u64> {
    let line = status.lines().find(|line| line.starts_with("VmRSS:"))?;
    let kilobytes: u64 = line
        .trim_start_matches("VmRSS:")
        .trim()
        .trim_end_matches("kB")
        .trim()
        .parse()
        .ok()?;
    Some(kilobytes * 1024)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_vmrss_in_kilobytes() {
        let status = "Name:\tloctel\nVmPeak:\t  9999 kB\nVmRSS:\t  1234 kB\nThreads:\t4\n";

        assert_eq!(parse_vm_rss(status), Some(1234 * 1024));
        assert_eq!(parse_vm_rss("Name:\tx\n"), None);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn this_process_has_a_resident_size() {
        assert!(resident_bytes().expect("Linux has /proc") > 1_000_000);
    }
}
