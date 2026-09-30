//! What the machine allows the process: memory and CPU budget, read through
//! cgroups on Linux like the Go plugin, and a few process readings.

#[cfg(target_os = "linux")]
use std::path::{Path, PathBuf};

#[cfg(target_os = "linux")]
const CGROUP_ROOT: &str = "/sys/fs/cgroup";

/// Directories holding the limits of this process, from its own group up to
/// the root. `controller` is `None` for the unified hierarchy, otherwise the
/// name of a v1 controller. Without group information only the root is listed.
#[cfg(target_os = "linux")]
fn group_dirs(controller: Option<&str>) -> Vec<PathBuf> {
    let base = match controller {
        Some(c) => Path::new(CGROUP_ROOT).join(c),
        None => PathBuf::from(CGROUP_ROOT),
    };
    let own = std::fs::read_to_string("/proc/self/cgroup").ok().and_then(|text| own_group(&text, controller));
    let Some(own) = own.filter(|p| p.starts_with('/')) else {
        return vec![base];
    };
    let mut dirs = Vec::new();
    let mut current = PathBuf::from(own);
    loop {
        dirs.push(base.join(current.strip_prefix("/").unwrap_or(&current)));
        if !current.pop() {
            break;
        }
    }
    dirs
}

/// The group path of one line of /proc/self/cgroup: the unified entry when
/// `controller` is `None`, otherwise the entry listing that controller.
#[cfg(target_os = "linux")]
fn own_group(text: &str, controller: Option<&str>) -> Option<String> {
    text.lines().find_map(|line| {
        let mut parts = line.splitn(3, ':');
        let (id, controllers, path) = (parts.next()?, parts.next()?, parts.next()?);
        let matches = match controller {
            None => id == "0" && controllers.is_empty(),
            Some(c) => controllers.split(',').any(|x| x == c),
        };
        matches.then(|| path.to_owned())
    })
}

/// A cgroup file holding one number. "max", zero and the sentinels some
/// kernels use for no limit read as `None`.
#[cfg(target_os = "linux")]
fn read_limit(path: &Path) -> Option<u64> {
    const SENTINEL: u64 = 1 << 60;
    let text = std::fs::read_to_string(path).ok()?;
    text.trim().parse::<u64>().ok().filter(|v| *v > 0 && *v < SENTINEL)
}

/// One field of a cgroup or proc file of "name value" lines.
#[cfg(target_os = "linux")]
fn read_field(path: &Path, name: &str) -> Option<u64> {
    let text = std::fs::read_to_string(path).ok()?;
    text.lines().find_map(|l| {
        let mut it = l.split_whitespace();
        (it.next()?.trim_end_matches(':') == name).then(|| it.next()?.parse().ok()).flatten()
    })
}

/// The memory limit of this process and the anonymous memory of the group
/// that sets it: the smallest limit of the own group and its ancestors, in the
/// unified hierarchy and in the v1 memory controller.
#[cfg(target_os = "linux")]
fn cgroup_memory() -> Option<(u64, Option<u64>)> {
    let mut best: Option<(u64, Option<u64>)> = None;
    let mut consider = |limit: Option<u64>, anon: &dyn Fn() -> Option<u64>| {
        if let Some(limit) = limit {
            if best.is_none_or(|(b, _)| limit < b) {
                best = Some((limit, anon()));
            }
        }
    };
    for dir in group_dirs(None) {
        consider(read_limit(&dir.join("memory.max")), &|| read_field(&dir.join("memory.stat"), "anon"));
    }
    for dir in group_dirs(Some("memory")) {
        consider(read_limit(&dir.join("memory.limit_in_bytes")), &|| read_field(&dir.join("memory.stat"), "total_rss"));
    }
    best
}

#[cfg(target_os = "linux")]
fn meminfo(name: &str) -> Option<u64> {
    read_field(Path::new("/proc/meminfo"), name).map(|kb| kb * 1024)
}

