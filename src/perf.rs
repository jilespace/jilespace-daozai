//! 系统资源采样：CPU、内存、网络与磁盘。
//!
//! 采样放在外壳线程按秒调用；纯计算与格式化留在跨平台代码里，
//! 这样没有窗口的测试也能验证百分比、速率与容量文案。

use std::time::Instant;

/// 一次采样结果。取不到的项为 `None`，界面据此显示占位状态。
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Performance {
    /// CPU 使用率 0..100。
    pub cpu: Option<f32>,
    /// 已用内存 / 总内存（字节）。
    pub memory: Option<(u64, u64)>,
    /// 下行 / 上行速率（字节每秒）。
    pub network: Option<(f64, f64)>,
    /// 磁盘可用 / 总容量（字节）。
    pub disk: Option<(u64, u64)>,
}

/// 由两次 `GetSystemTimes` 的累计值算出 CPU 使用率。
///
/// kernel 时间包含 idle 时间；忙时 = kernel + user - idle。
pub fn cpu_usage(prev: (u64, u64, u64), current: (u64, u64, u64)) -> Option<f32> {
    let idle = current.0.saturating_sub(prev.0) as f64;
    let kernel = current.1.saturating_sub(prev.1) as f64;
    let user = current.2.saturating_sub(prev.2) as f64;
    let total = kernel + user;
    if total <= 0.0 {
        return None;
    }
    let busy = (total - idle).max(0.0);
    Some(((busy / total) * 100.0).clamp(0.0, 100.0) as f32)
}

/// 字节数转成易读容量：`9.8 GB`、`512 GB`、`820 MB`。
pub fn format_bytes(bytes: u64) -> String {
    const UNITS: [(&str, u64); 4] =
        [("TB", 1 << 40), ("GB", 1 << 30), ("MB", 1 << 20), ("KB", 1 << 10)];
    for (unit, scale) in UNITS {
        if bytes >= scale {
            let value = bytes as f64 / scale as f64;
            return if value >= 100.0 {
                format!("{value:.0} {unit}")
            } else if value >= 10.0 {
                format!("{value:.0} {unit}")
            } else {
                format!("{value:.1} {unit}")
            };
        }
    }
    format!("{bytes} B")
}

/// 每秒字节数转成网络速率文案。
pub fn format_rate(bytes_per_second: f64) -> String {
    if !bytes_per_second.is_finite() || bytes_per_second <= 0.0 {
        return "0 B/s".into();
    }
    format!("{}/s", format_bytes(bytes_per_second.max(0.0) as u64))
}

/// 百分比文案：小于 10 时保留一位小数，其余取整。
pub fn format_percent(value: f32) -> String {
    if value >= 10.0 || value <= 0.0 {
        format!("{:.0}%", value)
    } else {
        format!("{value:.1}%")
    }
}

/// 网络速率归一化到 0..1，用于迷你折线；上限取历史峰值并至少 64 KB/s。
pub fn rate_scale(history: &std::collections::VecDeque<f32>) -> f32 {
    history
        .iter()
        .copied()
        .fold(64.0 * 1024.0, f32::max)
        .max(1.0)
}

/// 保持 CPU 与网络历史的定长队列。
#[derive(Default)]
pub struct History {
    pub cpu: std::collections::VecDeque<f32>,
    pub down: std::collections::VecDeque<f32>,
    pub up: std::collections::VecDeque<f32>,
}

impl History {
    pub const LEN: usize = 48;

    pub fn push(&mut self, cpu: Option<f32>, down: Option<f64>, up: Option<f64>) {
        push_bounded(&mut self.cpu, cpu.unwrap_or(0.0).clamp(0.0, 100.0));
        push_bounded(&mut self.down, down.unwrap_or(0.0).max(0.0) as f32);
        push_bounded(&mut self.up, up.unwrap_or(0.0).max(0.0) as f32);
    }

