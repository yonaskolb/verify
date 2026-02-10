use crate::metadata::MetadataValue;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::fs::{self, File};
use std::io::BufWriter;
use std::path::Path;

const CACHE_VERSION: u32 = 4;
const LOCK_FILE: &str = "verify.lock";

/// Root cache structure stored in verify.lock
#[derive(Debug, Default, Deserialize, Serialize)]
pub struct CacheState {
    /// Version for future cache format migrations
    pub version: u32,

    /// Cache entry for each verification check
    pub checks: BTreeMap<String, CheckCache>,
}

/// Cache state for a single verification check
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CheckCache {
    /// Hash of the check's configuration (command, cache_paths, etc.)
    /// Used to detect when the check definition changes in verify.yaml
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config_hash: Option<String>,

    /// Hash of all files matching cache_paths at time of last successful run
    /// None means the check needs to run (never passed or last run failed)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_hash: Option<String>,

    /// Individual file hashes - only stored for per_file checks to track partial progress
    /// Maps file path to content hash
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub file_hashes: BTreeMap<String, String>,

    /// Extracted metadata values from last successful run
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub metadata: BTreeMap<String, MetadataValue>,
}

/// Computed verification status for a check
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerificationStatus {
    /// Check passed and nothing has changed since
    Verified,
    /// Check needs to run
    Unverified { reason: UnverifiedReason },
    /// Check has no cache_paths so changes can't be tracked
    Untracked,
}

/// Reason why a check is unverified
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnverifiedReason {
    /// Files in cache_paths have changed
    FilesChanged { changed_files: Vec<String> },
    /// A dependency is unverified
    DependencyUnverified { dependency: String },
    /// The check definition changed in verify.yaml
    ConfigChanged,
    /// Never run or no successful run recorded
    NeverRun,
}

impl CacheState {
    /// Create a new empty cache state
    pub fn new() -> Self {
        Self {
            version: CACHE_VERSION,
            checks: BTreeMap::new(),
        }
    }

    /// Load cache from disk, returning empty cache if file doesn't exist or can't be parsed
    pub fn load(project_root: &Path) -> Result<Self> {
        let lock_path = project_root.join(LOCK_FILE);

        if !lock_path.exists() {
            return Ok(Self::new());
        }

        let content = match fs::read_to_string(&lock_path) {
            Ok(c) => c,
            Err(_) => return Ok(Self::new()),
        };

        let cache: CacheState = match serde_json::from_str(&content) {
            Ok(c) => c,
            Err(_) => return Ok(Self::new()),
        };

        // Handle version migration - just return empty cache on version mismatch
        if cache.version != CACHE_VERSION {
            return Ok(Self::new());
        }

        Ok(cache)
    }

    /// Save cache to disk atomically
    pub fn save(&self, project_root: &Path) -> Result<()> {
        let lock_path = project_root.join(LOCK_FILE);
        let temp_path = project_root.join("verify.lock.tmp");

        // Write to temp file
        let file = File::create(&temp_path)
            .with_context(|| format!("Failed to create temp lock file: {}", temp_path.display()))?;
        let writer = BufWriter::new(file);
        serde_json::to_writer_pretty(writer, self).with_context(|| "Failed to serialize cache")?;

        // Atomic rename
        fs::rename(&temp_path, &lock_path)
            .with_context(|| format!("Failed to save lock file: {}", lock_path.display()))?;

        Ok(())
    }

    /// Determine verification status based on current content hash and config hash
    pub fn check_staleness(
        &self,
        check_name: &str,
        current_content_hash: &str,
        current_config_hash: &str,
    ) -> VerificationStatus {
        match self.checks.get(check_name) {
            None => VerificationStatus::Unverified {
                reason: UnverifiedReason::NeverRun,
            },
            Some(cache) => {
                // Check config hash first - if config changed, check is unverified
                match &cache.config_hash {
                    None => {
                        return VerificationStatus::Unverified {
                            reason: UnverifiedReason::NeverRun,
                        }
                    }
                    Some(stored_config_hash) => {
                        if stored_config_hash != current_config_hash {
                            return VerificationStatus::Unverified {
                                reason: UnverifiedReason::ConfigChanged,
                            };
                        }
                    }
                }

                // Then check content hash
                match &cache.content_hash {
                    None => VerificationStatus::Unverified {
                        reason: UnverifiedReason::NeverRun,
                    },
                    Some(stored_hash) => {
                        if stored_hash == current_content_hash {
                            VerificationStatus::Verified
                        } else {
                            VerificationStatus::Unverified {
                                reason: UnverifiedReason::FilesChanged {
                                    changed_files: vec![], // Will be filled in by caller if needed
                                },
                            }
                        }
                    }
                }
            }
        }
    }

