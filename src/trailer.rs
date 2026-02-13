use anyhow::{Context, Result};
use blake3::Hasher;
use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

use crate::cache::{CacheState, VerificationStatus};
use crate::config::Config;
use crate::graph::DependencyGraph;
use crate::hasher::compute_check_hash;

const TRAILER_HASH_LENGTH: usize = 8;

/// Compute combined hash for a regular check from its config_hash and content_hash.
/// Returns full 64-char blake3 hex string.
pub fn compute_combined_hash(config_hash: &str, content_hash: &str) -> String {
    let mut hasher = Hasher::new();
    hasher.update(config_hash.as_bytes());
    hasher.update(b":");
    hasher.update(content_hash.as_bytes());
    hasher.finalize().to_hex().to_string()
}

/// Truncate a full hash to the trailer length (8 chars).
pub fn truncate_hash(hash: &str) -> &str {
    &hash[..TRAILER_HASH_LENGTH.min(hash.len())]
}

/// Compute combined hashes for all currently fresh checks, respecting dependency order.
/// Returns a map of check name -> full combined hash.
/// Skips aggregate checks (implicit from their dependencies).
/// Skips stale checks (files changed, config changed, never run).
pub fn compute_all_hashes(
    project_root: &Path,
    config: &Config,
    cache: &CacheState,
) -> Result<BTreeMap<String, String>> {
    let graph = DependencyGraph::from_config(config)?;
    let waves = graph.execution_waves();
    let mut combined_hashes: BTreeMap<String, String> = BTreeMap::new();

    for wave in waves {
        for name in wave {
            let check = match config.get(&name) {
                Some(v) => v,
                None => continue, // subproject, skip
            };

            // Skip aggregate checks — they're implicit from their dependencies
            if check.command.is_none() {
                continue;
            }

            // Skip untracked checks (no cache_paths)
            if check.cache_paths.is_empty() {
                continue;
            }

            // Compute current hashes and check freshness
            let current_config_hash = check.config_hash();
            let hash_result = compute_check_hash(project_root, &check.cache_paths)?;
            let status = cache.check_staleness(&name, &hash_result.combined_hash, &current_config_hash);

            if matches!(status, VerificationStatus::Verified) {
                let hash = compute_combined_hash(&current_config_hash, &hash_result.combined_hash);
                combined_hashes.insert(name, hash);
            }
        }
    }

    Ok(combined_hashes)
}

/// Compute the expected combined hash for a regular check from current files.
pub fn compute_expected_hash(project_root: &Path, check: &crate::config::Verification) -> Result<String> {
    let config_hash = check.config_hash();
    let hash_result = compute_check_hash(project_root, &check.cache_paths)?;
    Ok(compute_combined_hash(&config_hash, &hash_result.combined_hash))
}

/// Compute expected hashes for all checks from current files, respecting dependency order.
/// Returns a map of check name -> full combined hash.
/// Skips aggregate checks (implicit from their dependencies).
pub fn compute_all_expected_hashes(
    project_root: &Path,
    config: &Config,
) -> Result<BTreeMap<String, String>> {
    let graph = DependencyGraph::from_config(config)?;
    let waves = graph.execution_waves();
    let mut expected_hashes: BTreeMap<String, String> = BTreeMap::new();

    for wave in waves {
        for name in wave {
            let check = match config.get(&name) {
                Some(v) => v,
                None => continue, // subproject, skip
            };

            // Skip aggregate checks — they're implicit from their dependencies
            if check.command.is_none() {
                continue;
            }

            // Skip untracked checks (no cache_paths)
            if check.cache_paths.is_empty() {
                continue;
            }

            expected_hashes.insert(name.clone(), compute_expected_hash(project_root, check)?);
        }
    }

    Ok(expected_hashes)
}

