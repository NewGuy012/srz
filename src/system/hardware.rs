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
    pub concise_model: &'static str,
    pub recommended_size: &'static str,
    pub recommended_quant: &'static str,
    pub disk_size: &'static str,
    pub recommended_context: usize,
    pub recommended_context_display: &'static str,
    pub recommended_launch_command: String,
    pub ollama_launch_command: String,
    pub ram_footprint: &'static str,
}

static DETECTED_PROFILE: std::sync::LazyLock<HardwareProfile> = std::sync::LazyLock::new(|| {
    let total_memory_bytes = detect_total_memory().unwrap_or(16 * 1024 * 1024 * 1024);
    let chip_name = detect_chip_name().unwrap_or_else(|| "Standard CPU".to_string());
    HardwareProfile::from_specs(total_memory_bytes, chip_name)
});

impl HardwareProfile {
    /// Detects host hardware memory and processor characteristics, mapping to the optimal Gemma 4 configuration.
    /// Results are cached across calls to avoid spawning subprocesses (`sysctl`/`powershell`) multiple times.
    pub fn detect() -> Self {
        DETECTED_PROFILE.clone()
    }

    pub fn from_specs(total_memory_bytes: u64, chip_name: String) -> Self {
        let chip_name = crate::ui::sanitize_title(&chip_name);
        let total_memory_gb = total_memory_bytes as f64 / (1024.0 * 1024.0 * 1024.0);
        let tier = HardwareTier::from_bytes(total_memory_bytes);

        let (recommended_model, recommended_size, disk_size, model_file, ollama_model) =
            if matches!(tier, HardwareTier::Tier64GBPlus) {
                (
                    "ggml-org/gemma-4-26B-A4B-it-GGUF:Q8_0",
                    "26B-A4B",
                    "~28.0 GB",
                    "gemma-4-26B-A4B-it-Q8_0.gguf",
                    "gemma2:27b",
                )
            } else {
                (
                    "ggml-org/gemma-4-E4B-it-GGUF:Q8_0",
                    "E4B",
                    "~8.6 GB",
                    "gemma-4-E4B-it-Q8_0.gguf",
                    "gemma2:9b",
                )
            };
        let concise_model = "gemma-4";
        let recommended_quant = "Q8_0";
        let (recommended_context, recommended_context_display, ram_footprint) = match tier {
            HardwareTier::Tier8GB => (8_192, "8k ctx", "~8.9 GB"),
            HardwareTier::Tier16GB => (32_768, "32k ctx", "~9.4 GB"),
            HardwareTier::Tier32GB => (65_536, "64k ctx", "~10 GB"),
            HardwareTier::Tier64GBPlus => (131_072, "128k ctx", "~30–32 GB"),
        };

        let recommended_launch_command = format!(
            "llama-server -m {} --alias {} -c {} --port 9931",
            model_file, recommended_model, recommended_context
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
            concise_model,
            recommended_size,
            recommended_quant,
            disk_size,
            recommended_context,
            recommended_context_display,
            recommended_launch_command,
            ollama_launch_command,
            ram_footprint,
        }
    }

    /// Formatted selection string matching the simplified naming in Llama GUI (e.g. `gemma-4 | E4B | Q8_0 | 32k ctx`).
    pub fn llama_gui_selection(&self) -> String {
        format!(
            "{}  |  {}  |  {}  |  {}",
            self.concise_model,
            self.recommended_size,
            self.recommended_quant,
            self.recommended_context_display
        )
    }

    /// Returns the approximate single-pass token ceiling (~70% of context window) before chunking kicks in.
    pub fn one_pass_token_ceiling(&self) -> usize {
        self.recommended_context * 7 / 10
    }

    /// Serializes the hardware profile into a structured JSON representation for diagnostics and recommendations.
    pub fn to_json_value(&self) -> serde_json::Value {
        serde_json::json!({
            "device": self.chip_name,
            "total_memory_bytes": self.total_memory_bytes,
            "total_memory_gb": self.total_memory_gb,
            "tier": self.tier.display_name(),
            "recommended_family": "Gemma 4",
            "recommended_model": self.recommended_model,
            "concise_model": self.concise_model,
            "recommended_size": self.recommended_size,
            "recommended_quant": self.recommended_quant,
            "disk_size": self.disk_size,
            "recommended_context": self.recommended_context,
            "recommended_context_display": self.recommended_context_display,
            "recommended_launch_command": self.recommended_launch_command,
            "ollama_launch_command": self.ollama_launch_command,
            "ram_footprint": self.ram_footprint,
            "one_pass_token_ceiling": self.one_pass_token_ceiling(),
            "llama_gui_selection": self.llama_gui_selection(),
        })
    }