    /// Update cache after running a check.
    /// Only stores file_hashes for per_file checks to keep lock file small.
    pub fn update(
        &mut self,
        check_name: &str,
        success: bool,
        config_hash: String,
        content_hash: Option<String>,
        file_hashes: BTreeMap<String, String>,
        metadata: BTreeMap<String, MetadataValue>,
        per_file: bool,
    ) {
        let cache = if success {
            CheckCache {
                config_hash: Some(config_hash),
                content_hash,
                // Only store file_hashes for per_file checks
                file_hashes: if per_file {
                    file_hashes
                } else {
                    BTreeMap::new()
                },
                metadata,
            }
        } else {
            // On failure, clear content_hash (will trigger re-run)
            // but keep file_hashes for per_file partial progress
            CheckCache {
                config_hash: Some(config_hash),
                content_hash: None,
                file_hashes: if per_file {
                    self.checks
                        .get(check_name)
                        .map(|c| c.file_hashes.clone())
                        .unwrap_or_default()
                } else {
                    BTreeMap::new()
                },
                metadata: BTreeMap::new(),
            }
        };
        self.checks.insert(check_name.to_string(), cache);
    }

    /// Get cached info for a check
    pub fn get(&self, check_name: &str) -> Option<&CheckCache> {
        self.checks.get(check_name)
    }

    /// Initialize or get mutable cache entry for per_file mode
    pub fn get_or_create_mut(&mut self, check_name: &str, config_hash: &str) -> &mut CheckCache {
        self.checks
            .entry(check_name.to_string())
            .or_insert_with(|| CheckCache {
                config_hash: Some(config_hash.to_string()),
                content_hash: None,
                file_hashes: BTreeMap::new(),
                metadata: BTreeMap::new(),
            })
    }

    /// Update cache for a single file in per_file mode
    pub fn update_per_file_hash(
        &mut self,
        check_name: &str,
        config_hash: &str,
        file_path: &str,
        file_hash: String,
    ) {
        let cache = self.get_or_create_mut(check_name, config_hash);
        cache.file_hashes.insert(file_path.to_string(), file_hash);
    }

    /// Mark per_file check as complete (all files passed)
    pub fn finalize_per_file(
        &mut self,
        check_name: &str,
        config_hash: &str,
        combined_hash: String,
        file_hashes: BTreeMap<String, String>,
        metadata: BTreeMap<String, MetadataValue>,
    ) {
        let cache = self.get_or_create_mut(check_name, config_hash);
        cache.config_hash = Some(config_hash.to_string());
        cache.content_hash = Some(combined_hash);
        cache.file_hashes = file_hashes;
        cache.metadata = metadata;
    }

    /// Mark per_file check as failed (keeps partial file_hashes for progress)
    pub fn mark_per_file_failed(&mut self, check_name: &str, config_hash: &str) {
        let cache = self.get_or_create_mut(check_name, config_hash);
        cache.config_hash = Some(config_hash.to_string());
        cache.content_hash = None;
        // Keep existing file_hashes for partial progress
    }

    /// Remove cache entries for checks not in the valid set
    pub fn cleanup_orphaned(&mut self, valid_check_names: &HashSet<String>) {
        self.checks
            .retain(|name, _| valid_check_names.contains(name));
    }

    /// Clear cache for specific checks or all
    pub fn clear(&mut self, names: &[String]) {
        if names.is_empty() {
            self.checks.clear();
        } else {
            for name in names {
                self.checks.remove(name);
            }
        }
    }
}

