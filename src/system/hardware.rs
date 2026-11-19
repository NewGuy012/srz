#[cfg(any(target_os = "macos", target_os = "windows"))]
use std::process::Command;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HardwareTier {
    Tier8GB,
    Tier16GB,
    Tier32GB,
    Tier64GBPlus,
}

impl HardwareTier {
    fn from_bytes(bytes: u64) -> Self {
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

#[derive(Debug, Clone)]
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
    let (total_memory_bytes, chip_name) = detect_host_specs();
    HardwareProfile::from_specs(
        total_memory_bytes.unwrap_or(16 * 1024 * 1024 * 1024),
        chip_name.unwrap_or_else(|| "Standard CPU".to_string()),
    )
});

impl HardwareProfile {
    /// Returns the cached host hardware profile without re-spawning OS probes.
    pub fn detect() -> &'static Self {
        &DETECTED_PROFILE
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
                    "gemma4:26b",
                )
            } else {
                (
                    "ggml-org/gemma-4-E4B-it-GGUF:Q8_0",
                    "E4B",
                    "~8.6 GB",
                    "gemma-4-E4B-it-Q8_0.gguf",
                    "gemma4:e4b",
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
            "OLLAMA_CONTEXT_LENGTH={} ollama run {}",
            recommended_context, ollama_model
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

    pub fn llama_gui_selection(&self) -> String {
        format!(
            "{}  |  {}  |  {}  |  {}",
            self.concise_model,
            self.recommended_size,
            self.recommended_quant,
            self.recommended_context_display
        )
    }

    fn one_pass_token_ceiling(&self) -> usize {
        self.recommended_context * 7 / 10
    }

    fn host_json(&self) -> serde_json::Value {
        serde_json::json!({
            "device": self.chip_name,
            "total_memory_bytes": self.total_memory_bytes,
            "total_memory_gb": self.total_memory_gb,
            "tier": self.tier.display_name(),
        })
    }

    pub(crate) fn to_json_value(&self) -> serde_json::Value {
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

    pub(crate) fn to_recommend_report_json(&self) -> serde_json::Value {
        serde_json::json!({
            "hardware": self.host_json(),
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

/// Formats a token count as a compact display label (e.g. `"32k ctx"`).
pub(crate) fn format_context_tokens(tokens: usize) -> String {
    if tokens >= 1024 && tokens.is_multiple_of(1024) {
        format!("{}k ctx", tokens / 1024)
    } else if tokens >= 1000 {
        format!("{}k ctx", tokens / 1000)
    } else {
        format!("{} ctx", tokens)
    }
}

#[cfg(target_os = "macos")]
fn sysctl_value(key: &str) -> Option<String> {
    let output = Command::new("/usr/sbin/sysctl")
        .args(["-n", key])
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
}

fn detect_host_specs() -> (Option<u64>, Option<String>) {
    #[cfg(target_os = "macos")]
    {
        let mem = sysctl_value("hw.memsize").and_then(|s| s.parse::<u64>().ok());
        let chip = sysctl_value("machdep.cpu.brand_string").or_else(|| sysctl_value("hw.model"));
        (mem, chip)
    }

    #[cfg(target_os = "linux")]
    {
        let mem = std::fs::read_to_string("/proc/meminfo").ok().and_then(|c| {
            c.lines().find_map(|line| {
                line.strip_prefix("MemTotal:")?
                    .split_whitespace()
                    .next()?
                    .parse::<u64>()
                    .ok()?
                    .checked_mul(1024)
            })
        });

        let chip = std::fs::read_to_string("/proc/cpuinfo")
            .ok()
            .and_then(|c| {
                c.lines().find_map(|line| {
                    ["model name", "Hardware", "Processor"]
                        .iter()
                        .any(|p| line.starts_with(p))
                        .then(|| {
                            line.split(':')
                                .nth(1)
                                .map(|v| v.trim().to_string())
                                .filter(|v| !v.is_empty())
                        })
                        .flatten()
                })
            })
            .or_else(|| {
                [
                    "/sys/firmware/devicetree/base/model",
                    "/sys/devices/virtual/dmi/id/product_name",
                ]
                .iter()
                .find_map(|p| {
                    std::fs::read_to_string(p)
                        .ok()
                        .map(|s| s.trim_matches(['\0', ' ', '\n', '\r']).to_string())
                        .filter(|s| !s.is_empty())
                })
            });
        (mem, chip)
    }

    #[cfg(target_os = "windows")]
    {
        let env_chip = std::env::var("PROCESSOR_IDENTIFIER")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let ps = crate::fs_util::system_root_bin(r"WindowsPowerShell\v1.0\powershell.exe");
        let ps_cmd = if env_chip.is_some() {
            "(Get-CimInstance Win32_ComputerSystem).TotalPhysicalMemory"
        } else {
            "$m=(Get-CimInstance Win32_ComputerSystem).TotalPhysicalMemory; $c=(Get-CimInstance Win32_Processor | Select-Object -First 1).Name; \"$m`n$c\""
        };
        if let Ok(output) = Command::new(ps)
            .args(["-NoProfile", "-Command", ps_cmd])
            .output()
            && output.status.success()
            && let Ok(text) = std::str::from_utf8(&output.stdout)
        {
            let mut lines = text.lines();
            let mem = lines.next().and_then(|s| s.trim().parse::<u64>().ok());
            let chip = env_chip.or_else(|| {
                lines
                    .next()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
            });
            return (mem, chip);
        }
        (None, env_chip)
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    (None, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_profile_from_specs() {
        for (gb, tier, model, size, ctx, ctx_disp, ollama_sub) in [
            (
                8,
                HardwareTier::Tier8GB,
                "ggml-org/gemma-4-E4B-it-GGUF:Q8_0",
                "E4B",
                8_192,
                "8k ctx",
                "OLLAMA_CONTEXT_LENGTH=8192 ollama run gemma4:e4b",
            ),
            (
                16,
                HardwareTier::Tier16GB,
                "ggml-org/gemma-4-E4B-it-GGUF:Q8_0",
                "E4B",
                32_768,
                "32k ctx",
                "OLLAMA_CONTEXT_LENGTH=32768 ollama run gemma4:e4b",
            ),
            (
                32,
                HardwareTier::Tier32GB,
                "ggml-org/gemma-4-E4B-it-GGUF:Q8_0",
                "E4B",
                65_536,
                "64k ctx",
                "OLLAMA_CONTEXT_LENGTH=65536 ollama run gemma4:e4b",
            ),
            (
                64,
                HardwareTier::Tier64GBPlus,
                "ggml-org/gemma-4-26B-A4B-it-GGUF:Q8_0",
                "26B-A4B",
                131_072,
                "128k ctx",
                "OLLAMA_CONTEXT_LENGTH=131072 ollama run gemma4:26b",
            ),
        ] {
            let profile =
                HardwareProfile::from_specs(gb * 1024 * 1024 * 1024, "Test CPU".to_string());
            assert_eq!(profile.tier, tier);
            assert_eq!(profile.recommended_model, model);
            assert_eq!(profile.recommended_size, size);
            assert_eq!(profile.recommended_context, ctx);
            assert_eq!(profile.recommended_context_display, ctx_disp);
            assert!(
                profile
                    .recommended_launch_command
                    .contains(&format!("-c {ctx}"))
            );
            assert_eq!(profile.ollama_launch_command, ollama_sub);
            assert_eq!(profile.one_pass_token_ceiling(), ctx * 7 / 10);
            assert!(profile.llama_gui_selection().contains(ctx_disp));
        }
    }

    #[test]
    fn test_format_context_tokens() {
        assert_eq!(format_context_tokens(8192), "8k ctx");
        assert_eq!(format_context_tokens(131072), "128k ctx");
        assert_eq!(format_context_tokens(4000), "4k ctx");
        assert_eq!(format_context_tokens(512), "512 ctx");
    }

    #[test]
    fn test_hardware_system_detection() {
        let profile = HardwareProfile::detect();
        assert!(profile.total_memory_bytes > 0);
        assert!(!profile.chip_name.is_empty());
    }
}
