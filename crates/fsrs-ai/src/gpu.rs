//! Discovery of the local accelerators and selection of the inference backend.
//!
//! The decision model must run on the fastest accelerator available, without
//! competing with an unrelated service for the device. This module inspects
//! the machine, reports which accelerators exist, which processes hold them,
//! and which backend the runtime should use for a given preference order.

use std::process::Command;

use fsrs_core::GpuPreference;

/// Kind of accelerator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Accelerator {
    /// Discrete NVIDIA GPU.
    Nvidia,
    /// Integrated Intel GPU.
    Intel,
    /// Plain CPU execution.
    Cpu,
}

impl Accelerator {
    /// Human readable name.
    pub fn label(self) -> &'static str {
        match self {
            Accelerator::Nvidia => "NVIDIA GPU",
            Accelerator::Intel => "Intel GPU",
            Accelerator::Cpu => "CPU",
        }
    }
}

/// Inference backend the Foq runtime should use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Backend {
    /// CUDA backend of the shipped runtime.
    Cuda,
    /// Vulkan backend, requires a Vulkan build of the runtime.
    Vulkan,
    /// CPU kernels only.
    Cpu,
}

impl Backend {
    /// Number of layers offloaded to the accelerator.
    ///
    /// `99` means "all layers"; the CPU backend keeps every layer on the CPU.
    pub fn gpu_layers(self) -> i64 {
        match self {
            Backend::Cuda | Backend::Vulkan => 99,
            Backend::Cpu => 0,
        }
    }

    /// Value of `CUDA_VISIBLE_DEVICES` for this backend.
    ///
    /// `None` means the variable must not be set, so that every visible device
    /// stays visible to the runtime.
    pub fn visible_devices(self) -> Option<&'static str> {
        match self {
            Backend::Cuda => Some("0"),
            Backend::Vulkan | Backend::Cpu => None,
        }
    }

    /// Human readable name.
    pub fn label(self) -> &'static str {
        match self {
            Backend::Cuda => "CUDA",
            Backend::Vulkan => "Vulkan",
            Backend::Cpu => "CPU",
        }
    }
}

/// One accelerator found on the machine.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AcceleratorInfo {
    /// Kind of the accelerator.
    pub kind: Accelerator,
    /// Index inside its own family.
    pub index: usize,
    /// Device name as reported by the driver.
    pub name: String,
    /// Total memory in megabytes, `0` when unknown.
    pub memory_total_mb: u64,
}

/// A process that currently holds the accelerator.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AcceleratorProcess {
    /// Process id.
    pub pid: u32,
    /// Process name, when the driver reported one.
    pub name: String,
    /// Memory used by the process, in megabytes.
    pub used_memory_mb: u64,
}

/// Complete picture of the accelerators of the machine.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AcceleratorInventory {
    /// NVIDIA devices, in driver order.
    pub nvidia: Vec<AcceleratorInfo>,
    /// Intel devices.
    pub intel: Vec<AcceleratorInfo>,
    /// Processes currently holding an NVIDIA device.
    pub busy_processes: Vec<AcceleratorProcess>,
}

impl AcceleratorInventory {
    /// All accelerators, NVIDIA first.
    pub fn all(&self) -> Vec<&AcceleratorInfo> {
        self.nvidia.iter().chain(self.intel.iter()).collect()
    }

    /// The preferred NVIDIA device, if any.
    pub fn primary_nvidia(&self) -> Option<&AcceleratorInfo> {
        self.nvidia.first()
    }

    /// Whether the accelerator is held by another process.
    ///
    /// The decision model is the only workload allowed to use the GPU, so the
    /// dashboard refuses to start it while a foreign process holds the device.
    pub fn is_busy(&self) -> bool {
        !self.busy_processes.is_empty()
    }

    /// A short description of the processes holding the accelerator.
    pub fn busy_summary(&self) -> String {
        if self.busy_processes.is_empty() {
            return "no process holds the GPU".to_string();
        }
        self.busy_processes
            .iter()
            .map(|process| {
                format!(
                    "pid {} ({}, {} MiB)",
                    process.pid, process.name, process.used_memory_mb
                )
            })
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// Choose the backend that satisfies the preference order.
    ///
    /// The first preference that matches an available accelerator wins; the
    /// CPU is used when no accelerator is available.
    pub fn select_backend(&self, preference: &[GpuPreference]) -> Backend {
        for wanted in preference {
            match wanted {
                GpuPreference::Nvidia if !self.nvidia.is_empty() => return Backend::Cuda,
                GpuPreference::Intel if !self.intel.is_empty() => return Backend::Vulkan,
                GpuPreference::Cpu => return Backend::Cpu,
                _ => continue,
            }
        }
        Backend::Cpu
    }
}

/// Inspect the machine for accelerators and for processes holding them.
pub fn detect() -> AcceleratorInventory {
    AcceleratorInventory {
        nvidia: detect_nvidia(),
        intel: detect_intel(),
        busy_processes: detect_nvidia_processes(),
    }
}

/// Query the NVIDIA driver for the list of devices.
fn detect_nvidia() -> Vec<AcceleratorInfo> {
    let output = Command::new("nvidia-smi")
        .args([
            "--query-gpu=index,name,memory.total",
            "--format=csv,noheader,nounits",
        ])
        .output();
    let Ok(output) = output else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let mut fields = line.split(',').map(|f| f.trim());
            let index: usize = fields.next()?.parse().ok()?;
            let name = fields.next().unwrap_or("NVIDIA GPU").to_string();
            let memory_total_mb = fields
                .next()
                .and_then(|value| value.parse::<u64>().ok())
                .unwrap_or(0);
            Some(AcceleratorInfo {
                kind: Accelerator::Nvidia,
                index,
                name,
                memory_total_mb,
            })
        })
        .collect()
}

