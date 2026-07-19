//! Hardware probe + model ladder for the bundled LLM engine.
//!
//! Probed once per process (cheap, cached in a `OnceLock`): total RAM, whether
//! we are inside a VM (paravirtual GPUs are slower than CPU for llama.cpp, //! measured 10-15x on this project's reference VM), and whether this is real
//! Apple Silicon (Metal-worthy). The *ladder* maps that to a default cleanup
//! model so capable machines get the full-quality 7B and small machines are
//! never bricked by a model they can't load. The ladder picks defaults only, //! the user can select any model in the picker at any time.
//!
//! Test/support overrides: `VAPORLY_FORCE_RAM_GB`, `VAPORLY_FORCE_VM=1|0`.

use serde::{Deserialize, Serialize};
use std::sync::OnceLock;

/// Which default the machine earns. Serialized to the frontend for onboarding
/// copy ("16 GB detected, full quality").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum Tier {
    /// >= 14 GB total RAM: Qwen2.5-7B-Instruct Q4_K_M, the exact weights the
    /// Ollama default (`qwen2.5:7b`) runs, so migrating costs zero quality.
    Full7b,
    /// 6-14 GB: Qwen2.5-1.5B-Instruct Q4_K_M, honestly labeled reduced quality.
    Compact1_5b,
    /// < 6 GB: raw dictation by default; LLM cleanup stays opt-in (0.5B).
    Raw,
}

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
pub struct HardwareProfile {
    pub total_ram_gb: f64,
    pub is_vm: bool,
    pub is_apple_silicon: bool,
    pub tier: Tier,
    /// Catalog id of the ladder's pick (empty for `Raw`, nothing auto-selected).
    pub recommended_model_id: String,
}

static PROFILE: OnceLock<HardwareProfile> = OnceLock::new();

/// The cached hardware profile (probed on first call).
pub fn profile() -> &'static HardwareProfile {
    PROFILE.get_or_init(probe)
}

/// Catalog id the ladder recommends for this machine ("" on the Raw tier).
pub fn recommended_model_id() -> String {
    profile().recommended_model_id.clone()
}

/// GPU layers for llama-server's `-ngl`: everything on Metal for real Apple
/// Silicon, CPU-only anywhere else (VMs' paravirtual GPUs lose to CPU; Windows/
/// Linux ship the CPU build of llama-server, so offload would be a no-op).
/// CPU threads for the bundled llama-server: leave headroom for the STT
/// decode worker, the audio callback, and the UI. Physical cores minus two,
/// floor 2; logical parallelism is the fallback when the physical count is
/// unknown. Round 21 tried cores minus one and it MEASURABLY starved the
/// pseudo-stream live ticks (max tick 405 ms -> 828 ms on the owner's 8-core
/// VM, stale uncased live text) - the two-core margin is load-bearing; the
/// real finalize speedup is stitch reuse, not more engine threads.
pub fn engine_cpu_threads() -> u32 {
    let logical = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);
    let cores = sysinfo::System::new()
        .physical_core_count()
        .unwrap_or(logical);
    cores.saturating_sub(2).max(2) as u32
}

pub fn auto_gpu_layers() -> u32 {
    let p = profile();
    if p.is_apple_silicon && !p.is_vm {
        99
    } else {
        0
    }
}

fn probe() -> HardwareProfile {
    let total_ram_gb = match std::env::var("VAPORLY_FORCE_RAM_GB")
        .ok()
        .and_then(|v| v.parse::<f64>().ok())
    {
        Some(forced) => forced,
        None => {
            let mut sys = sysinfo::System::new();
            sys.refresh_memory();
            sys.total_memory() as f64 / (1024.0 * 1024.0 * 1024.0)
        }
    };

    let is_vm = match std::env::var("VAPORLY_FORCE_VM").ok().as_deref() {
        Some("1") | Some("true") => true,
        Some("0") | Some("false") => false,
        _ => detect_vm(),
    };

    let is_apple_silicon = cfg!(all(target_os = "macos", target_arch = "aarch64"));

    let (tier, recommended_model_id) = ladder(total_ram_gb);

    let profile = HardwareProfile {
        total_ram_gb,
        is_vm,
        is_apple_silicon,
        tier,
        recommended_model_id,
    };
    log::info!(
        "hardware probe: ram={:.1}GB vm={} apple_silicon={} tier={:?}",
        profile.total_ram_gb,
        profile.is_vm,
        profile.is_apple_silicon,
        profile.tier
    );
    profile
}