/// Clean the cache file
pub fn clean_cache(project_root: &Path, names: Vec<String>) -> Result<()> {
    let mut cache = CacheState::load(project_root)?;
    cache.clear(&names);
    cache.save(project_root)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_staleness_never_run() {
        let cache = CacheState::new();
        assert_eq!(
            cache.check_staleness("test", "somehash", "confighash"),
            VerificationStatus::Unverified {
                reason: UnverifiedReason::NeverRun
            }
        );
    }

    #[test]
    fn test_staleness_fresh() {
        let mut cache = CacheState::new();
        cache.update(
            "test",
            true,
            "confighash".to_string(),
            Some("abc123".to_string()),
            BTreeMap::new(),
            BTreeMap::new(),
            false,
        );

        assert_eq!(
            cache.check_staleness("test", "abc123", "confighash"),
            VerificationStatus::Verified
        );
    }

    #[test]
    fn test_staleness_after_content_change() {
        let mut cache = CacheState::new();
        cache.update(
            "test",
            true,
            "confighash".to_string(),
            Some("abc123".to_string()),
            BTreeMap::new(),
            BTreeMap::new(),
            false,
        );

        match cache.check_staleness("test", "different_hash", "confighash") {
            VerificationStatus::Unverified {
                reason: UnverifiedReason::FilesChanged { .. },
            } => {}
            other => panic!("Expected Unverified(FilesChanged), got {:?}", other),
        }
    }

    #[test]
    fn test_staleness_after_config_change() {
        let mut cache = CacheState::new();
        cache.update(
            "test",
            true,
            "confighash".to_string(),
            Some("abc123".to_string()),
            BTreeMap::new(),
            BTreeMap::new(),
            false,
        );

        match cache.check_staleness("test", "abc123", "different_config") {
            VerificationStatus::Unverified {
                reason: UnverifiedReason::ConfigChanged,
            } => {}
            other => panic!("Expected Unverified(ConfigChanged), got {:?}", other),
        }
    }

    #[test]
    fn test_staleness_after_failure() {
        let mut cache = CacheState::new();
        cache.update(
            "test",
            false,
            "confighash".to_string(),
            Some("abc123".to_string()),
            BTreeMap::new(),
            BTreeMap::new(),
            false,
        );

        // After failure, content_hash is cleared, so it should be Unverified(NeverRun)
        assert_eq!(
            cache.check_staleness("test", "anyhash", "confighash"),
            VerificationStatus::Unverified {
                reason: UnverifiedReason::NeverRun
            }
        );
    }

    #[test]
    fn test_cleanup_orphaned() {
        let mut cache = CacheState::new();
        cache.update(
            "keep",
            true,
            "config1".to_string(),
            Some("hash1".to_string()),
            BTreeMap::new(),
            BTreeMap::new(),
            false,
        );
        cache.update(
            "remove",
            true,
            "config2".to_string(),
            Some("hash2".to_string()),
            BTreeMap::new(),
            BTreeMap::new(),
            false,
        );

        let valid: HashSet<String> = vec!["keep".to_string()].into_iter().collect();
        cache.cleanup_orphaned(&valid);

        assert!(cache.get("keep").is_some());
        assert!(cache.get("remove").is_none());
    }

    #[test]
    fn test_file_hashes_only_stored_for_per_file() {
        let mut cache = CacheState::new();
        let mut file_hashes = BTreeMap::new();
        file_hashes.insert("test.rs".to_string(), "abc".to_string());

        // Regular check - file_hashes should NOT be stored
        cache.update(
            "regular",
            true,
            "config".to_string(),
            Some("hash".to_string()),
            file_hashes.clone(),
            BTreeMap::new(),
            false,
        );
        assert!(cache.get("regular").unwrap().file_hashes.is_empty());

        // per_file check - file_hashes SHOULD be stored
        cache.update(
            "perfile",
            true,
            "config".to_string(),
            Some("hash".to_string()),
            file_hashes,
            BTreeMap::new(),
            true,
        );
        assert!(!cache.get("perfile").unwrap().file_hashes.is_empty());
    }
}
