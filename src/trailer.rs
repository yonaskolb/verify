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
pub fn read_trailer(project_root: &Path) -> Result<Option<BTreeMap<String, String>>> {
    // Try git's built-in trailer parser first
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
    if !value.is_empty() {
        return Ok(Some(parse_trailer_value(&value)));
    }

    // Fallback: parse commit body directly for "Verified:" line.
    // GitHub squash-merge can insert separators or blank lines between trailers,
    // which breaks git's trailer detection.
    let output = Command::new("git")
        .args(["log", "-1", "--format=%B"])
        .current_dir(project_root)
        .output()
        .context("Failed to run git log")?;

    if !output.status.success() {
        anyhow::bail!(
            "git log failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let body = String::from_utf8_lossy(&output.stdout);
    Ok(parse_verified_from_body(&body))
}

/// Search recent git history for the most recent commit with a Verified trailer.
/// Returns None if no trailer is found within max_depth commits.
///
/// Uses direct body parsing rather than git's built-in trailer parser, because
/// GitHub squash-merge can reformat commit messages in ways that break git's
/// trailer detection.
pub fn read_trailer_from_history(
    project_root: &Path,
    max_depth: usize,
) -> Result<Option<BTreeMap<String, String>>> {
    let output = Command::new("git")
        .args(["log", &format!("-{}", max_depth), "--format=%B%x00"])
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
    for commit_body in stdout.split('\0') {
        if let Some(map) = parse_verified_from_body(commit_body) {
            return Ok(Some(map));
        }
    }

    Ok(None)
}

/// Parse a commit message body for a "Verified: name:hash,..." line.
/// Returns the last match, since squash-merge commits may concatenate
/// multiple commit messages each with their own Verified trailer.
fn parse_verified_from_body(body: &str) -> Option<BTreeMap<String, String>> {
    let mut last_match: Option<BTreeMap<String, String>> = None;
    for line in body.lines() {
        let trimmed = line.trim();
        if let Some(value) = trimmed.strip_prefix("Verified:") {
            let value = value.trim();
            if !value.is_empty() {
                last_match = Some(parse_trailer_value(value));
            }
        }
    }
    last_match
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

/// RAII guard that removes a file on drop.
struct FileGuard(std::path::PathBuf);
impl Drop for FileGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Amend HEAD's commit message with a fresh Verified trailer.
/// Temporarily removes MERGE_HEAD if present so `git commit --amend`
/// doesn't fail during post-merge hooks (where git hasn't cleaned up
/// merge state yet). Restores it afterward.
pub fn resign_head(project_root: &Path, hashes: &BTreeMap<String, String>) -> Result<()> {
    // Read HEAD's commit message
    let output = Command::new("git")
        .args(["log", "-1", "--format=%B", "HEAD"])
        .current_dir(project_root)
        .output()
        .context("Failed to read HEAD commit message")?;

    if !output.status.success() {
        anyhow::bail!(
            "git log failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let message = String::from_utf8_lossy(&output.stdout).to_string();

    // Write message with trailer to temp file (outside .git to handle worktrees)
    let temp_path = std::env::temp_dir().join(format!("verify-resign-msg-{}", std::process::id()));
    let _cleanup = FileGuard(temp_path.clone());
    std::fs::write(&temp_path, &message).context("Failed to write temp commit message file")?;
    write_trailer(&temp_path, hashes)?;

    // Temporarily remove MERGE_HEAD if present — git commit --amend refuses
    // to run while it exists, but during post-merge hooks the merge is already
    // complete and git just hasn't cleaned up yet.
    let git_dir_output = Command::new("git")
        .args(["rev-parse", "--git-dir"])
        .current_dir(project_root)
        .output()
        .context("Failed to find git dir")?;
    let git_dir = project_root.join(String::from_utf8_lossy(&git_dir_output.stdout).trim());
    let merge_head = git_dir.join("MERGE_HEAD");
    let merge_head_backup = std::fs::read(&merge_head).ok();
    if merge_head_backup.is_some() {
        let _ = std::fs::remove_file(&merge_head);
    }

    // Amend HEAD with the updated message
    let amend = Command::new("git")
        .args(["commit", "--amend", "-F"])
        .arg(&temp_path)
        .args(["--no-verify", "--allow-empty"])
        .env("VERIFY_RESIGNING", "1")
        .current_dir(project_root)
        .output()
        .context("Failed to amend HEAD commit")?;

    // Restore MERGE_HEAD if we removed it
    if let Some(content) = merge_head_backup {
        let _ = std::fs::write(&merge_head, content);
    }

    if !amend.status.success() {
        anyhow::bail!(
            "git commit --amend failed: {}",
            String::from_utf8_lossy(&amend.stderr)
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
    fn test_parse_verified_from_body_with_separator() {
        // GitHub squash-merge Case 1: separator between trailer blocks
        let body = "\
Some commit message

Co-Authored-By: Claude Opus 4.6 <noreply@anthropic.com>
Verified: build:e833da99,lint:4f573842,specs:3a6033ce,unit-tests:4dac16e9

---------

Co-authored-by: Claude Haiku 4.5 <noreply@anthropic.com>";
        let result = parse_verified_from_body(body).unwrap();
        assert_eq!(result.len(), 4);
        assert_eq!(result["build"], "e833da99");
        assert_eq!(result["lint"], "4f573842");
        assert_eq!(result["specs"], "3a6033ce");
        assert_eq!(result["unit-tests"], "4dac16e9");
    }

    #[test]
    fn test_parse_verified_from_body_with_blank_line() {
        // GitHub squash-merge Case 2: blank line between trailers
        let body = "\
Some commit message

Verified: build:913f862e,lint:d70e7981,snapshots:83f76e78

Co-authored-by: Claude Opus 4.6 <noreply@anthropic.com>";
        let result = parse_verified_from_body(body).unwrap();
        assert_eq!(result.len(), 3);
        assert_eq!(result["build"], "913f862e");
        assert_eq!(result["lint"], "d70e7981");
        assert_eq!(result["snapshots"], "83f76e78");
    }

    #[test]
    fn test_parse_verified_from_body_normal_trailers() {
        // Normal case: contiguous trailer block (git parser would handle this,
        // but our fallback should work too)
        let body = "\
Some commit message

Co-Authored-By: Claude Opus 4.6 <noreply@anthropic.com>
Verified: build:65c54b33,lint:c22ab02f";
        let result = parse_verified_from_body(body).unwrap();
        assert_eq!(result.len(), 2);
        assert_eq!(result["build"], "65c54b33");
        assert_eq!(result["lint"], "c22ab02f");
    }

    #[test]
    fn test_parse_verified_from_body_no_trailer() {
        let body = "Some commit message\n\nNo trailers here.";
        assert!(parse_verified_from_body(body).is_none());
    }

    #[test]
    fn test_parse_verified_from_body_squash_merge_multiple_trailers() {
        // GitHub squash-merge concatenates all commit messages from the PR.
        // Multiple commits may each have their own Verified trailer.
        // The last one is the most recent and should win.
        let body = "\
First commit message

Co-Authored-By: Claude Opus 4.6 <noreply@anthropic.com>
Verified: build:old11111,lint:old22222,specs:45ed4459

Second commit with updated checks

Co-Authored-By: Claude Opus 4.6 <noreply@anthropic.com>
Verified: build:913f862e,lint:d70e7981,snapshots:83f76e78,specs:45ed4459,unit-tests:9157effd

Co-authored-by: Claude Opus 4.6 <noreply@anthropic.com>";
        let result = parse_verified_from_body(body).unwrap();
        assert_eq!(result.len(), 5);
        // Should have the LAST trailer's values, not the first
        assert_eq!(result["build"], "913f862e");
        assert_eq!(result["lint"], "d70e7981");
        assert_eq!(result["snapshots"], "83f76e78");
        assert_eq!(result["specs"], "45ed4459");
        assert_eq!(result["unit-tests"], "9157effd");
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
