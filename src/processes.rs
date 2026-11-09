//! 进程排行：按 CPU / 内存列出占用最高的进程，供「进程」页展示。
//!
//! 采样在后台线程进行（首次建立基线，之后按间隔计算 CPU 差值），
//! 只在用户打开进程页时运行；页面收起后服务结束，不做常驻轮询。

use std::sync::mpsc::{channel, Receiver, Sender};

pub const TOP: usize = 8;
const SAMPLE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(2);

#[derive(Clone, Debug, PartialEq)]
pub struct Process {
    pub name: String,
    pub pid: u32,
    /// 占整机 CPU 的百分比（0–100，单进程最多约等于 100）。
    pub cpu: f32,
    /// 工作集内存（字节）。
    pub memory: u64,
}

#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub items: Vec<Process>,
    /// 采样到的进程总数。
    pub total: usize,
    pub cores: u32,
}

/// 排序并截断：先 CPU 后内存，同分按名称稳定排序。
pub fn rank(mut items: Vec<Process>, top: usize) -> Vec<Process> {
    items.sort_by(|a, b| {
        b.cpu
            .partial_cmp(&a.cpu)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| b.memory.cmp(&a.memory))
            .then_with(|| a.name.cmp(&b.name))
    });
    items.truncate(top);
    items
}

pub fn format_memory(bytes: u64) -> String {
    const MB: f64 = 1024.0 * 1024.0;
    if bytes as f64 >= 1024.0 * MB {
        format!("{:.1} GB", bytes as f64 / (1024.0 * MB))
    } else {
        format!("{:.0} MB", bytes as f64 / MB)
    }
}

pub fn format_cpu(cpu: f32) -> String {
    if cpu >= 9.95 {
        format!("{cpu:.0}%")
    } else if cpu >= 0.05 {
        format!("{cpu:.1}%")
    } else {
        "0%".to_string()
    }
}

pub struct Service {
    events: Receiver<Snapshot>,
}

impl Service {
    pub fn start() -> Self {
        let (events, receiver) = channel();
        std::thread::Builder::new()
            .name("daozai-processes".into())
            .spawn(move || sampler_loop(events))
            .ok();
        Self { events: receiver }
    }

    pub fn try_recv(&self) -> Option<Snapshot> {
        self.events.try_recv().ok()
    }
}

fn sampler_loop(events: Sender<Snapshot>) {
    let mut sampler = Sampler::new();
    // 先建立 CPU 时间基线，再隔一秒采样一次。
    sampler.sample();
    std::thread::sleep(std::time::Duration::from_millis(900));
    loop {
        let snapshot = sampler.sample();
        if events.send(snapshot).is_err() {
            return;
        }
        std::thread::sleep(SAMPLE_INTERVAL);
    }
}

#[derive(Default)]
pub struct Sampler {
    previous: std::collections::HashMap<u32, u64>,
    last: Option<std::time::Instant>,
    cores: u32,
}

impl Sampler {
    pub fn new() -> Self {
        Self {
            previous: std::collections::HashMap::new(),
            last: None,
            cores: std::thread::available_parallelism().map(|n| n.get() as u32).unwrap_or(1).max(1),
        }
    }

