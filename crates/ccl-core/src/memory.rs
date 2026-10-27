//! Launch-time heap budgeting. No background sampling and no swap-as-RAM accounting.
use crate::config::{MAX_LAUNCH_MEMORY_MB, MIN_LAUNCH_MEMORY_MB};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{path::Path, str::FromStr};

const MIB: u64 = 1024 * 1024;

/// JSON/config/IPC accepts "auto" or an integer MiB limit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MemoryPolicy {
    Auto(AutoMemory),
    Manual(u32),
}
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AutoMemory {
    #[default]
    Auto,
}
impl Default for MemoryPolicy {
    fn default() -> Self {
        Self::AUTO
    }
}
impl MemoryPolicy {
    pub const AUTO: Self = Self::Auto(AutoMemory::Auto);
    pub fn validate(self) -> Result<(), String> {
        if let Self::Manual(mb) = self
            && !(MIN_LAUNCH_MEMORY_MB..=MAX_LAUNCH_MEMORY_MB).contains(&mb)
        {
            return Err(format!(
                "memory must be auto or {MIN_LAUNCH_MEMORY_MB}..={MAX_LAUNCH_MEMORY_MB} MiB"
            ));
        }
        Ok(())
    }
}
impl FromStr for MemoryPolicy {
    type Err = String;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let policy = if value == "auto" {
            Self::AUTO
        } else {
            Self::Manual(
                value
                    .parse()
                    .map_err(|_| "memory must be auto or an integer MiB limit".to_owned())?,
            )
        };
        policy.validate()?;
        Ok(policy)
    }
}

/// Bytes, not KiB; available includes reclaimable physical memory, never swap.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct MemorySnapshot {
    pub total_bytes: u64,
    pub available_bytes: u64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryDecision {
    pub policy: MemoryPolicy,
    pub max_memory_mb: u32,
    pub snapshot: Option<MemorySnapshot>,
    pub fallback: bool,
    pub warning: Option<String>,
}

pub fn decide(
    policy: MemoryPolicy,
    snapshot: Result<MemorySnapshot, String>,
) -> Result<MemoryDecision, String> {
    policy.validate()?;
    if let MemoryPolicy::Manual(mb) = policy {
        return Ok(MemoryDecision {
            policy,
            max_memory_mb: mb,
            snapshot: None,
            fallback: false,
            warning: None,
        });
    }
    let snapshot = match snapshot {
        Ok(value) if value.total_bytes > 0 && value.available_bytes <= value.total_bytes => value,
        error => {
            return Ok(MemoryDecision {
                policy,
                max_memory_mb: 2048,
                snapshot: None,
                fallback: true,
                warning: Some(format!(
                    "Memory detection unavailable; using 2048 MiB. Close other applications or choose a manual limit if needed. {}",
                    error
                        .err()
                        .unwrap_or_else(|| "Invalid physical memory statistics".into())
                )),
            });
        }
    };
    let usable_mb = (snapshot.available_bytes / MIB).saturating_sub(1024);
    let suggested = if usable_mb <= 8192 {
        usable_mb * 4 / 5
    } else {
        (8192 * 4 + usable_mb - 8192) / 5
    };
    let mb = suggested.min(16384) / 256 * 256;
    if mb < u64::from(MIN_LAUNCH_MEMORY_MB) {
        return Err("Not enough available memory for automatic allocation after reserving 1 GiB. Close other applications or choose a manual limit.".into());
    }
    Ok(MemoryDecision {
        policy,
        max_memory_mb: mb as u32,
        snapshot: Some(snapshot),
        fallback: false,
        warning: None,
    })
}

pub fn resolve(policy: MemoryPolicy) -> Result<MemoryDecision, String> {
    // Manual allocations do not depend on platform probes.
    if matches!(policy, MemoryPolicy::Manual(_)) {
        decide(policy, Err(String::new()))
    } else {
        decide(policy, probe())
    }
}

