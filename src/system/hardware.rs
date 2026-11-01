use serde::{Deserialize, Serialize};
#[cfg(any(target_os = "macos", target_os = "windows"))]
use std::process::Command;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HardwareTier {
    /// Up to ~10 GB RAM (e.g. 8 GB base laptops)
    Tier8GB,
    /// ~11 GB to 20 GB RAM (e.g. 16 GB developer laptops)
    Tier16GB,
    /// ~21 GB to 40 GB RAM (e.g. 24 GB - 36 GB Pro/Max laptops)
    Tier32GB,
    /// > 40 GB RAM (e.g. 64 GB+ workstations)
    Tier64GBPlus,
}

impl HardwareTier {
    pub fn from_bytes(bytes: u64) -> Self {
        let gib = bytes / (1024 * 1024 * 1024);
        if gib <= 10 {
            HardwareTier::Tier8GB
        } else if gib <= 20 {
            HardwareTier::Tier16GB
        } else if gib <= 40 {
            HardwareTier::Tier32GB
        } else {
            HardwareTier::Tier64GBPlus
        }
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            HardwareTier::Tier8GB => "8 GB Tier (Lightweight / Thin-and-Light)",
            HardwareTier::Tier16GB => "16 GB Tier (Standard Developer Laptop)",
            HardwareTier::Tier32GB => "24–36 GB Tier (High-End Pro / Max Laptop)",
            HardwareTier::Tier64GBPlus => "64 GB+ Tier (Workstation / Studio)",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HardwareProfile {
    pub total_memory_bytes: u64,
    pub total_memory_gb: f64,
    pub chip_name: String,
    pub tier: HardwareTier,
    pub recommended_model: &'static str,
    pub recommended_quant: &'static str,
    pub recommended_context: usize,
    pub recommended_launch_command: String,
    pub ollama_launch_command: String,
    pub ram_footprint: &'static str,
}

impl HardwareProfile {
    /// Detects host hardware memory and processor characteristics, mapping to the optimal Gemma 4 configuration.
    pub fn detect() -> Self {
        let total_memory_bytes = detect_total_memory().unwrap_or(16 * 1024 * 1024 * 1024);
        let chip_name = detect_chip_name().unwrap_or_else(|| "Standard CPU".to_string());
        Self::from_specs(total_memory_bytes, chip_name)
    }

    pub fn from_specs(total_memory_bytes: u64, chip_name: String) -> Self {
        let total_memory_gb = total_memory_bytes as f64 / (1024.0 * 1024.0 * 1024.0);
        let tier = HardwareTier::from_bytes(total_memory_bytes);

        let (
            recommended_model,
            recommended_quant,
            recommended_context,
            model_file,
            ollama_model,
            ram_footprint,
        ) = match tier {
            HardwareTier::Tier8GB => (
                "ggml-org/gemma-4-E4B-it-GGUF:Q8_0",
                "Q8_0",
                8_192,
                "gemma-4-E4B-it-Q8_0.gguf",
                "gemma2:9b",
                "~5.0 GB total",
            ),
            HardwareTier::Tier16GB => (
                "ggml-org/gemma-4-E4B-it-GGUF:Q8_0",
                "Q8_0",
                32_768,
                "gemma-4-E4B-it-Q8_0.gguf",
                "gemma2:9b",
                "~6.5 GB total",
            ),
            HardwareTier::Tier32GB => (
                "ggml-org/gemma-4-E4B-it-GGUF:Q8_0",
                "Q8_0",
                65_536,
                "gemma-4-E4B-it-Q8_0.gguf",
                "gemma2:9b",
                "~8.5 GB total",
            ),
            HardwareTier::Tier64GBPlus => (
                "ggml-org/gemma-4-26B-it-GGUF:Q8_0",
                "Q8_0",
                131_072,
                "gemma-4-26B-it-Q8_0.gguf",
                "gemma2:27b",
                "~28–32 GB total",
            ),
        };

        let recommended_launch_command = format!(
            "llama-server -m {} -c {} --port 9931",
            model_file, recommended_context
        );
        let ollama_launch_command = format!(
            "ollama run {} --ctx-size {}",
            ollama_model, recommended_context
        );

        Self {
            total_memory_bytes,
            total_memory_gb,
            chip_name,
            tier,
            recommended_model,
            recommended_quant,
            recommended_context,
            recommended_launch_command,
            ollama_launch_command,
            ram_footprint,
        }
    }

    /// Returns the approximate single-pass token ceiling (~70% of context window) before chunking kicks in.
    pub fn one_pass_token_ceiling(&self) -> usize {
        self.recommended_context * 7 / 10
    }
}

fn detect_total_memory() -> Option<u64> {
    #[cfg(target_os = "macos")]
    {
        if let Ok(output) = Command::new("sysctl").args(["-n", "hw.memsize"]).output()
            && output.status.success()
            && let Ok(text) = std::str::from_utf8(&output.stdout)
            && let Ok(bytes) = text.trim().parse::<u64>()
        {
            return Some(bytes);
        }
    }

    #[cfg(target_os = "linux")]
    {
        if let Ok(content) = std::fs::read_to_string("/proc/meminfo") {
            for line in content.lines() {
                if let Some(rest) = line.strip_prefix("MemTotal:") {
                    let parts: Vec<&str> = rest.split_whitespace().collect();
                    if let Some(kb_str) = parts.first()
                        && let Ok(kb) = kb_str.parse::<u64>()
                    {
                        return Some(kb * 1024);
                    }
                }
            }
        }
    }

    #[cfg(target_os = "windows")]
    {
        if let Ok(output) = Command::new("powershell")
            .args([
                "-NoProfile",
                "-Command",
                "(Get-CimInstance Win32_PhysicalMemory | Measure-Object -Property Capacity -Sum).Sum",
            ])
            .output()
            && output.status.success()
            && let Ok(text) = std::str::from_utf8(&output.stdout)
            && let Ok(bytes) = text.trim().parse::<u64>()
        {
            return Some(bytes);
        }
    }

    None
}

fn detect_chip_name() -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        if let Ok(output) = Command::new("sysctl")
            .args(["-n", "machdep.cpu.brand_string"])
            .output()
            && output.status.success()
            && let Ok(text) = std::str::from_utf8(&output.stdout)
        {
            let clean = text.trim();
            if !clean.is_empty() {
                return Some(clean.to_string());
            }
        }
    }