/// Query the NVIDIA driver for the processes holding a device.
fn detect_nvidia_processes() -> Vec<AcceleratorProcess> {
    let output = Command::new("nvidia-smi")
        .args([
            "--query-compute-apps=pid,process_name,used_memory",
            "--format=csv,noheader,nounits",
        ])
        .output();
    let Ok(output) = output else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let mut fields = line.split(',').map(|f| f.trim());
            let pid: u32 = fields.next()?.parse().ok()?;
            let name = fields.next().unwrap_or("unknown").to_string();
            let used_memory_mb = fields
                .next()
                .and_then(|value| value.parse::<u64>().ok())
                .unwrap_or(0);
            Some(AcceleratorProcess {
                pid,
                name,
                used_memory_mb,
            })
        })
        .collect()
}

/// Detect Intel integrated GPUs through the DRM render nodes.
fn detect_intel() -> Vec<AcceleratorInfo> {
    let Ok(entries) = std::fs::read_dir("/dev/dri") else {
        return Vec::new();
    };
    let mut indices: Vec<usize> = entries
        .filter_map(|entry| {
            let name = entry.ok()?.file_name().to_string_lossy().to_string();
            let index = name.strip_prefix("renderD")?.parse::<usize>().ok()?;
            Some(index)
        })
        .collect();
    indices.sort_unstable();
    indices
        .into_iter()
        .enumerate()
        .map(|(index, _)| AcceleratorInfo {
            kind: Accelerator::Intel,
            index,
            name: "Intel integrated GPU".to_string(),
            memory_total_mb: 0,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inventory(nvidia: usize, intel: usize) -> AcceleratorInventory {
        AcceleratorInventory {
            nvidia: (0..nvidia)
                .map(|index| AcceleratorInfo {
                    kind: Accelerator::Nvidia,
                    index,
                    name: "Quadro M4000M".to_string(),
                    memory_total_mb: 4096,
                })
                .collect(),
            intel: (0..intel)
                .map(|index| AcceleratorInfo {
                    kind: Accelerator::Intel,
                    index,
                    name: "Intel HD 530".to_string(),
                    memory_total_mb: 0,
                })
                .collect(),
            busy_processes: Vec::new(),
        }
    }

    #[test]
    fn nvidia_is_preferred_over_intel_and_cpu() {
        let prefs = vec![
            GpuPreference::Nvidia,
            GpuPreference::Intel,
            GpuPreference::Cpu,
        ];
        assert_eq!(inventory(1, 1).select_backend(&prefs), Backend::Cuda);
    }

    #[test]
    fn intel_is_used_when_no_nvidia_device_is_present() {
        let prefs = vec![
            GpuPreference::Nvidia,
            GpuPreference::Intel,
            GpuPreference::Cpu,
        ];
        assert_eq!(inventory(0, 1).select_backend(&prefs), Backend::Vulkan);
    }

    #[test]
    fn cpu_is_the_last_resort() {
        let prefs = vec![
            GpuPreference::Nvidia,
            GpuPreference::Intel,
            GpuPreference::Cpu,
        ];
        assert_eq!(inventory(0, 0).select_backend(&prefs), Backend::Cpu);
    }

    #[test]
    fn an_empty_preference_list_falls_back_to_the_cpu() {
        assert_eq!(inventory(1, 1).select_backend(&[]), Backend::Cpu);
    }

    #[test]
    fn the_cuda_backend_only_exposes_the_first_device() {
        assert_eq!(Backend::Cuda.visible_devices(), Some("0"));
        assert_eq!(Backend::Vulkan.visible_devices(), None);
        assert_eq!(Backend::Cpu.visible_devices(), None);
    }

    #[test]
    fn the_cpu_backend_keeps_every_layer_on_the_cpu() {
        assert_eq!(Backend::Cpu.gpu_layers(), 0);
        assert_eq!(Backend::Cuda.gpu_layers(), 99);
    }

    #[test]
    fn a_busy_accelerator_is_reported() {
        let mut inv = inventory(1, 0);
        assert!(!inv.is_busy());
        inv.busy_processes.push(AcceleratorProcess {
            pid: 42,
            name: "llama-server".to_string(),
            used_memory_mb: 3000,
        });
        assert!(inv.is_busy());
        assert!(inv.busy_summary().contains("pid 42"));
    }

    #[test]
    fn the_primary_nvidia_device_is_the_first_one() {
        let inv = inventory(2, 0);
        assert_eq!(inv.primary_nvidia().unwrap().index, 0);
        assert_eq!(inv.all().len(), 2);
    }
}