/// Stable across display-name changes; callers use the actual instance ID, not a loader revision.
pub fn profile_key(directory: &Path, id: &str) -> Result<String, std::io::Error> {
    let canonical = std::fs::canonicalize(directory)?;
    let mut hash = Sha256::new();
    hash.update(canonical.to_string_lossy().as_bytes());
    hash.update([0]);
    hash.update(id.as_bytes());
    Ok(format!("memory:{:x}", hash.finalize()))
}

#[cfg(target_os = "windows")]
pub fn probe() -> Result<MemorySnapshot, String> {
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    // SAFETY: zeroed POD with the documented structure size, valid for this call.
    unsafe {
        let mut status: MEMORYSTATUSEX = std::mem::zeroed();
        status.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
        if GlobalMemoryStatusEx(&mut status) == 0 {
            return Err("GlobalMemoryStatusEx failed".into());
        }
        Ok(MemorySnapshot {
            total_bytes: status.ullTotalPhys,
            available_bytes: status.ullAvailPhys,
        })
    }
}

#[cfg(target_os = "macos")]
pub fn probe() -> Result<MemorySnapshot, String> {
    // The libc crate omits port deallocation and deprecates its port helpers;
    // declare just these stable system ABI symbols rather than adding a runtime.
    unsafe extern "C" {
        fn mach_host_self() -> libc::mach_port_t;
        static mach_task_self_: libc::mach_port_t;
        fn mach_port_deallocate(
            task: libc::mach_port_t,
            name: libc::mach_port_t,
        ) -> libc::kern_return_t;
    }
    // SAFETY: native calls receive correctly sized initialized output buffers. The
    // send right returned by mach_host_self is released on every path.
    unsafe {
        let mut total: u64 = 0;
        let mut size = std::mem::size_of::<u64>();
        if libc::sysctlbyname(
            c"hw.memsize".as_ptr(),
            (&mut total as *mut u64).cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        ) != 0
        {
            return Err("Cannot read macOS physical memory".into());
        }
        let host = mach_host_self();
        let mut stats: libc::vm_statistics64 = std::mem::zeroed();
        let mut count = libc::HOST_VM_INFO64_COUNT;
        let result = libc::host_statistics64(
            host,
            libc::HOST_VM_INFO64,
            (&mut stats as *mut libc::vm_statistics64).cast(),
            &mut count,
        );
        let page_size = libc::sysconf(libc::_SC_PAGESIZE);
        mach_port_deallocate(mach_task_self_, host);
        if result != libc::KERN_SUCCESS || page_size <= 0 {
            return Err(format!(
                "Cannot read macOS available memory (status {result}, count {count})"
            ));
        }
        mac_snapshot(&stats, count, page_size as u64, total)
    }
}

#[cfg(target_os = "macos")]
fn mac_snapshot(
    stats: &libc::vm_statistics64,
    count: u32,
    page_size: u64,
    total: u64,
) -> Result<MemorySnapshot, String> {
    // Older kernels return a shorter, valid revision. Only require the prefix we
    // actually read, not the size of the newest libc/SDK statistics structure.
    let needed = (std::mem::offset_of!(libc::vm_statistics64, inactive_count)
        + std::mem::size_of::<libc::natural_t>())
    .div_ceil(std::mem::size_of::<libc::integer_t>()) as u32;
    if count < needed || page_size == 0 {
        return Err("Truncated macOS memory statistics".into());
    }
    // XNU's free_count ALREADY includes speculative_count. vm_stat splits them
    // for display; adding speculative_count again would overstate capacity.
    let pages = u64::from(stats.free_count) + u64::from(stats.inactive_count);
    Ok(MemorySnapshot {
        total_bytes: total,
        available_bytes: pages.saturating_mul(page_size).min(total),
    })
}