/// RAM -> (tier, catalog id). Ids live in [`crate::managers::llm_catalog`].
fn ladder(ram_gb: f64) -> (Tier, String) {
    if ram_gb >= 14.0 {
        (Tier::Full7b, "qwen2.5-7b-instruct-q4_k_m".to_string())
    } else if ram_gb >= 6.0 {
        (
            Tier::Compact1_5b,
            "qwen2.5-1.5b-instruct-q4_k_m".to_string(),
        )
    } else {
        (Tier::Raw, String::new())
    }
}

/// VM detection. macOS: `kern.hv_vmm_present` is the canonical signal for
/// Apple-Silicon VMs; `hw.model` containing "VirtualMac" covers the same
/// ground on older builds. Other OSes: conservative `false` (they run the CPU
/// llama-server build regardless, so a miss costs nothing).
#[cfg(target_os = "macos")]
fn detect_vm() -> bool {
    fn sysctl_i32(name: &str) -> Option<i32> {
        let cname = std::ffi::CString::new(name).ok()?;
        let mut val: i32 = 0;
        let mut len = std::mem::size_of::<i32>();
        let rc = unsafe {
            libc::sysctlbyname(
                cname.as_ptr(),
                &mut val as *mut _ as *mut libc::c_void,
                &mut len,
                std::ptr::null_mut(),
                0,
            )
        };
        (rc == 0).then_some(val)
    }
    fn sysctl_string(name: &str) -> Option<String> {
        let cname = std::ffi::CString::new(name).ok()?;
        let mut len: usize = 0;
        unsafe {
            if libc::sysctlbyname(
                cname.as_ptr(),
                std::ptr::null_mut(),
                &mut len,
                std::ptr::null_mut(),
                0,
            ) != 0
            {
                return None;
            }
            let mut buf = vec![0u8; len];
            if libc::sysctlbyname(
                cname.as_ptr(),
                buf.as_mut_ptr() as *mut libc::c_void,
                &mut len,
                std::ptr::null_mut(),
                0,
            ) != 0
            {
                return None;
            }
            buf.truncate(len.saturating_sub(1)); // trailing NUL
            String::from_utf8(buf).ok()
        }
    }

    if sysctl_i32("kern.hv_vmm_present").unwrap_or(0) != 0 {
        return true;
    }
    sysctl_string("hw.model")
        .map(|m| m.contains("VirtualMac"))
        .unwrap_or(false)
}

#[cfg(not(target_os = "macos"))]
fn detect_vm() -> bool {
    false
}

#[cfg(test)]
mod tests {
    #[test]
    fn engine_cpu_threads_leaves_headroom() {
        let t = super::engine_cpu_threads();
        let logical = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4) as u32;
        assert!(t >= 2);
        assert!(t <= logical);
    }

    use super::*;

    #[test]
    fn ladder_thresholds() {
        assert_eq!(ladder(64.0).0, Tier::Full7b);
        assert_eq!(ladder(16.0).0, Tier::Full7b);
        assert_eq!(ladder(14.0).0, Tier::Full7b);
        assert_eq!(ladder(13.9).0, Tier::Compact1_5b);
        assert_eq!(ladder(8.0).0, Tier::Compact1_5b);
        assert_eq!(ladder(6.0).0, Tier::Compact1_5b);
        assert_eq!(ladder(5.9).0, Tier::Raw);
        assert_eq!(ladder(2.0).0, Tier::Raw);
    }

    #[test]
    fn ladder_ids_exist_for_capable_tiers() {
        assert!(!ladder(16.0).1.is_empty());
        assert!(!ladder(8.0).1.is_empty());
        assert!(ladder(4.0).1.is_empty());
    }

    #[test]
    fn probe_smoke() {
        // Must not panic and must report something plausible.
        let p = profile();
        assert!(p.total_ram_gb > 0.5, "ram_gb = {}", p.total_ram_gb);
    }
}