    /// Returns the structured recommendation report matching `srz recommend --json`.
    pub fn to_recommend_report_json(&self) -> serde_json::Value {
        serde_json::json!({
            "hardware": {
                "device": self.chip_name,
                "total_memory_bytes": self.total_memory_bytes,
                "total_memory_gb": self.total_memory_gb,
                "tier": self.tier.display_name(),
            },
            "recommendations": {
                "family": "Google Gemma 4",
                "model": self.concise_model,
                "model_id": self.recommended_model,
                "size": self.recommended_size,
                "quantization": self.recommended_quant,
                "disk_size": self.disk_size,
                "context_window": self.recommended_context,
                "context_display": self.recommended_context_display,
                "ram_footprint": self.ram_footprint,
                "one_pass_token_ceiling": self.one_pass_token_ceiling(),
            },
            "launch_commands": {
                "llama_gui": format!("In Llama GUI, select: {}", self.llama_gui_selection()),
                "llama_server": self.recommended_launch_command,
                "ollama": self.ollama_launch_command,
            }
        })
    }
}

/// Formats a raw token context window (e.g. 32768) into a simplified label (e.g. "32k ctx")
/// matching the Llama GUI display convention.
pub fn format_context_tokens(tokens: usize) -> String {
    if tokens >= 1024 && tokens.is_multiple_of(1024) {
        format!("{}k ctx", tokens / 1024)
    } else if tokens >= 1000 {
        format!("{}k ctx", tokens / 1000)
    } else {
        format!("{} ctx", tokens)
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

    #[cfg(target_os = "windows")]
    {
        if let Ok(brand) = std::env::var("PROCESSOR_IDENTIFIER") {
            let clean = brand.trim();
            if !clean.is_empty() {
                return Some(clean.to_string());
            }
        }
        if let Ok(output) = Command::new("powershell")
            .args([
                "-NoProfile",
                "-Command",
                "(Get-CimInstance Win32_Processor).Name",
            ])
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
        assert_eq!(
            profile.recommended_model,
            "ggml-org/gemma-4-E4B-it-GGUF:Q8_0"
        );
        assert_eq!(profile.concise_model, "gemma-4");
        assert_eq!(profile.recommended_size, "E4B");
        assert_eq!(profile.recommended_quant, "Q8_0");
        assert_eq!(profile.disk_size, "~8.6 GB");
        assert_eq!(profile.recommended_context, 32_768);
        assert_eq!(profile.recommended_context_display, "32k ctx");
        assert_eq!(profile.ram_footprint, "~9.4 GB");
        assert!(profile.recommended_launch_command.contains("-c 32768"));
        assert!(profile.recommended_launch_command.contains("gemma-4-E4B"));
        assert!(
            profile
                .ollama_launch_command
                .contains("ollama run gemma2:9b --ctx-size 32768")
        );
        assert_eq!(profile.one_pass_token_ceiling(), 32_768 * 7 / 10);
        assert_eq!(
            profile.llama_gui_selection(),
            "gemma-4  |  E4B  |  Q8_0  |  32k ctx"
        );
    }

    #[test]
    fn test_profile_64gb_specs() {
        let profile =
            HardwareProfile::from_specs(64 * 1024 * 1024 * 1024, "Apple M3 Max".to_string());
        assert_eq!(profile.tier, HardwareTier::Tier64GBPlus);
        assert_eq!(
            profile.recommended_model,
            "ggml-org/gemma-4-26B-A4B-it-GGUF:Q8_0"
        );
        assert_eq!(profile.concise_model, "gemma-4");
        assert_eq!(profile.recommended_size, "26B-A4B");
        assert_eq!(profile.recommended_quant, "Q8_0");
        assert_eq!(profile.disk_size, "~28.0 GB");
        assert_eq!(profile.recommended_context, 131_072);
        assert_eq!(profile.recommended_context_display, "128k ctx");
        assert!(profile.recommended_launch_command.contains("26B-A4B"));
        assert_eq!(
            profile.llama_gui_selection(),
            "gemma-4  |  26B-A4B  |  Q8_0  |  128k ctx"
        );
    }

    #[test]
    fn test_format_context_tokens() {
        assert_eq!(format_context_tokens(8192), "8k ctx");
        assert_eq!(format_context_tokens(32768), "32k ctx");
        assert_eq!(format_context_tokens(65536), "64k ctx");
        assert_eq!(format_context_tokens(131072), "128k ctx");
        assert_eq!(format_context_tokens(4000), "4k ctx");
        assert_eq!(format_context_tokens(512), "512 ctx");
    }

    #[test]
    fn test_hardware_detect_runs_without_panic() {
        let profile = HardwareProfile::detect();
        assert!(profile.total_memory_bytes > 0);
        assert!(!profile.chip_name.is_empty());
    }

    #[test]
    fn test_hardware_system_detection() {
        let mem = detect_total_memory();
        assert!(mem.is_some_and(|m| m > 0));
        let chip = detect_chip_name();
        assert!(chip.is_some_and(|c| !c.is_empty()));
    }
}