#[cfg(target_os = "linux")]
pub fn probe() -> Result<MemorySnapshot, String> {
    let text = std::fs::read_to_string("/proc/meminfo")
        .map_err(|_| "Cannot read Linux memory statistics")?;
    let mut snapshot = parse_meminfo(&text)?;
    // Resolve the process's actual cgroup mount/root, including containers, and
    // apply ancestor limits as well as the leaf. Paths come only from procfs.
    let groups = std::fs::read_to_string("/proc/self/cgroup")
        .map_err(|_| "Cannot read cgroup membership")?;
    let mounts =
        std::fs::read_to_string("/proc/self/mountinfo").map_err(|_| "Cannot read cgroup mounts")?;
    for line in mounts.lines() {
        let Some((left, right)) = line.split_once(" - ") else {
            continue;
        };
        let fields: Vec<_> = left.split_whitespace().collect();
        let fs: Vec<_> = right.split_whitespace().collect();
        if fields.len() < 5 || fs.len() < 3 {
            continue;
        }
        let v2 = fs[0] == "cgroup2";
        if !v2 && !(fs[0] == "cgroup" && fs[2].split(',').any(|c| c == "memory")) {
            continue;
        }
        for group in groups.lines() {
            let parts: Vec<_> = group.splitn(3, ':').collect();
            if parts.len() != 3
                || !(if v2 {
                    parts[1].is_empty()
                } else {
                    parts[1].split(',').any(|c| c == "memory")
                })
            {
                continue;
            }
            let root = proc_path(fields[3]);
            let mount = proc_path(fields[4]);
            let group = Path::new(parts[2]);
            let relative = group
                .strip_prefix(&root)
                .map_err(|_| "Cgroup path is outside its visible mount")?;
            if relative
                .components()
                .any(|c| !matches!(c, std::path::Component::Normal(_)))
            {
                return Err("Unsafe cgroup path".into());
            }
            let mut directory = mount.join(relative);
            loop {
                let (limit_name, usage_name) = if v2 {
                    ("memory.max", "memory.current")
                } else {
                    ("memory.limit_in_bytes", "memory.usage_in_bytes")
                };
                match std::fs::read_to_string(directory.join(limit_name)) {
                    Ok(limit) if limit.trim() != "max" => {
                        let limit: u64 = limit
                            .trim()
                            .parse()
                            .map_err(|_| "Invalid cgroup memory limit")?;
                        let usage = std::fs::read_to_string(directory.join(usage_name))
                            .map_err(|_| "Cannot read cgroup memory usage")?;
                        let usage: u64 = usage
                            .trim()
                            .parse()
                            .map_err(|_| "Invalid cgroup memory usage")?;
                        apply_limit(&mut snapshot, limit, usage);
                    }
                    Ok(_) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(_) => return Err("Cannot read cgroup memory limit".into()),
                }
                if directory == mount || !directory.pop() {
                    break;
                }
            }
        }
    }
    Ok(snapshot)
}
#[cfg(target_os = "linux")]
fn proc_path(value: &str) -> std::path::PathBuf {
    value
        .replace("\\040", " ")
        .replace("\\011", "\t")
        .replace("\\012", "\n")
        .replace("\\134", "\\")
        .into()
}
#[cfg(any(target_os = "linux", test))]
fn parse_meminfo(text: &str) -> Result<MemorySnapshot, String> {
    let read = |name: &str| -> Result<u64, String> {
        let value = text
            .lines()
            .find_map(|line| line.strip_prefix(name))
            .ok_or_else(|| format!("Missing {name}"))?;
        let fields: Vec<_> = value.split_whitespace().collect();
        if fields.len() != 2 || fields[1] != "kB" {
            return Err("Invalid memory statistic units".into());
        }
        fields[0]
            .parse::<u64>()
            .ok()
            .and_then(|n| n.checked_mul(1024))
            .ok_or_else(|| "Invalid memory statistic".into())
    };
    Ok(MemorySnapshot {
        total_bytes: read("MemTotal:")?,
        available_bytes: read("MemAvailable:")?,
    })
}
#[cfg(any(target_os = "linux", test))]
fn apply_limit(snapshot: &mut MemorySnapshot, limit: u64, usage: u64) {
    snapshot.total_bytes = snapshot.total_bytes.min(limit);
    snapshot.available_bytes = snapshot
        .available_bytes
        .min(limit.saturating_sub(usage))
        .min(snapshot.total_bytes);
}
#[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
pub fn probe() -> Result<MemorySnapshot, String> {
    Err("Unsupported memory probe platform".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn automatic(available_mb: u64) -> Result<MemoryDecision, String> {
        decide(
            MemoryPolicy::AUTO,
            Ok(MemorySnapshot {
                total_bytes: available_mb * MIB,
                available_bytes: available_mb * MIB,
            }),
        )
    }
    #[test]
    fn budgets_reserve_rounding_and_cap_without_inflating_low_memory() {
        assert!(automatic(1024).is_err());
        assert!(automatic(1343).is_err());
        assert_eq!(automatic(1344).unwrap().max_memory_mb, 256);
        assert_eq!(automatic(4096).unwrap().max_memory_mb, 2304);
        assert_eq!(automatic(9216).unwrap().max_memory_mb, 6400);
        assert_eq!(automatic(17408).unwrap().max_memory_mb, 8192);
        assert_eq!(automatic(131072).unwrap().max_memory_mb, 16384);
    }
    #[test]
    fn manual_is_authoritative_and_probe_failure_is_reported() {
        assert_eq!(
            resolve(MemoryPolicy::Manual(32768)).unwrap().max_memory_mb,
            32768
        );
        assert!(resolve(MemoryPolicy::Manual(255)).is_err());
        let fallback = decide(MemoryPolicy::AUTO, Err("unavailable".into())).unwrap();
        assert!(fallback.fallback && fallback.warning.is_some());
        assert_eq!(fallback.max_memory_mb, 2048);
        assert!(serde_json::from_str::<MemoryPolicy>("\"inherit\"").is_err());
        assert_eq!("auto".parse::<MemoryPolicy>().unwrap(), MemoryPolicy::AUTO);
    }
    #[test]
    fn linux_units_and_container_headroom_are_not_swap_or_host_ram() {
        let mut s = parse_meminfo("MemTotal: 16777216 kB\nMemFree: 10 kB\nMemAvailable: 8388608 kB\nSwapFree: 99999999 kB").unwrap();
        apply_limit(&mut s, 4096 * MIB, 2048 * MIB);
        assert_eq!(s.available_bytes, 2048 * MIB);
        assert_eq!(s.total_bytes, 4096 * MIB);
        apply_limit(&mut s, 1024 * MIB, 2048 * MIB);
        assert_eq!(s.available_bytes, 0);
        assert!(parse_meminfo("MemTotal: 10 MB\nMemAvailable: 10 MB").is_err());
    }
    #[cfg(target_os = "macos")]
    #[test]
    fn accepts_older_mach_replies_without_double_counting_speculative_pages() {
        // SAFETY: this C statistics struct consists of integer counters.
        let mut stats: libc::vm_statistics64 = unsafe { std::mem::zeroed() };
        stats.free_count = 100;
        stats.inactive_count = 50;
        stats.speculative_count = 30; // already part of free_count
        let old_count = (std::mem::offset_of!(libc::vm_statistics64, decompressions)
            / std::mem::size_of::<libc::integer_t>()) as u32;
        assert!(old_count < libc::HOST_VM_INFO64_COUNT);
        assert_eq!(
            mac_snapshot(&stats, old_count, 4096, 1000 * 4096)
                .unwrap()
                .available_bytes,
            150 * 4096
        );
        assert!(mac_snapshot(&stats, 2, 4096, 1000 * 4096).is_err());
    }

    #[test]
    fn native_probe_reports_sensible_physical_values() {
        let s = probe().expect("native memory probe");
        assert!(s.total_bytes > 0 && s.available_bytes <= s.total_bytes);
    }
}