/// Read the Verified trailer from the HEAD commit.
/// Returns None if no Verified trailer is found.
/// Parses "name:hash,name:hash,..." format into a map.
pub fn read_trailer(project_root: &Path) -> Result<Option<BTreeMap<String, String>>> {
    let output = Command::new("git")
        .args(["log", "-1", "--format=%(trailers:key=Verified,valueonly)"])
        .current_dir(project_root)
        .output()
        .context("Failed to run git log. Is this a git repository?")?;

    if !output.status.success() {
        anyhow::bail!(
            "git log failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if value.is_empty() {
        return Ok(None);
    }

    Ok(Some(parse_trailer_value(&value)))
}

/// Search recent git history for the most recent commit with a Verified trailer.
/// Returns None if no trailer is found within max_depth commits.
pub fn read_trailer_from_history(
    project_root: &Path,
    max_depth: usize,
) -> Result<Option<BTreeMap<String, String>>> {
    let depth_arg = format!("-{}", max_depth);
    let output = Command::new("git")
        .args(["log", &depth_arg, "--format=%(trailers:key=Verified,valueonly)"])
        .current_dir(project_root)
        .output()
        .context("Failed to run git log. Is this a git repository?")?;

    if !output.status.success() {
        anyhow::bail!(
            "git log failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines() {
        let trimmed = line.trim();
        if !trimmed.is_empty() {
            return Ok(Some(parse_trailer_value(trimmed)));
        }
    }

    Ok(None)
}

/// Parse a trailer value string "name:hash,name:hash,..." into a map.
pub fn parse_trailer_value(value: &str) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    for pair in value.split(',') {
        let pair = pair.trim();
        if let Some((name, hash)) = pair.split_once(':') {
            map.insert(name.to_string(), hash.to_string());
        }
    }
    map
}

/// Format hashes as a trailer value string "name:hash,name:hash,...".
/// Truncates hashes to 8 chars for compact trailer output.
pub fn format_trailer_value(hashes: &BTreeMap<String, String>) -> String {
    hashes
        .iter()
        .map(|(name, hash)| format!("{}:{}", name, truncate_hash(hash)))
        .collect::<Vec<_>>()
        .join(",")
}

/// Write the Verified trailer to a commit message file using git interpret-trailers.
pub fn write_trailer(commit_msg_file: &Path, hashes: &BTreeMap<String, String>) -> Result<()> {
    if hashes.is_empty() {
        return Ok(());
    }

    let trailer_value = format_trailer_value(hashes);
    let trailer = format!("Verified: {}", trailer_value);

    let output = Command::new("git")
        .args([
            "interpret-trailers",
            "--in-place",
            "--if-exists",
            "replace",
            "--trailer",
            &trailer,
        ])
        .arg(commit_msg_file)
        .output()
        .context("Failed to run git interpret-trailers")?;

    if !output.status.success() {
        anyhow::bail!(
            "git interpret-trailers failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_combined_hash_deterministic() {
        let h1 = compute_combined_hash("config_abc", "content_xyz");
        let h2 = compute_combined_hash("config_abc", "content_xyz");
        assert_eq!(h1, h2);
    }

    #[test]
    fn test_combined_hash_full_length() {
        let h = compute_combined_hash("config", "content");
        assert_eq!(h.len(), 64);
    }

    #[test]
    fn test_combined_hash_changes_on_config() {
        let h1 = compute_combined_hash("config_a", "content");
        let h2 = compute_combined_hash("config_b", "content");
        assert_ne!(h1, h2);
    }

    #[test]
    fn test_combined_hash_changes_on_content() {
        let h1 = compute_combined_hash("config", "content_a");
        let h2 = compute_combined_hash("config", "content_b");
        assert_ne!(h1, h2);
    }

    #[test]
    fn test_truncate_hash() {
        let full = "a1b2c3d4e5f6a7b8c9d0e1f23a4b5c6d7e8f9a0b1c2d3e4f5a6b7c8d9e0f1a2";
        assert_eq!(truncate_hash(full), "a1b2c3d4");
    }

    #[test]
    fn test_truncate_hash_short_input() {
        assert_eq!(truncate_hash("abc"), "abc");
    }

    #[test]
    fn test_format_trailer_value() {
        let mut hashes = BTreeMap::new();
        hashes.insert("build".to_string(), "a1b2c3d4e5f6a7b8".to_string());
        hashes.insert("lint".to_string(), "c9d0e1f23a4b5c6d".to_string());

        let output = format_trailer_value(&hashes);
        assert_eq!(output, "build:a1b2c3d4,lint:c9d0e1f2");
    }

    #[test]
    fn test_format_trailer_value_empty() {
        let hashes = BTreeMap::new();
        assert_eq!(format_trailer_value(&hashes), "");
    }

    #[test]
    fn test_parse_trailer_value() {
        let parsed = parse_trailer_value("build:a1b2c3d4,lint:e5f6a7b8");
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed["build"], "a1b2c3d4");
        assert_eq!(parsed["lint"], "e5f6a7b8");
    }

    #[test]
    fn test_parse_trailer_value_single() {
        let parsed = parse_trailer_value("build:a1b2c3d4");
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed["build"], "a1b2c3d4");
    }

    #[test]
    fn test_parse_trailer_value_empty() {
        let parsed = parse_trailer_value("");
        assert_eq!(parsed.len(), 0);
    }

    #[test]
    fn test_format_parse_roundtrip() {
        let mut hashes = BTreeMap::new();
        hashes.insert("build".to_string(), "a1b2c3d4e5f6a7b8c9d0e1f23a4b5c6d7e8f9a0b1c2d3e4f5a6b7c8d9e0f1a2".to_string());
        hashes.insert("lint".to_string(), "1122334455667788aabbccddeeff00112233445566778899aabbccddeeff001122".to_string());

        let formatted = format_trailer_value(&hashes);
        let parsed = parse_trailer_value(&formatted);

        // Parsed values should be truncated versions
        assert_eq!(parsed["build"], "a1b2c3d4");
        assert_eq!(parsed["lint"], "11223344");
    }
}
