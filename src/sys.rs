//! What the machine allows the process: memory and CPU budget, read through
//! cgroups on Linux like the Go plugin, and a few process readings.

#[cfg(target_os = "linux")]
use std::path::Path;

/// Memory limit of the cgroup of this process, when one is set.
#[cfg(target_os = "linux")]
fn cgroup_memory_limit() -> Option<u64> {
    const SENTINEL: u64 = 1 << 60;
    let own = std::fs::read_to_string("/proc/self/cgroup").unwrap_or_default();
    let v2_path = own.lines().find_map(|l| l.strip_prefix("0::")).map(str::to_owned).unwrap_or_else(|| "/".to_owned());

    let read = |p: &Path| -> Option<u64> {
        let text = std::fs::read_to_string(p).ok()?;
        let text = text.trim();
        if text == "max" {
            return None;
        }
        text.parse::<u64>().ok().filter(|v| *v > 0 && *v < SENTINEL)
    };

    let mut best: Option<u64> = None;
    let mut consider = |v: Option<u64>| {
        if let Some(v) = v {
            best = Some(best.map_or(v, |b| b.min(v)));
        }
    };
    let mut dir = Path::new("/sys/fs/cgroup").join(v2_path.trim_start_matches('/'));
    loop {
        consider(read(&dir.join("memory.max")));
        if dir == Path::new("/sys/fs/cgroup") || !dir.pop() {
            break;
        }
    }
    consider(read(Path::new("/sys/fs/cgroup/memory/memory.limit_in_bytes")));
    best
}

#[cfg(target_os = "linux")]
fn total_memory() -> Option<u64> {
    let text = std::fs::read_to_string("/proc/meminfo").ok()?;
    let kb: u64 = text.lines().find_map(|l| l.strip_prefix("MemTotal:"))?.split_whitespace().next()?.parse().ok()?;
    Some(kb * 1024)
}

#[cfg(target_os = "macos")]
fn total_memory() -> Option<u64> {
    let mut size: u64 = 0;
    let mut len = std::mem::size_of::<u64>();
    let name = std::ffi::CString::new("hw.memsize").ok()?;
    // SAFETY: the buffer and its length describe one u64, the name is NUL terminated.
    let rc =
        unsafe { libc::sysctlbyname(name.as_ptr(), (&mut size as *mut u64).cast(), &mut len, std::ptr::null_mut(), 0) };
    (rc == 0 && size > 0).then_some(size)
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn total_memory() -> Option<u64> {
    None
}

#[cfg(not(target_os = "linux"))]
fn cgroup_memory_limit() -> Option<u64> {
    None
}

/// The memory budget in bytes: the cgroup limit when it is below the total
/// memory, otherwise the total memory. `None` when neither is known.
pub fn available_memory() -> Option<u64> {
    if let Some(mb) = std::env::var("LOG_ANALYTICS_MEMORY_MB").ok().and_then(|v| v.trim().parse::<u64>().ok()) {
        return Some(mb << 20);
    }
    match (cgroup_memory_limit(), total_memory()) {
        (Some(limit), Some(total)) => Some(limit.min(total)),
        (limit, total) => limit.or(total),
    }
}

/// CPUs the process may use: the smaller of the parallelism and the cgroup
/// quota, never below one.
pub fn available_cpus() -> usize {
    let procs = std::thread::available_parallelism().map_or(1, usize::from);
    match cpu_quota() {
        Some(q) => procs.min(q.ceil().max(1.0) as usize),
        None => procs,
    }
}

#[cfg(target_os = "linux")]
fn cpu_quota() -> Option<f64> {
    let own = std::fs::read_to_string("/proc/self/cgroup").unwrap_or_default();
    let path = own.lines().find_map(|l| l.strip_prefix("0::")).unwrap_or("/");
    let mut dir = Path::new("/sys/fs/cgroup").join(path.trim_start_matches('/'));
    let mut best: Option<f64> = None;
    loop {
        if let Ok(text) = std::fs::read_to_string(dir.join("cpu.max")) {
            let mut it = text.split_whitespace();
            if let (Some(q), Some(p)) = (it.next(), it.next()) {
                if let (Ok(q), Ok(p)) = (q.parse::<f64>(), p.parse::<f64>()) {
                    if q > 0.0 && p > 0.0 {
                        best = Some(best.map_or(q / p, |b: f64| b.min(q / p)));
                    }
                }
            }
        }
        if dir == Path::new("/sys/fs/cgroup") || !dir.pop() {
            break;
        }
    }
    best
}

#[cfg(not(target_os = "linux"))]
fn cpu_quota() -> Option<f64> {
    None
}

/// Peak resident set size of the process in MiB, zero where unknown.
#[cfg(unix)]
pub fn peak_rss_mb() -> u64 {
    // SAFETY: rusage is plain data and getrusage only writes into it.
    let mut ru: libc::rusage = unsafe { std::mem::zeroed() };
    let rc = unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut ru) };
    if rc != 0 {
        return 0;
    }
    let raw = ru.ru_maxrss as u64;
    if cfg!(target_os = "linux") {
        raw / 1024
    } else {
        raw / (1024 * 1024)
    }
}

#[cfg(not(unix))]
pub fn peak_rss_mb() -> u64 {
    0
}

/// Hands the memory the allocator holds back to the system. An indexing round
/// uses a lot of memory for a while, and an idle plugin should not keep it.
#[cfg(not(windows))]
pub fn release_memory() {
    // SAFETY: mi_collect only returns free memory of the allocator to the system.
    unsafe { libmimalloc_sys::mi_collect(true) };
}

#[cfg(windows)]
pub fn release_memory() {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn budget_is_reported() {
        assert!(available_cpus() >= 1);
        if let Some(m) = available_memory() {
            assert!(m > 0);
        }
    }
}
