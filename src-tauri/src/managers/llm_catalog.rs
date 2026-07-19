//! Static catalog of cleanup-LLM models for the bundled engine.
//!
//! All entries are official Qwen GGUF repos (Apache-2.0). Sizes/filenames were
//! pinned 2026-07-08 from the Hugging Face trees; 7B quants ship as split GGUFs
//! and llama-server loads them by pointing `-m` at shard 1 (all shards must be
//! present). `ram_needed_gb` is the honest total-RAM guidance shown as the
//! green/amber/red fit badge in the picker, weights + KV cache + headroom,
//! not just file size.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
pub struct LlmModelInfo {
    /// Catalog id, e.g. "qwen2.5-7b-instruct-q4_k_m" (also the ladder's key).
    pub id: String,
    pub display_name: String,
    /// Hugging Face repo the shards live in.
    pub repo: String,
    /// GGUF shard filenames in order; shard 0 is what `-m` points at.
    pub files: Vec<String>,
    pub params_b: f64,
    pub quant: String,
    pub total_bytes: u64,
    /// Total machine RAM (GB) where this model runs comfortably.
    pub ram_needed_gb: f64,
    pub license: String,
}

const GB: u64 = 1024 * 1024 * 1024;

fn q(
    id: &str,
    display_name: &str,
    repo: &str,
    files: &[&str],
    params_b: f64,
    quant: &str,
    total_bytes: u64,
    ram_needed_gb: f64,
) -> LlmModelInfo {
    LlmModelInfo {
        id: id.to_string(),
        display_name: display_name.to_string(),
        repo: repo.to_string(),
        files: files.iter().map(|s| s.to_string()).collect(),
        params_b,
        quant: quant.to_string(),
        total_bytes,
        ram_needed_gb,
        license: "Apache-2.0".to_string(),
    }
}

/// Every model the cleanup-model picker offers. Order = display order
/// (best-quality first within each size class).
pub fn catalog() -> Vec<LlmModelInfo> {
    const R05: &str = "Qwen/Qwen2.5-0.5B-Instruct-GGUF";
    const R15: &str = "Qwen/Qwen2.5-1.5B-Instruct-GGUF";
    const R7: &str = "Qwen/Qwen2.5-7B-Instruct-GGUF";
    vec![
        q(
            "qwen2.5-7b-instruct-q8_0",
            "Qwen2.5 7B (Q8, max quality)",
            R7,
            &[
                "qwen2.5-7b-instruct-q8_0-00001-of-00003.gguf",
                "qwen2.5-7b-instruct-q8_0-00002-of-00003.gguf",
                "qwen2.5-7b-instruct-q8_0-00003-of-00003.gguf",
            ],
            7.6,
            "Q8_0",
            (7.54 * GB as f64) as u64,
            20.0,
        ),
        q(
            "qwen2.5-7b-instruct-q5_k_m",
            "Qwen2.5 7B (Q5)",
            R7,
            &[
                "qwen2.5-7b-instruct-q5_k_m-00001-of-00002.gguf",
                "qwen2.5-7b-instruct-q5_k_m-00002-of-00002.gguf",
            ],
            7.6,
            "Q5_K_M",
            (5.08 * GB as f64) as u64,
            16.0,
        ),
        q(
            "qwen2.5-7b-instruct-q4_k_m",
            "Qwen2.5 7B (Q4, recommended)",
            R7,
            &[
                "qwen2.5-7b-instruct-q4_k_m-00001-of-00002.gguf",
                "qwen2.5-7b-instruct-q4_k_m-00002-of-00002.gguf",
            ],
            7.6,
            "Q4_K_M",
            (4.36 * GB as f64) as u64,
            14.0,
        ),
        q(
            "qwen2.5-1.5b-instruct-q8_0",
            "Qwen2.5 1.5B (Q8)",
            R15,
            &["qwen2.5-1.5b-instruct-q8_0.gguf"],
            1.5,
            "Q8_0",
            (1.76 * GB as f64) as u64,
            8.0,
        ),
        q(
            "qwen2.5-1.5b-instruct-q4_k_m",
            "Qwen2.5 1.5B (Q4, compact)",
            R15,
            &["qwen2.5-1.5b-instruct-q4_k_m.gguf"],
            1.5,
            "Q4_K_M",
            (1.04 * GB as f64) as u64,
            6.0,
        ),
        q(
            "qwen2.5-0.5b-instruct-q8_0",
            "Qwen2.5 0.5B (Q8)",
            R05,
            &["qwen2.5-0.5b-instruct-q8_0.gguf"],
            0.5,
            "Q8_0",
            (0.63 * GB as f64) as u64,
            4.0,
        ),
        q(
            "qwen2.5-0.5b-instruct-q4_k_m",
            "Qwen2.5 0.5B (Q4, minimal)",
            R05,
            &["qwen2.5-0.5b-instruct-q4_k_m.gguf"],
            0.5,
            "Q4_K_M",
            (0.46 * GB as f64) as u64,
            4.0,
        ),
    ]
}

pub fn find(id: &str) -> Option<LlmModelInfo> {
    catalog().into_iter().find(|m| m.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ladder_picks_exist() {
        // The hardware ladder's ids must resolve in this catalog.
        assert!(find("qwen2.5-7b-instruct-q4_k_m").is_some());
        assert!(find("qwen2.5-1.5b-instruct-q4_k_m").is_some());
    }

    #[test]
    fn shard_ordering_is_first_shard_loadable() {
        for m in catalog() {
            assert!(!m.files.is_empty(), "{} has no files", m.id);
            if m.files.len() > 1 {
                assert!(
                    m.files[0].contains("-00001-of-"),
                    "{}: shard 0 must be the -00001- file, got {}",
                    m.id,
                    m.files[0]
                );
            }
            assert!(m.files[0].ends_with(".gguf"));
        }
    }

    #[test]
    fn ram_badges_are_monotonic_within_family() {
        // Bigger quants of the same params must not claim less RAM.
        let c = catalog();
        let ram = |id: &str| c.iter().find(|m| m.id == id).unwrap().ram_needed_gb;
        assert!(ram("qwen2.5-7b-instruct-q8_0") >= ram("qwen2.5-7b-instruct-q5_k_m"));
        assert!(ram("qwen2.5-7b-instruct-q5_k_m") >= ram("qwen2.5-7b-instruct-q4_k_m"));
    }
}