    #[cfg(windows)]
    pub fn sample(&mut self) -> Snapshot {
        use windows::Win32::Foundation::CloseHandle;
        use windows::Win32::System::Diagnostics::ToolHelp::{
            CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
            TH32CS_SNAPPROCESS,
        };
        use windows::Win32::System::ProcessStatus::{K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
        use windows::Win32::System::Threading::{
            GetProcessTimes, OpenProcess, PROCESS_QUERY_INFORMATION, PROCESS_VM_READ,
        };
        let now = std::time::Instant::now();
        let elapsed = self
            .last
            .map(|last| (now - last).as_secs_f64().max(0.05))
            .unwrap_or(0.0);
        self.last = Some(now);
        let mut current = std::collections::HashMap::new();
        let mut items = Vec::new();
        unsafe {
            let Ok(snapshot) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else {
                return Snapshot::default();
            };
            let mut entry = PROCESSENTRY32W {
                dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
                ..Default::default()
            };
            if Process32FirstW(snapshot, &mut entry).is_ok() {
                loop {
                    let name = String::from_utf16_lossy(
                        &entry.szExeFile[..entry
                            .szExeFile
                            .iter()
                            .position(|ch| *ch == 0)
                            .unwrap_or(entry.szExeFile.len())],
                    );
                    let pid = entry.th32ProcessID;
                    if pid != 0 && !name.is_empty() {
                        let mut cpu = 0.0f32;
                        let mut memory = 0u64;
                        if let Ok(process) = OpenProcess(
                            PROCESS_QUERY_INFORMATION | PROCESS_VM_READ,
                            false,
                            pid,
                        ) {
                            let mut creation = windows::Win32::Foundation::FILETIME::default();
                            let mut exit = windows::Win32::Foundation::FILETIME::default();
                            let mut kernel = windows::Win32::Foundation::FILETIME::default();
                            let mut user = windows::Win32::Foundation::FILETIME::default();
                            if GetProcessTimes(
                                process,
                                &mut creation,
                                &mut exit,
                                &mut kernel,
                                &mut user,
                            )
                            .is_ok()
                            {
                                let total = filetime_100ns(kernel) + filetime_100ns(user);
                                current.insert(pid, total);
                                if let Some(previous) = self.previous.get(&pid) {
                                    let delta = total.saturating_sub(*previous);
                                    let seconds = delta as f64 / 10_000_000.0;
                                    cpu = ((seconds / elapsed) * 100.0 / self.cores as f64) as f32;
                                }
                            }
                            let mut counters = PROCESS_MEMORY_COUNTERS {
                                cb: std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
                                ..Default::default()
                            };
                            let _ = K32GetProcessMemoryInfo(
                                process,
                                &mut counters,
                                std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
                            );
                            memory = counters.WorkingSetSize as u64;
                            let _ = CloseHandle(process);
                        }
                        items.push(Process {
                            name,
                            pid,
                            cpu: cpu.clamp(0.0, 100.0),
                            memory,
                        });
                    }
                    if Process32NextW(snapshot, &mut entry).is_err() {
                        break;
                    }
                }
            }
            let _ = CloseHandle(snapshot);
        }
        let total = items.len();
        self.previous = current;
        Snapshot {
            items: rank(items, TOP),
            total,
            cores: self.cores,
        }
    }

    #[cfg(not(windows))]
    pub fn sample(&mut self) -> Snapshot {
        Snapshot::default()
    }
}

#[cfg(windows)]
fn filetime_100ns(time: windows::Win32::Foundation::FILETIME) -> u64 {
    ((time.dwHighDateTime as u64) << 32) | time.dwLowDateTime as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn process(name: &str, pid: u32, cpu: f32, memory: u64) -> Process {
        Process { name: name.into(), pid, cpu, memory }
    }

    #[test]
    fn ranking_sorts_by_cpu_then_memory_and_truncates() {
        let items = vec![
            process("idle", 1, 0.0, 1),
            process("game", 2, 42.5, 900),
            process("browser", 3, 42.5, 1200),
            process("music", 4, 3.0, 100),
        ];
        let ranked = rank(items, 3);
        assert_eq!(ranked.len(), 3);
        assert_eq!(ranked[0].name, "browser", "CPU 相同时内存高的在前");
        assert_eq!(ranked[1].name, "game");
        assert_eq!(ranked[2].name, "music");
        assert_eq!(rank(vec![process("a", 1, 1.0, 1)], 0).len(), 0);
    }

    #[test]
    fn formatting_cpu_and_memory_is_compact() {
        assert_eq!(format_cpu(0.0), "0%");
        assert_eq!(format_cpu(0.04), "0%");
        assert_eq!(format_cpu(0.25), "0.2%");
        assert_eq!(format_cpu(12.6), "13%");
        assert_eq!(format_memory(512 * 1024 * 1024), "512 MB");
        assert_eq!(format_memory(2 * 1024 * 1024 * 1024), "2.0 GB");
    }

    #[cfg(windows)]
    #[test]
    fn real_sampling_lists_processes_and_sorts_by_cpu() {
        let mut sampler = Sampler::new();
        sampler.sample();
        std::thread::sleep(std::time::Duration::from_millis(250));
        let snapshot = sampler.sample();
        assert!(snapshot.total > 5, "应能枚举到多个进程：{}", snapshot.total);
        assert!(!snapshot.items.is_empty());
        assert!(snapshot.items.len() <= TOP);
        for pair in snapshot.items.windows(2) {
            assert!(pair[0].cpu >= pair[1].cpu || pair[0].memory >= pair[1].memory);
        }
        assert!(snapshot.items.iter().any(|process| !process.name.is_empty()));
    }
}
