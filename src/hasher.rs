use anyhow::{Context, Result};
use blake3::Hasher;
use glob::glob;
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;

/// Result of hashing all files for a verification check
#[derive(Debug)]
pub struct HashResult {
    /// Combined hash of all files
    pub combined_hash: String,
    /// Individual file hashes, keyed by relative path (path -> hash)
    pub file_hashes: BTreeMap<String, String>,
}

/// Compute content hash for a verification check's cache paths
pub fn compute_check_hash(project_root: &Path, cache_paths: &[String]) -> Result<HashResult> {
    let mut all_files: BTreeMap<String, String> = BTreeMap::new();

    // Expand all glob patterns and collect matching files
    for pattern in cache_paths {
        let full_pattern = project_root.join(pattern);
        let pattern_str = full_pattern.to_string_lossy();

        let entries = glob(&pattern_str)
            .with_context(|| format!("Invalid glob pattern: {}", pattern))?;

        for entry in entries {
            let path = entry.with_context(|| format!("Error reading glob entry for: {}", pattern))?;

            if path.is_file() {
                let relative = path
                    .strip_prefix(project_root)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .to_string();

                // Only hash each file once (in case patterns overlap)
                if !all_files.contains_key(&relative) {
                    let hash = hash_file(&path)
                        .with_context(|| format!("Failed to hash file: {}", path.display()))?;
                    all_files.insert(relative, hash);
                }
            }
        }
    }

    // Create deterministic combined hash
    // BTreeMap ensures sorted, deterministic ordering
    let mut combined_hasher = Hasher::new();

    for (path, hash) in &all_files {
        // Include path in hash to detect renames
        combined_hasher.update(path.as_bytes());
        combined_hasher.update(b":");
        combined_hasher.update(hash.as_bytes());
        combined_hasher.update(b"\n");
    }

    let combined_hash = combined_hasher.finalize().to_hex().to_string();

    Ok(HashResult {
        combined_hash,
        file_hashes: all_files,
    })
}

/// Hash a single file using BLAKE3
fn hash_file(path: &Path) -> Result<String> {
    let file = File::open(path)?;
    let mut reader = BufReader::new(file);
    let mut hasher = Hasher::new();

    // Stream file in chunks for memory efficiency
    let mut buffer = [0u8; 65536]; // 64KB buffer
    loop {
        let bytes_read = reader.read(&mut buffer)?;
        if bytes_read == 0 {
            break;
        }
        hasher.update(&buffer[..bytes_read]);
    }

    Ok(hasher.finalize().to_hex().to_string())
}

