//! "PC details" for problem reports: OS and version, CPU, memory, graphics
//! adapter and backend, display scale. Never the host name, user name or
//! any path.

use std::sync::OnceLock;

/// The graphics adapter eframe picked (from wgpu's `AdapterInfo`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GpuInfo {
    pub name: String,
    pub backend: String,
    pub device_type: String,
    pub driver: String,
}

impl GpuInfo {
    pub fn from_wgpu(i: &wgpu::AdapterInfo) -> GpuInfo {
        let driver = [i.driver.trim(), i.driver_info.trim()]
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        GpuInfo {
            name: i.name.clone(),
            backend: format!("{:?}", i.backend),
            device_type: match i.device_type {
                wgpu::DeviceType::DiscreteGpu => "discrete",
                wgpu::DeviceType::IntegratedGpu => "integrated",
                wgpu::DeviceType::VirtualGpu => "virtual",
                wgpu::DeviceType::Cpu => "software",
                wgpu::DeviceType::Other => "other",
            }
            .to_string(),
            driver,
        }
    }
}

/// Machine facts that don't change while the app runs.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Machine {
    pub os: String,
    pub kernel: Option<String>,
    pub arch: String,
    pub cpu: Option<String>,
    pub threads: Option<usize>,
    pub memory_bytes: Option<u64>,
}

impl Machine {
    /// Read once (CPU and memory queries take a moment).
    pub fn get() -> &'static Machine {
        static M: OnceLock<Machine> = OnceLock::new();
        M.get_or_init(|| {
            let mut sys = sysinfo::System::new();
            sys.refresh_memory();
            sys.refresh_cpu_list(sysinfo::CpuRefreshKind::nothing());
            let cpu = sys
                .cpus()
                .first()
                .map(|c| c.brand().trim().to_string())
                .filter(|b| !b.is_empty());
            Machine {
                os: sysinfo::System::long_os_version()
                    .unwrap_or_else(|| platform_short().to_string()),
                kernel: sysinfo::System::kernel_version(),
                arch: std::env::consts::ARCH.to_string(),
                cpu,
                threads: std::thread::available_parallelism().ok().map(|n| n.get()),
                memory_bytes: Some(sys.total_memory()).filter(|m| *m > 0),
            }
        })
    }
}

/// "Linux", "Windows" or "macOS".
pub fn platform_short() -> &'static str {
    match std::env::consts::OS {
        "linux" => "Linux",
        "windows" => "Windows",
        "macos" => "macOS",
        other => other,
    }
}

/// How the window is scaled.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Display {
    /// egui points to pixels (OS scale x app UI scale).
    pub pixels_per_point: f32,
    /// What the OS reports for the window.
    pub native: Option<f32>,
    pub ui_scale: f32,
}

/// Everything "PC details" adds, as `system.txt`.
pub fn system_text(m: &Machine, gpu: Option<&GpuInfo>, d: Display) -> String {
    let mut s = String::new();
    s.push_str(&format!("OS: {}\n", m.os));
    if let Some(k) = &m.kernel {
        s.push_str(&format!("Kernel: {k}\n"));
    }
    s.push_str(&format!("Architecture: {}\n", m.arch));
    match (&m.cpu, m.threads) {
        (Some(c), Some(t)) => s.push_str(&format!("CPU: {c} ({t} threads)\n")),
        (Some(c), None) => s.push_str(&format!("CPU: {c}\n")),
        (None, Some(t)) => s.push_str(&format!("CPU: {t} threads\n")),
        (None, None) => {}
    }
    if let Some(b) = m.memory_bytes {
        s.push_str(&format!("Memory: {}\n", memory(b)));
    }
    match gpu {
        Some(g) => {
            s.push_str(&format!(
                "Graphics: {} ({}, {})\n",
                g.name, g.backend, g.device_type
            ));
            if !g.driver.is_empty() {
                s.push_str(&format!("Graphics driver: {}\n", g.driver));
            }
        }
        None => s.push_str("Graphics: not available (the viewport has no GPU)\n"),
    }
    if d.pixels_per_point > 0.0 {
        s.push_str(&format!(
            "Display scale: {:.2} (OS {}, app UI scale {:.2})\n",
            d.pixels_per_point,
            d.native
                .map(|n| format!("{n:.2}"))
                .unwrap_or_else(|| "unknown".into()),
            d.ui_scale
        ));
    }
    super::scrub::scrub(&s)
}

/// One line for the dialog: "Windows 11 · RTX 4070 · 32 GB".
pub fn summary(m: &Machine, gpu: Option<&GpuInfo>) -> String {
    let mut parts = vec![m.os.clone()];
    if let Some(g) = gpu {
        parts.push(g.name.clone());
    }
    if let Some(b) = m.memory_bytes {
        parts.push(memory(b));
    }
    super::scrub::scrub(&parts.join(" · "))
}

/// Installed memory, rounded to whole GB.
pub fn memory(bytes: u64) -> String {
    let gb = bytes as f64 / (1024.0 * 1024.0 * 1024.0);
    if gb >= 1.0 {
        format!("{} GB", gb.round() as u64)
    } else {
        format!("{} MB", bytes / (1024 * 1024))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn machine() -> Machine {
        Machine {
            os: "Windows 11 (26100)".into(),
            kernel: Some("26100".into()),
            arch: "x86_64".into(),
            cpu: Some("AMD Ryzen 7 7800X3D".into()),
            threads: Some(16),
            memory_bytes: Some(32 * 1024 * 1024 * 1024 - 300 * 1024 * 1024),
        }
    }

    #[test]
    fn text_lists_the_machine_without_names() {
        let gpu = GpuInfo {
            name: "NVIDIA GeForce RTX 4070".into(),
            backend: "Vulkan".into(),
            device_type: "discrete".into(),
            driver: "NVIDIA 560.35".into(),
        };
        let t = system_text(
            &machine(),
            Some(&gpu),
            Display {
                pixels_per_point: 1.25,
                native: Some(1.25),
                ui_scale: 1.0,
            },
        );
        assert!(t.contains("OS: Windows 11 (26100)"));
        assert!(t.contains("CPU: AMD Ryzen 7 7800X3D (16 threads)"));
        assert!(t.contains("Memory: 32 GB"));
        assert!(t.contains("Graphics: NVIDIA GeForce RTX 4070 (Vulkan, discrete)"));
        assert!(t.contains("Display scale: 1.25"));
        assert_eq!(
            summary(&machine(), Some(&gpu)),
            "Windows 11 (26100) · NVIDIA GeForce RTX 4070 · 32 GB"
        );
    }

    #[test]
    fn this_machine_has_no_host_or_user_name() {
        let m = Machine::get();
        let t = system_text(m, None, Display::default());
        assert!(t.starts_with("OS: "));
        // The host name is never read; the user name must not appear.
        if let Ok(user) = std::env::var("USER").or_else(|_| std::env::var("USERNAME"))
            && user.len() >= 3
        {
            assert!(!t.contains(&user), "user name leaked");
        }
    }
}
