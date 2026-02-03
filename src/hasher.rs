use anyhow::{Context, Result};
use blake3::Hasher;
use glob::glob;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;

/// Hash information for a single file
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct FileHash {
    /// BLAKE3 hash of file contents
    pub hash: String,
    /// File size in bytes
    pub size: u64,
}

/// Result of hashing all files for a verification check
#[derive(Debug)]
pub struct HashResult {
    /// Combined hash of all files
    pub combined_hash: String,
    /// Individual file hashes, keyed by relative path
    pub file_hashes: BTreeMap<String, FileHash>,
}

/// Compute content hash for a verification check's cache paths
pub fn compute_check_hash(project_root: &Path, cache_paths: &[String]) -> Result<HashResult> {
    let mut all_files: BTreeMap<String, FileHash> = BTreeMap::new();

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
                    let file_hash = hash_file(&path)
                        .with_context(|| format!("Failed to hash file: {}", path.display()))?;
                    all_files.insert(relative, file_hash);
                }
            }
        }
    }

    // Create deterministic combined hash
    // BTreeMap ensures sorted, deterministic ordering
    let mut combined_hasher = Hasher::new();

    for (path, file_hash) in &all_files {
        // Include path in hash to detect renames
        combined_hasher.update(path.as_bytes());
        combined_hasher.update(b":");
        combined_hasher.update(file_hash.hash.as_bytes());
        combined_hasher.update(b"\n");
    }

    let combined_hash = combined_hasher.finalize().to_hex().to_string();

    Ok(HashResult {
        combined_hash,
        file_hashes: all_files,
    })
}

/// Hash a single file using BLAKE3
fn hash_file(path: &Path) -> Result<FileHash> {
    let file = File::open(path)?;
    let metadata = file.metadata()?;
    let size = metadata.len();

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

    Ok(FileHash {
        hash: hasher.finalize().to_hex().to_string(),
        size,
    })
}

/// Compare two hash results and return list of changed files
pub fn find_changed_files(
    old_hashes: &BTreeMap<String, FileHash>,
    new_hashes: &BTreeMap<String, FileHash>,
) -> Vec<String> {
    let mut changed = Vec::new();

    // Check for modified or added files
    for (path, new_hash) in new_hashes {
        match old_hashes.get(path) {
            None => changed.push(format!("+ {}", path)), // Added
            Some(old_hash) if old_hash.hash != new_hash.hash => {
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

    #[test]
    fn test_hash_determinism() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test.txt");
        fs::write(&file_path, "hello world").unwrap();

        let hash1 = hash_file(&file_path).unwrap();
        let hash2 = hash_file(&file_path).unwrap();

        assert_eq!(hash1.hash, hash2.hash);
        assert_eq!(hash1.size, hash2.size);
    }

    #[test]
    fn test_find_changed_files() {
        let mut old = BTreeMap::new();
        old.insert(
            "a.txt".to_string(),
            FileHash {
                hash: "hash1".to_string(),
                size: 10,
            },
        );
        old.insert(
            "b.txt".to_string(),
            FileHash {
                hash: "hash2".to_string(),
                size: 20,
            },
        );

        let mut new = BTreeMap::new();
        new.insert(
            "a.txt".to_string(),
            FileHash {
                hash: "hash1_changed".to_string(),
                size: 15,
            },
        );
        new.insert(
            "c.txt".to_string(),
            FileHash {
                hash: "hash3".to_string(),
                size: 30,
            },
        );

        let changed = find_changed_files(&old, &new);
        assert!(changed.contains(&"+ c.txt".to_string())); // Added
        assert!(changed.contains(&"- b.txt".to_string())); // Deleted
        assert!(changed.contains(&"M a.txt".to_string())); // Modified
    }
}
