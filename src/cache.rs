use crate::hasher::FileHash;
use crate::metadata::MetadataValue;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::{self, File};
use std::io::BufWriter;
use std::path::Path;

const CACHE_VERSION: u32 = 2;
const LOCK_FILE: &str = "verify.lock";

/// Root cache structure stored in verify.lock
#[derive(Debug, Default, Deserialize, Serialize)]
pub struct CacheState {
    /// Version for future cache format migrations
    pub version: u32,

    /// Cache entry for each verification check
    pub checks: HashMap<String, CheckCache>,
}

/// Cache state for a single verification check
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CheckCache {
    /// Hash of all files matching cache_paths at time of last successful run
    /// None means the check needs to run (never passed or last run failed)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_hash: Option<String>,

    /// Individual file hashes for debugging/transparency and per_file partial progress
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub file_hashes: BTreeMap<String, FileHash>,

    /// Extracted metadata values from last successful run
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub metadata: HashMap<String, MetadataValue>,
}

/// Computed staleness status for a check
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StalenessStatus {
    /// Files have changed since last successful run
    Stale { reason: StalenessReason },
    /// No changes since last successful run
    Fresh,
    /// Never run or no successful run recorded
    NeverRun,
}

/// Reason why a check is stale
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StalenessReason {
    /// Files in cache_paths have changed
    FilesChanged { changed_files: Vec<String> },
    /// A dependency is stale
    DependencyStale { dependency: String },
    /// No cache_paths defined - always run
    NoCachePaths,
}

impl CacheState {
    /// Create a new empty cache state
    pub fn new() -> Self {
        Self {
            version: CACHE_VERSION,
            checks: HashMap::new(),
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
        serde_json::to_writer_pretty(writer, self)
            .with_context(|| "Failed to serialize cache")?;

        // Atomic rename
        fs::rename(&temp_path, &lock_path)
            .with_context(|| format!("Failed to save lock file: {}", lock_path.display()))?;

        Ok(())
    }

    /// Determine if a check is stale based on current hash
    pub fn check_staleness(&self, check_name: &str, current_hash: &str) -> StalenessStatus {
        match self.checks.get(check_name) {
            None => StalenessStatus::NeverRun,
            Some(cache) => {
                match &cache.content_hash {
                    None => StalenessStatus::NeverRun,
                    Some(stored_hash) => {
                        if stored_hash == current_hash {
                            StalenessStatus::Fresh
                        } else {
                            StalenessStatus::Stale {
                                reason: StalenessReason::FilesChanged {
                                    changed_files: vec![], // Will be filled in by caller if needed
                                },
                            }
                        }
                    }
                }
            }
        }
    }

    /// Update cache after running a check
    pub fn update(
        &mut self,
        check_name: &str,
        success: bool,
        content_hash: Option<String>,
        file_hashes: BTreeMap<String, FileHash>,
        metadata: HashMap<String, MetadataValue>,
    ) {
        let cache = if success {
            CheckCache {
                content_hash,
                file_hashes,
                metadata,
            }
        } else {
            // On failure, clear content_hash (will trigger re-run)
            // but keep file_hashes for per_file partial progress
            CheckCache {
                content_hash: None,
                file_hashes: self
                    .checks
                    .get(check_name)
                    .map(|c| c.file_hashes.clone())
                    .unwrap_or_default(),
                metadata: HashMap::new(),
            }
        };
        self.checks.insert(check_name.to_string(), cache);
    }

    /// Get cached info for a check
    pub fn get(&self, check_name: &str) -> Option<&CheckCache> {
        self.checks.get(check_name)
    }

    /// Initialize or get mutable cache entry for per_file mode
    pub fn get_or_create_mut(&mut self, check_name: &str) -> &mut CheckCache {
        self.checks.entry(check_name.to_string()).or_insert_with(|| CheckCache {
            content_hash: None,
            file_hashes: BTreeMap::new(),
            metadata: HashMap::new(),
        })
    }

    /// Update cache for a single file in per_file mode
    pub fn update_per_file_hash(
        &mut self,
        check_name: &str,
        file_path: &str,
        file_hash: FileHash,
    ) {
        let cache = self.get_or_create_mut(check_name);
        cache.file_hashes.insert(file_path.to_string(), file_hash);
    }

    /// Mark per_file check as complete (all files passed)
    pub fn finalize_per_file(
        &mut self,
        check_name: &str,
        combined_hash: String,
        file_hashes: BTreeMap<String, FileHash>,
        metadata: HashMap<String, MetadataValue>,
    ) {
        let cache = self.get_or_create_mut(check_name);
        cache.content_hash = Some(combined_hash);
        cache.file_hashes = file_hashes;
        cache.metadata = metadata;
    }

    /// Mark per_file check as failed (keeps partial file_hashes for progress)
    pub fn mark_per_file_failed(&mut self, check_name: &str) {
        let cache = self.get_or_create_mut(check_name);
        cache.content_hash = None;
        // Keep existing file_hashes for partial progress
    }

    /// Remove cache entries for checks not in the valid set
    pub fn cleanup_orphaned(&mut self, valid_check_names: &HashSet<String>) {
        self.checks.retain(|name, _| valid_check_names.contains(name));
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
            cache.check_staleness("test", "somehash"),
            StalenessStatus::NeverRun
        );
    }

    #[test]
    fn test_staleness_fresh() {
        let mut cache = CacheState::new();
        cache.update(
            "test",
            true,
            Some("abc123".to_string()),
            BTreeMap::new(),
            HashMap::new(),
        );

        assert_eq!(
            cache.check_staleness("test", "abc123"),
            StalenessStatus::Fresh
        );
    }

    #[test]
    fn test_staleness_after_change() {
        let mut cache = CacheState::new();
        cache.update(
            "test",
            true,
            Some("abc123".to_string()),
            BTreeMap::new(),
            HashMap::new(),
        );

        match cache.check_staleness("test", "different_hash") {
            StalenessStatus::Stale { .. } => {}
            other => panic!("Expected Stale, got {:?}", other),
        }
    }

    #[test]
    fn test_staleness_after_failure() {
        let mut cache = CacheState::new();
        cache.update("test", false, Some("abc123".to_string()), BTreeMap::new(), HashMap::new());

        // After failure, content_hash is cleared, so it should be NeverRun
        assert_eq!(
            cache.check_staleness("test", "anyhash"),
            StalenessStatus::NeverRun
        );
    }

    #[test]
    fn test_cleanup_orphaned() {
        let mut cache = CacheState::new();
        cache.update("keep", true, Some("hash1".to_string()), BTreeMap::new(), HashMap::new());
        cache.update("remove", true, Some("hash2".to_string()), BTreeMap::new(), HashMap::new());

        let valid: HashSet<String> = vec!["keep".to_string()].into_iter().collect();
        cache.cleanup_orphaned(&valid);

        assert!(cache.get("keep").is_some());
        assert!(cache.get("remove").is_none());
    }
}
