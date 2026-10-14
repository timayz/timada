//! How many tenant databases this machine can keep open, when the operator
//! leaves `tenants.max_open` unset.
//!
//! An open tenant holds up to three SQLite connections, each with its own OS
//! thread, about three file descriptors (the database, its write-ahead log,
//! the shared-memory index) and a page cache of about 2 MiB. File descriptors
//! run out first on a default Linux limit; memory on a small machine.

/// File descriptors one open tenant may use, rounded up.
const FDS_PER_TENANT: u64 = 10;
/// Memory one open tenant may use: page caches, thread stacks, the group's
/// state, with room to spare.
const BYTES_PER_TENANT: u64 = 16 * 1024 * 1024;
/// Never fewer, so a tight machine still serves a handful of tenants.
const MIN: usize = 16;
/// Never more: past it, one node is better split.
const MAX: usize = 4096;
/// When nothing can be measured.
const FALLBACK: usize = 64;

/// Measures the machine and logs what it found. Raises this process's
/// open-file limit to the most it is allowed first.
pub fn auto_max_open() -> usize {
    let nofile = open_file_limit();
    let mem_available = available_memory();
    let max_open = budget(nofile, mem_available);
    tracing::info!(
        max_open,
        nofile,
        mem_available_mib = mem_available.map(|b| b / (1024 * 1024)),
        "tenant limit worked out from the machine"
    );
    max_open
}

/// Half of the file descriptors go to tenants (the rest to sockets, the
/// platform database and media files), a quarter of the available memory;
/// the tighter of the two wins.
pub fn budget(nofile: Option<u64>, mem_available: Option<u64>) -> usize {
    let by_fds = nofile.map(|n| n / 2 / FDS_PER_TENANT);
    let by_mem = mem_available.map(|b| b / 4 / BYTES_PER_TENANT);
    let tightest = match (by_fds, by_mem) {
        (Some(a), Some(b)) => a.min(b),
        (Some(a), None) | (None, Some(a)) => a,
        (None, None) => return FALLBACK,
    };
    usize::try_from(tightest).unwrap_or(MAX).clamp(MIN, MAX)
}

/// The soft open-file limit, raised to the hard one when it is lower.
#[cfg(unix)]
fn open_file_limit() -> Option<u64> {
    let mut limit = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: `limit` is a valid, writable rlimit for the call to fill.
    if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) } != 0 {
        return None;
    }
    if limit.rlim_cur < limit.rlim_max {
        let raised = libc::rlimit {
            rlim_cur: limit.rlim_max,
            rlim_max: limit.rlim_max,
        };
        // SAFETY: `raised` is a valid rlimit; a refusal leaves the old limit.
        if unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &raised) } == 0 {
            limit.rlim_cur = limit.rlim_max;
        }
    }
    if limit.rlim_cur == libc::RLIM_INFINITY {
        return None;
    }
    Some(limit.rlim_cur)
}

#[cfg(not(unix))]
fn open_file_limit() -> Option<u64> {
    None
}

/// `MemAvailable` from `/proc/meminfo`, in bytes; `None` off Linux.
fn available_memory() -> Option<u64> {
    let meminfo = std::fs::read_to_string("/proc/meminfo").ok()?;
    let kib = meminfo
        .lines()
        .find_map(|line| line.strip_prefix("MemAvailable:"))?
        .trim()
        .strip_suffix("kB")?
        .trim()
        .parse::<u64>()
        .ok()?;
    Some(kib * 1024)
}

#[cfg(test)]
mod tests {
    use super::*;

    const GIB: u64 = 1024 * 1024 * 1024;

    #[test]
    fn the_tighter_resource_wins() {
        // 1024 descriptors: 51 tenants; 8 GiB: 128 tenants.
        assert_eq!(budget(Some(1024), Some(8 * GIB)), 51);
        // A large descriptor limit on a 2 GiB machine: memory decides.
        assert_eq!(budget(Some(1 << 20), Some(2 * GIB)), 32);
    }

    #[test]
    fn one_measure_is_enough() {
        assert_eq!(budget(Some(4096), None), 204);
        assert_eq!(budget(None, Some(16 * GIB)), 256);
        assert_eq!(budget(None, None), FALLBACK);
    }

    #[test]
    fn the_result_stays_within_bounds() {
        assert_eq!(budget(Some(64), Some(GIB / 4)), MIN);
        assert_eq!(budget(Some(u64::MAX), Some(u64::MAX)), MAX);
    }
}