    pub fn seed(&mut self, samples: impl IntoIterator<Item = (f32, f32, f32)>) {
        self.cpu.clear();
        self.down.clear();
        self.up.clear();
        for (cpu, down, up) in samples {
            push_bounded(&mut self.cpu, cpu);
            push_bounded(&mut self.down, down);
            push_bounded(&mut self.up, up);
        }
    }
}

fn push_bounded(queue: &mut std::collections::VecDeque<f32>, value: f32) {
    if queue.len() >= History::LEN {
        queue.pop_front();
    }
    queue.push_back(value);
}

/// 采样器：保存上一次的累计值，按需产出瞬时数据。
#[derive(Default)]
pub struct Sampler {
    prev_times: Option<(u64, u64, u64)>,
    prev_net: Option<(u64, u64)>,
    prev_at: Option<Instant>,
}

impl Sampler {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn sample(&mut self) -> Performance {
        let times = system_times();
        let cpu = match (self.prev_times, times) {
            (Some(prev), Some(current)) => cpu_usage(prev, current),
            _ => None,
        };
        self.prev_times = times;

        let memory = memory();
        let totals = network_totals();
        let now = Instant::now();
        let network = match (self.prev_net, totals, self.prev_at) {
            (Some((prev_down, prev_up)), Some((down, up)), Some(at)) => {
                let seconds = now.duration_since(at).as_secs_f64().max(0.001);
                Some((
                    down.saturating_sub(prev_down) as f64 / seconds,
                    up.saturating_sub(prev_up) as f64 / seconds,
                ))
            }
            _ => None,
        };
        self.prev_net = totals;
        self.prev_at = Some(now);
        Performance { cpu, memory, network, disk: disk_space() }
    }
}

#[cfg(windows)]
fn system_times() -> Option<(u64, u64, u64)> {
    use windows::Win32::Foundation::FILETIME;
    use windows::Win32::System::Threading::GetSystemTimes;
    let mut idle = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    unsafe { GetSystemTimes(Some(&mut idle), Some(&mut kernel), Some(&mut user)).ok()? };
    let ticks = |time: FILETIME| ((time.dwHighDateTime as u64) << 32) | time.dwLowDateTime as u64;
    Some((ticks(idle), ticks(kernel), ticks(user)))
}

#[cfg(not(windows))]
fn system_times() -> Option<(u64, u64, u64)> {
    None
}

#[cfg(windows)]
fn memory() -> Option<(u64, u64)> {
    use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    let mut status = MEMORYSTATUSEX {
        dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32,
        ..Default::default()
    };
    unsafe { GlobalMemoryStatusEx(&mut status).ok()? };
    let total = status.ullTotalPhys;
    if total == 0 {
        return None;
    }
    Some((total.saturating_sub(status.ullAvailPhys), total))
}

#[cfg(not(windows))]
fn memory() -> Option<(u64, u64)> {
    None
}

#[cfg(windows)]
fn network_totals() -> Option<(u64, u64)> {
    use windows::Win32::NetworkManagement::IpHelper::{FreeMibTable, GetIfTable2, MIB_IF_TABLE2};
    use windows::Win32::NetworkManagement::Ndis::IfOperStatusUp;
    // IF_TYPE_SOFTWARE_LOOPBACK；环回流量不代表真实网络。
    const LOOPBACK: u32 = 24;
    unsafe {
        let mut table: *mut MIB_IF_TABLE2 = std::ptr::null_mut();
        if GetIfTable2(&mut table).0 != 0 {
            return None;
        }
        if table.is_null() {
            return None;
        }
        let count = (*table).NumEntries as usize;
        let rows = (*table).Table.as_ptr();
        let mut down = 0u64;
        let mut up = 0u64;
        for index in 0..count {
            let row = &*rows.add(index);
            // 只统计硬件接口：排除环回、Hyper-V/WSL/VPN 等虚拟网卡，避免重复计数。
            let hardware = row.InterfaceAndOperStatusFlags._bitfield & 1 != 0;
            if !hardware || row.OperStatus != IfOperStatusUp || row.Type == LOOPBACK {
                continue;
            }
            down = down.saturating_add(row.InOctets);
            up = up.saturating_add(row.OutOctets);
        }
        FreeMibTable(table as *const _);
        Some((down, up))
    }
}