#[cfg(target_os = "linux")]
fn total_memory() -> Option<u64> {
    meminfo("MemTotal")
}

/// Anonymous memory this process holds.
#[cfg(target_os = "linux")]
fn own_anon() -> u64 {
    read_field(Path::new("/proc/self/status"), "RssAnon").map_or(0, |kb| kb * 1024)
}

/// The memory budget of this process: the cgroup limit less what the other
/// processes of the group hold, or without a limit the available memory and
/// what this process holds. Never above the total memory.
#[cfg(target_os = "linux")]
fn memory_budget() -> Option<u64> {
    let total = total_memory();
    let own = own_anon();
    let budget = match cgroup_memory() {
        Some((limit, anon)) => {
            let others = anon.map_or(0, |a| a.saturating_sub(own));
            Some(limit.saturating_sub(others))
        }
        None => meminfo("MemAvailable").map(|a| a + own).or(total),
    };
    match (budget, total) {
        (Some(b), Some(t)) => Some(b.min(t)),
        (b, t) => b.or(t),
    }
}

#[cfg(target_os = "macos")]
fn memory_budget() -> Option<u64> {
    let mut size: u64 = 0;
    let mut len = std::mem::size_of::<u64>();
    let name = std::ffi::CString::new("hw.memsize").ok()?;
    // SAFETY: the buffer and its length describe one u64, the name is NUL terminated.
    let rc =
        unsafe { libc::sysctlbyname(name.as_ptr(), (&mut size as *mut u64).cast(), &mut len, std::ptr::null_mut(), 0) };
    (rc == 0 && size > 0).then_some(size)
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn memory_budget() -> Option<u64> {
    None
}

/// The memory budget in bytes, `None` when it cannot be read. The
/// LOG_ANALYTICS_MEMORY_MB environment variable replaces it.
pub fn available_memory() -> Option<u64> {
    if let Some(mb) = std::env::var("LOG_ANALYTICS_MEMORY_MB").ok().and_then(|v| v.trim().parse::<u64>().ok()) {
        return Some(mb << 20);
    }
    memory_budget()
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

/// The smallest CPU quota of the own group and its ancestors, in the unified
/// hierarchy and in the v1 cpu controller.
#[cfg(target_os = "linux")]
fn cpu_quota() -> Option<f64> {
    let mut best: Option<f64> = None;
    let mut consider = |q: f64, p: f64| {
        if q > 0.0 && p > 0.0 {
            best = Some(best.map_or(q / p, |b: f64| b.min(q / p)));
        }
    };
    for dir in group_dirs(None) {
        if let Ok(text) = std::fs::read_to_string(dir.join("cpu.max")) {
            let mut it = text.split_whitespace();
            if let (Some(Ok(q)), Some(Ok(p))) = (it.next().map(str::parse::<f64>), it.next().map(str::parse::<f64>)) {
                consider(q, p);
            }
        }
    }
    for dir in group_dirs(Some("cpu")) {
        let read = |name: &str| std::fs::read_to_string(dir.join(name)).ok()?.trim().parse::<f64>().ok();
        if let (Some(q), Some(p)) = (read("cpu.cfs_quota_us"), read("cpu.cfs_period_us")) {
            consider(q, p);
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

    #[cfg(target_os = "linux")]
    #[test]
    fn own_group_reads_both_hierarchies() {
        let text = "12:cpu,cpuacct:/docker/abc\n9:memory:/docker/abc\n0::/system.slice/x.scope\n";
        assert_eq!(own_group(text, None).as_deref(), Some("/system.slice/x.scope"));
        assert_eq!(own_group(text, Some("memory")).as_deref(), Some("/docker/abc"));
        assert_eq!(own_group(text, Some("cpu")).as_deref(), Some("/docker/abc"));
        assert_eq!(own_group(text, Some("pids")), None);
    }

    #[test]
    fn budget_is_reported() {
        assert!(available_cpus() >= 1);
        if let Some(m) = available_memory() {
            assert!(m > 0);
        }
    }
}
