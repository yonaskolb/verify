use crate::hasher::FileHash;
use crate::metadata::MetadataValue;
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::fs::{self, File};
use std::io::BufWriter;
use std::path::Path;

const CACHE_VERSION: u32 = 1;
const CACHE_DIR: &str = ".vfy";
const CACHE_FILE: &str = "cache.json";

/// Root cache structure stored in .vfy/cache.json
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
    /// Result of the last run
    pub last_result: CheckResult,

    /// When the check was last run
    pub last_run: DateTime<Utc>,

    /// Duration of the last run in milliseconds
    pub duration_ms: u64,

    /// Hash of all files matching cache_paths at time of last successful run
    /// Only stored on success - used to detect staleness
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_hash: Option<String>,

    /// Individual file hashes for debugging/transparency
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub file_hashes: BTreeMap<String, FileHash>,

    /// Extracted metadata values from last run
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub metadata: HashMap<String, MetadataValue>,
}

/// Result of a verification check
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CheckResult {
    /// Check passed (exit code 0)
    Pass,
    /// Check failed (non-zero exit code)
    Fail,
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
    /// Last run failed
    LastRunFailed,
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

    /// Load cache from disk, returning empty cache if file doesn't exist
    pub fn load(project_root: &Path) -> Result<Self> {
        let cache_path = project_root.join(CACHE_DIR).join(CACHE_FILE);

        if !cache_path.exists() {
            return Ok(Self::new());
        }

        let content = fs::read_to_string(&cache_path)
            .with_context(|| format!("Failed to read cache file: {}", cache_path.display()))?;

        let cache: CacheState = serde_json::from_str(&content)
            .with_context(|| format!("Failed to parse cache file: {}", cache_path.display()))?;

        // Handle version migration if needed
        if cache.version != CACHE_VERSION {
            // For now, just return empty cache on version mismatch
            return Ok(Self::new());
        }

        Ok(cache)
    }

    /// Save cache to disk atomically
    pub fn save(&self, project_root: &Path) -> Result<()> {
        let cache_dir = project_root.join(CACHE_DIR);
        fs::create_dir_all(&cache_dir)
            .with_context(|| format!("Failed to create cache directory: {}", cache_dir.display()))?;

        let cache_path = cache_dir.join(CACHE_FILE);
        let temp_path = cache_dir.join("cache.json.tmp");

        // Write to temp file
        let file = File::create(&temp_path)
            .with_context(|| format!("Failed to create temp cache file: {}", temp_path.display()))?;
        let writer = BufWriter::new(file);
        serde_json::to_writer_pretty(writer, self)
            .with_context(|| "Failed to serialize cache")?;

        // Atomic rename
        fs::rename(&temp_path, &cache_path)
            .with_context(|| format!("Failed to save cache file: {}", cache_path.display()))?;

        Ok(())
    }

    /// Determine if a check is stale based on current hash
    pub fn check_staleness(&self, check_name: &str, current_hash: &str) -> StalenessStatus {
        match self.checks.get(check_name) {
            None => StalenessStatus::NeverRun,
            Some(cache) => {
                // If last run failed, always re-run
                if cache.last_result == CheckResult::Fail {
                    return StalenessStatus::Stale {
                        reason: StalenessReason::LastRunFailed,
                    };
                }

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
        result: CheckResult,
        duration_ms: u64,
        content_hash: Option<String>,
        file_hashes: BTreeMap<String, FileHash>,
        metadata: HashMap<String, MetadataValue>,
    ) {
        let cache = CheckCache {
            last_result: result,
            last_run: Utc::now(),
            duration_ms,
            // Only store hash on success
            content_hash: if result == CheckResult::Pass {
                content_hash
            } else {
                self.checks
                    .get(check_name)
                    .and_then(|c| c.content_hash.clone())
            },
            file_hashes: if result == CheckResult::Pass {
                file_hashes
            } else {
                BTreeMap::new()
            },
            metadata,
        };
        self.checks.insert(check_name.to_string(), cache);
    }

    /// Get cached info for a check
    pub fn get(&self, check_name: &str) -> Option<&CheckCache> {
        self.checks.get(check_name)
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

/// Clean the cache directory
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
            CheckResult::Pass,
            1000,
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
            CheckResult::Pass,
            1000,
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
        cache.update("test", CheckResult::Fail, 1000, None, BTreeMap::new(), HashMap::new());

        match cache.check_staleness("test", "anyhash") {
            StalenessStatus::Stale {
                reason: StalenessReason::LastRunFailed,
            } => {}
            other => panic!("Expected Stale(LastRunFailed), got {:?}", other),
        }
    }
}