#[cfg(not(windows))]
fn network_totals() -> Option<(u64, u64)> {
    None
}

#[cfg(windows)]
fn disk_space() -> Option<(u64, u64)> {
    use std::os::windows::ffi::OsStrExt;
    use windows::core::PCWSTR;
    use windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    let drive = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into());
    let path: Vec<u16> = std::ffi::OsStr::new(&format!("{drive}\\"))
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let mut free = 0u64;
    let mut total = 0u64;
    unsafe {
        GetDiskFreeSpaceExW(PCWSTR(path.as_ptr()), Some(&mut free), Some(&mut total), None).ok()?;
    }
    (total > 0).then_some((free, total))
}

#[cfg(not(windows))]
fn disk_space() -> Option<(u64, u64)> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_usage_handles_percentages_and_invalid_samples() {
        // 750 个 tick 中 375 个 idle：50% 忙（kernel 时间包含 idle）。
        assert_eq!(cpu_usage((0, 0, 0), (375, 500, 250)), Some(50.0));
        assert_eq!(cpu_usage((100, 200, 300), (475, 700, 550)), Some(50.0));
        assert_eq!(cpu_usage((0, 0, 0), (1000, 1000, 0)), Some(0.0));
        assert_eq!(cpu_usage((0, 0, 0), (0, 0, 0)), None);
        assert_eq!(cpu_usage((100, 100, 100), (100, 100, 100)), None);
    }

    #[test]
    fn byte_and_rate_formatting_uses_sensible_units() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(1536), "1.5 KB");
        assert_eq!(format_bytes(10 * 1024 * 1024), "10 MB");
        assert_eq!(format_bytes(9_800_000_000), "9.1 GB");
        assert_eq!(format_bytes(512u64 * 1024 * 1024 * 1024), "512 GB");
        assert_eq!(format_rate(0.0), "0 B/s");
        assert_eq!(format_rate(-5.0), "0 B/s");
        assert_eq!(format_rate(1_500_000.0), "1.4 MB/s");
    }

    #[test]
    fn percent_formatting_keeps_small_values_precise() {
        assert_eq!(format_percent(0.0), "0%");
        assert_eq!(format_percent(4.25), "4.2%");
        assert_eq!(format_percent(37.6), "38%");
        assert_eq!(format_percent(100.0), "100%");
    }

    #[cfg(windows)]
    #[test]
    fn sampler_reads_real_windows_counters_without_panicking() {
        let mut sampler = Sampler::new();
        let first = sampler.sample();
        assert!(first.memory.is_some(), "内存信息应可读取");
        assert!(first.disk.is_some(), "系统盘容量应可读取");
        std::thread::sleep(std::time::Duration::from_millis(120));
        let second = sampler.sample();
        assert!(second.cpu.is_some(), "第二次采样应能算出 CPU 使用率");
        assert!(second.network.is_some(), "第二次采样应能算出网络速率");
        if let Some((used, total)) = second.memory {
            assert!(used <= total);
        }
    }

    #[test]
    fn history_keeps_a_bounded_window_and_seeds_demo_data() {
        let mut history = History::default();
        for index in 0..(History::LEN + 12) {
            history.push(Some(index as f32), Some(index as f64 * 100.0), Some(0.0));
        }
        assert_eq!(history.cpu.len(), History::LEN);
        assert_eq!(history.down.len(), History::LEN);
        assert_eq!(*history.cpu.back().unwrap(), (History::LEN + 11) as f32);
        history.seed((0..10).map(|index| (index as f32, 1024.0, 512.0)));
        assert_eq!(history.cpu.len(), 10);
        assert_eq!(*history.down.back().unwrap(), 1024.0);
    }
}