    #[cfg(target_os = "linux")]
    {
        if let Ok(content) = std::fs::read_to_string("/proc/cpuinfo") {
            for line in content.lines() {
                if let Some(rest) = line.strip_prefix("model name")
                    && let Some(val) = rest.split(':').nth(1)
                {
                    let clean = val.trim();
                    if !clean.is_empty() {
                        return Some(clean.to_string());
                    }
                }
            }
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hardware_tier_classification() {
        assert_eq!(
            HardwareTier::from_bytes(8 * 1024 * 1024 * 1024),
            HardwareTier::Tier8GB
        );
        assert_eq!(
            HardwareTier::from_bytes(16 * 1024 * 1024 * 1024),
            HardwareTier::Tier16GB
        );
        assert_eq!(
            HardwareTier::from_bytes(24 * 1024 * 1024 * 1024),
            HardwareTier::Tier32GB
        );
        assert_eq!(
            HardwareTier::from_bytes(64 * 1024 * 1024 * 1024),
            HardwareTier::Tier64GBPlus
        );
    }

    #[test]
    fn test_profile_from_specs() {
        let profile = HardwareProfile::from_specs(16 * 1024 * 1024 * 1024, "Apple M3".to_string());
        assert_eq!(profile.tier, HardwareTier::Tier16GB);
        assert_eq!(profile.recommended_quant, "Q8_0");
        assert_eq!(profile.recommended_context, 32_768);
        assert_eq!(profile.ram_footprint, "~6.5 GB total");
        assert!(profile.recommended_launch_command.contains("-c 32768"));
        assert!(profile.recommended_launch_command.contains("gemma-4-E4B"));
        assert!(
            profile
                .ollama_launch_command
                .contains("ollama run gemma2:9b --ctx-size 32768")
        );
        assert_eq!(profile.one_pass_token_ceiling(), 32_768 * 7 / 10);
    }

    #[test]
    fn test_hardware_detect_runs_without_panic() {
        let profile = HardwareProfile::detect();
        assert!(profile.total_memory_bytes > 0);
        assert!(!profile.chip_name.is_empty());
    }
}