/// Compare two hash results and return list of changed files
pub fn find_changed_files(
    old_hashes: &BTreeMap<String, String>,
    new_hashes: &BTreeMap<String, String>,
) -> Vec<String> {
    let mut changed = Vec::new();

    // Check for modified or added files
    for (path, new_hash) in new_hashes {
        match old_hashes.get(path) {
            None => changed.push(format!("+ {}", path)), // Added
            Some(old_hash) if old_hash != new_hash => {
                changed.push(format!("M {}", path)) // Modified
            }
            _ => {} // Unchanged
        }
    }

    // Check for deleted files
    for path in old_hashes.keys() {
        if !new_hashes.contains_key(path) {
            changed.push(format!("- {}", path)); // Deleted
        }
    }

    changed.sort();
    changed
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    // ==================== hash_file tests ====================

    #[test]
    fn test_hash_determinism() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test.txt");
        fs::write(&file_path, "hello world").unwrap();

        let hash1 = hash_file(&file_path).unwrap();
        let hash2 = hash_file(&file_path).unwrap();

        assert_eq!(hash1, hash2);
    }

    #[test]
    fn test_hash_different_content_different_hash() {
        let dir = tempdir().unwrap();
        let file1 = dir.path().join("file1.txt");
        let file2 = dir.path().join("file2.txt");

        fs::write(&file1, "hello").unwrap();
        fs::write(&file2, "world").unwrap();

        let hash1 = hash_file(&file1).unwrap();
        let hash2 = hash_file(&file2).unwrap();

        assert_ne!(hash1, hash2);
    }

    #[test]
    fn test_hash_empty_file() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("empty.txt");
        fs::write(&file_path, "").unwrap();

        let hash = hash_file(&file_path).unwrap();
        // Empty file should still produce a valid hash
        assert!(!hash.is_empty());
        assert_eq!(hash.len(), 64); // BLAKE3 produces 256-bit (64 hex chars) hash
    }

    #[test]
    fn test_hash_same_content_same_hash() {
        let dir = tempdir().unwrap();
        let file1 = dir.path().join("file1.txt");
        let file2 = dir.path().join("file2.txt");

        fs::write(&file1, "identical content").unwrap();
        fs::write(&file2, "identical content").unwrap();

        let hash1 = hash_file(&file1).unwrap();
        let hash2 = hash_file(&file2).unwrap();

        assert_eq!(hash1, hash2);
    }

    // ==================== find_changed_files tests ====================

    #[test]
    fn test_find_changed_files() {
        let mut old = BTreeMap::new();
        old.insert("a.txt".to_string(), "hash1".to_string());
        old.insert("b.txt".to_string(), "hash2".to_string());

        let mut new = BTreeMap::new();
        new.insert("a.txt".to_string(), "hash1_changed".to_string());
        new.insert("c.txt".to_string(), "hash3".to_string());

        let changed = find_changed_files(&old, &new);
        assert!(changed.contains(&"+ c.txt".to_string())); // Added
        assert!(changed.contains(&"- b.txt".to_string())); // Deleted
        assert!(changed.contains(&"M a.txt".to_string())); // Modified
    }

    #[test]
    fn test_find_changed_files_only_added() {
        let old: BTreeMap<String, String> = BTreeMap::new();

        let mut new = BTreeMap::new();
        new.insert("a.txt".to_string(), "hash1".to_string());
        new.insert("b.txt".to_string(), "hash2".to_string());

        let changed = find_changed_files(&old, &new);
        assert_eq!(changed.len(), 2);
        assert!(changed.contains(&"+ a.txt".to_string()));
        assert!(changed.contains(&"+ b.txt".to_string()));
    }

    #[test]
    fn test_find_changed_files_only_deleted() {
        let mut old = BTreeMap::new();
        old.insert("a.txt".to_string(), "hash1".to_string());
        old.insert("b.txt".to_string(), "hash2".to_string());

        let new: BTreeMap<String, String> = BTreeMap::new();

        let changed = find_changed_files(&old, &new);
        assert_eq!(changed.len(), 2);
        assert!(changed.contains(&"- a.txt".to_string()));
        assert!(changed.contains(&"- b.txt".to_string()));
    }

    #[test]
    fn test_find_changed_files_only_modified() {
        let mut old = BTreeMap::new();
        old.insert("a.txt".to_string(), "old_hash".to_string());

        let mut new = BTreeMap::new();
        new.insert("a.txt".to_string(), "new_hash".to_string());

        let changed = find_changed_files(&old, &new);
        assert_eq!(changed.len(), 1);
        assert!(changed.contains(&"M a.txt".to_string()));
    }

    #[test]
    fn test_find_changed_files_no_changes() {
        let mut old = BTreeMap::new();
        old.insert("a.txt".to_string(), "hash1".to_string());
        old.insert("b.txt".to_string(), "hash2".to_string());

        let mut new = BTreeMap::new();
        new.insert("a.txt".to_string(), "hash1".to_string());
        new.insert("b.txt".to_string(), "hash2".to_string());

        let changed = find_changed_files(&old, &new);
        assert!(changed.is_empty());
    }

    #[test]
    fn test_find_changed_files_both_empty() {
        let old: BTreeMap<String, String> = BTreeMap::new();
        let new: BTreeMap<String, String> = BTreeMap::new();

        let changed = find_changed_files(&old, &new);
        assert!(changed.is_empty());
    }

    #[test]
    fn test_find_changed_files_sorted_output() {
        let mut old = BTreeMap::new();
        old.insert("z.txt".to_string(), "hash1".to_string());

        let mut new = BTreeMap::new();
        new.insert("a.txt".to_string(), "hash2".to_string());
        new.insert("m.txt".to_string(), "hash3".to_string());

        let changed = find_changed_files(&old, &new);
        // Output should be sorted: "- z.txt", "+ a.txt", "+ m.txt"
        assert_eq!(changed.len(), 3);
        assert_eq!(changed[0], "+ a.txt");
        assert_eq!(changed[1], "+ m.txt");
        assert_eq!(changed[2], "- z.txt");
    }

    // ==================== compute_check_hash tests ====================

    #[test]
    fn test_compute_check_hash_empty_patterns() {
        let dir = tempdir().unwrap();

        let result = compute_check_hash(dir.path(), &[]).unwrap();
        assert!(result.file_hashes.is_empty());
        // Combined hash of nothing should still be deterministic
        assert!(!result.combined_hash.is_empty());
    }

    #[test]
    fn test_compute_check_hash_single_file() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test.txt");
        fs::write(&file_path, "content").unwrap();

        let result = compute_check_hash(dir.path(), &["test.txt".to_string()]).unwrap();
        assert_eq!(result.file_hashes.len(), 1);
        assert!(result.file_hashes.contains_key("test.txt"));
    }

    #[test]
    fn test_compute_check_hash_glob_pattern() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("a.rs"), "fn a() {}").unwrap();
        fs::write(dir.path().join("b.rs"), "fn b() {}").unwrap();
        fs::write(dir.path().join("c.txt"), "text file").unwrap();

        let result = compute_check_hash(dir.path(), &["*.rs".to_string()]).unwrap();
        assert_eq!(result.file_hashes.len(), 2);
        assert!(result.file_hashes.contains_key("a.rs"));
        assert!(result.file_hashes.contains_key("b.rs"));
        assert!(!result.file_hashes.contains_key("c.txt"));
    }

    #[test]
    fn test_compute_check_hash_overlapping_patterns() {
        // When patterns overlap, files should only be hashed once
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("test.rs"), "content").unwrap();

        let result = compute_check_hash(
            dir.path(),
            &["*.rs".to_string(), "test.rs".to_string()],
        ).unwrap();

        // Should only have one entry despite matching both patterns
        assert_eq!(result.file_hashes.len(), 1);
    }

    #[test]
    fn test_compute_check_hash_determinism() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("a.txt"), "aaa").unwrap();
        fs::write(dir.path().join("b.txt"), "bbb").unwrap();

        let result1 = compute_check_hash(dir.path(), &["*.txt".to_string()]).unwrap();
        let result2 = compute_check_hash(dir.path(), &["*.txt".to_string()]).unwrap();

        assert_eq!(result1.combined_hash, result2.combined_hash);
        assert_eq!(result1.file_hashes, result2.file_hashes);
    }

    #[test]
    fn test_compute_check_hash_includes_path_in_combined() {
        // Renaming a file should change the combined hash even if content is same
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("a.txt"), "content").unwrap();

        let result1 = compute_check_hash(dir.path(), &["a.txt".to_string()]).unwrap();

        // Remove and create with different name
        fs::remove_file(dir.path().join("a.txt")).unwrap();
        fs::write(dir.path().join("b.txt"), "content").unwrap();

        let result2 = compute_check_hash(dir.path(), &["b.txt".to_string()]).unwrap();

        // Individual file hashes should be the same (same content)
        let hash1 = result1.file_hashes.get("a.txt").unwrap();
        let hash2 = result2.file_hashes.get("b.txt").unwrap();
        assert_eq!(hash1, hash2);

        // But combined hashes should differ (path is included)
        assert_ne!(result1.combined_hash, result2.combined_hash);
    }

    #[test]
    fn test_compute_check_hash_nested_directories() {
        let dir = tempdir().unwrap();
        let sub_dir = dir.path().join("src");
        fs::create_dir(&sub_dir).unwrap();
        fs::write(sub_dir.join("main.rs"), "fn main() {}").unwrap();
        fs::write(sub_dir.join("lib.rs"), "pub fn lib() {}").unwrap();

        let result = compute_check_hash(dir.path(), &["src/*.rs".to_string()]).unwrap();
        assert_eq!(result.file_hashes.len(), 2);
        assert!(result.file_hashes.contains_key("src/main.rs"));
        assert!(result.file_hashes.contains_key("src/lib.rs"));
    }

    #[test]
    fn test_compute_check_hash_no_matching_files() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("test.txt"), "content").unwrap();

        // Pattern that matches nothing
        let result = compute_check_hash(dir.path(), &["*.rs".to_string()]).unwrap();
        assert!(result.file_hashes.is_empty());
    }

    #[test]
    fn test_compute_check_hash_multiple_patterns() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("code.rs"), "rust").unwrap();
        fs::write(dir.path().join("code.ts"), "typescript").unwrap();
        fs::write(dir.path().join("readme.md"), "docs").unwrap();

        let result = compute_check_hash(
            dir.path(),
            &["*.rs".to_string(), "*.ts".to_string()],
        ).unwrap();

        assert_eq!(result.file_hashes.len(), 2);
        assert!(result.file_hashes.contains_key("code.rs"));
        assert!(result.file_hashes.contains_key("code.ts"));
        assert!(!result.file_hashes.contains_key("readme.md"));
    }
}
