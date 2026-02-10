use crate::cache::{UnverifiedReason, VerificationStatus};
use crate::metadata::{MetadataValue, compute_delta};
use serde::Serialize;
use std::collections::{BTreeMap, HashMap};

/// JSON output for `verify status`
#[derive(Debug, Serialize)]
pub struct StatusOutput {
    pub checks: Vec<StatusItemJson>,
}

/// Either a check status or a subproject with nested checks
#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum StatusItemJson {
    Check(CheckStatusJson),
    Subproject(SubprojectStatusJson),
}

/// JSON output for a subproject in status
#[derive(Debug, Serialize)]
pub struct SubprojectStatusJson {
    pub name: String,
    #[serde(rename = "type")]
    pub item_type: String,
    pub path: String,
    pub checks: Vec<StatusItemJson>,
}

impl SubprojectStatusJson {
    pub fn new(name: &str, path: &str, checks: Vec<StatusItemJson>) -> Self {
        Self {
            name: name.to_string(),
            item_type: "subproject".to_string(),
            path: path.to_string(),
            checks,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct CheckStatusJson {
    pub name: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stale_dependency: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub changed_files: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<HashMap<String, serde_json::Value>>,
}

impl CheckStatusJson {
    pub fn from_status(
        name: &str,
        status: &VerificationStatus,
        cache: Option<&crate::cache::CheckCache>,
    ) -> Self {
        let metadata = cache
            .filter(|c| !c.metadata.is_empty())
            .map(|c| {
                c.metadata
                    .iter()
                    .map(|(k, v)| {
                        let json_value = match v {
                            MetadataValue::Integer(i) => serde_json::Value::Number((*i).into()),
                            MetadataValue::Float(f) => serde_json::Number::from_f64(*f)
                                .map(serde_json::Value::Number)
                                .unwrap_or(serde_json::Value::Null),
                            MetadataValue::String(s) => serde_json::Value::String(s.clone()),
                        };
                        (k.clone(), json_value)
                    })
                    .collect()
            });

        match status {
            VerificationStatus::Verified => Self {
                name: name.to_string(),
                status: "verified".to_string(),
                reason: None,
                stale_dependency: None,
                changed_files: None,
                metadata,
            },
            VerificationStatus::Unverified { reason } => {
                let (reason_str, stale_dep, changed_files) = match reason {
                    UnverifiedReason::FilesChanged { changed_files } => (
                        Some("files_changed".to_string()),
                        None,
                        Some(changed_files.clone()),
                    ),
                    UnverifiedReason::DependencyUnverified { dependency } => (
                        Some("dependency_unverified".to_string()),
                        Some(dependency.clone()),
                        None,
                    ),
                    UnverifiedReason::ConfigChanged => {
                        (Some("config_changed".to_string()), None, None)
                    }
                    UnverifiedReason::NeverRun => (Some("never_run".to_string()), None, None),
                };

                Self {
                    name: name.to_string(),
                    status: "unverified".to_string(),
                    reason: reason_str,
                    stale_dependency: stale_dep,
                    changed_files,
                    metadata,
                }
            }
            VerificationStatus::Untracked => Self {
                name: name.to_string(),
                status: "untracked".to_string(),
                reason: None,
                stale_dependency: None,
                changed_files: None,
                metadata: None,
            },
        }
    }
}

/// JSON output for `verify run`
#[derive(Debug, Serialize)]
pub struct RunOutput {
    pub results: Vec<RunItemJson>,
    pub summary: RunSummary,
}

/// Either a check result or a subproject with nested results
#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum RunItemJson {
    Check(CheckRunJson),
    Subproject(SubprojectRunJson),
}

/// JSON output for a subproject in run results
#[derive(Debug, Clone, Serialize)]
pub struct SubprojectRunJson {
    pub name: String,
    #[serde(rename = "type")]
    pub item_type: String,
    pub path: String,
    pub results: Vec<RunItemJson>,
    pub summary: RunSummary,
}

impl SubprojectRunJson {
    pub fn new(name: &str, path: &str, results: Vec<RunItemJson>, summary: RunSummary) -> Self {
        Self {
            name: name.to_string(),
            item_type: "subproject".to_string(),
            path: path.to_string(),
            results,
            summary,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct CheckRunJson {
    pub name: String,
    pub result: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    pub cached: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<HashMap<String, serde_json::Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata_deltas: Option<HashMap<String, f64>>,
}

impl CheckRunJson {
    pub fn pass(
        name: &str,
        duration_ms: u64,
        cached: bool,
        metadata: &BTreeMap<String, MetadataValue>,
        prev_metadata: Option<&BTreeMap<String, MetadataValue>>,
    ) -> Self {
        let (metadata_json, metadata_deltas) = convert_metadata(metadata, prev_metadata);

        Self {
            name: name.to_string(),
            result: "pass".to_string(),
            duration_ms: Some(duration_ms),
            cached,
            exit_code: Some(0),
            output: None,
            metadata: metadata_json,
            metadata_deltas,
        }
    }

    pub fn fail(
        name: &str,
        duration_ms: u64,
        exit_code: Option<i32>,
        output: Option<String>,
        metadata: &BTreeMap<String, MetadataValue>,
        prev_metadata: Option<&BTreeMap<String, MetadataValue>>,
    ) -> Self {
        let (metadata_json, metadata_deltas) = convert_metadata(metadata, prev_metadata);

        Self {
            name: name.to_string(),
            result: "fail".to_string(),
            duration_ms: Some(duration_ms),
            cached: false,
            exit_code,
            output,
            metadata: metadata_json,
            metadata_deltas,
        }
    }

    pub fn skipped(name: &str) -> Self {
        Self {
            name: name.to_string(),
            result: "skipped".to_string(),
            duration_ms: None,
            cached: true,
            exit_code: None,
            output: None,
            metadata: None,
            metadata_deltas: None,
        }
    }
}

/// Convert metadata to JSON format and compute deltas
fn convert_metadata(
    metadata: &BTreeMap<String, MetadataValue>,
    prev_metadata: Option<&BTreeMap<String, MetadataValue>>,
) -> (
    Option<HashMap<String, serde_json::Value>>,
    Option<HashMap<String, f64>>,
) {
    if metadata.is_empty() {
        return (None, None);
    }

    let mut json_metadata = HashMap::new();
    let mut deltas = HashMap::new();

    for (key, value) in metadata {
        // Convert to JSON value
        let json_value = match value {
            MetadataValue::Integer(i) => serde_json::Value::Number((*i).into()),
            MetadataValue::Float(f) => serde_json::Number::from_f64(*f)
                .map(serde_json::Value::Number)
                .unwrap_or(serde_json::Value::Null),
            MetadataValue::String(s) => serde_json::Value::String(s.clone()),
        };
        json_metadata.insert(key.clone(), json_value);

        // Compute delta if previous value exists
        if let Some(prev) = prev_metadata {
            if let Some(prev_value) = prev.get(key) {
                if let Some(delta) = compute_delta(value, prev_value) {
                    deltas.insert(key.clone(), delta);
                }
            }
        }
    }

    let metadata_deltas = if deltas.is_empty() {
        None
    } else {
        Some(deltas)
    };
    (Some(json_metadata), metadata_deltas)
}

#[derive(Debug, Clone, Serialize)]
pub struct RunSummary {
    pub total: usize,
    pub passed: usize,
    pub failed: usize,
    pub skipped: usize,
}

/// Collected results during a run
#[derive(Debug, Default, Clone)]
pub struct RunResults {
    pub results: Vec<RunItemJson>,
    pub passed: usize,
    pub failed: usize,
    pub skipped: usize,
}

impl RunResults {
    pub fn add_pass(
        &mut self,
        name: &str,
        duration_ms: u64,
        cached: bool,
        metadata: &BTreeMap<String, MetadataValue>,
        prev_metadata: Option<&BTreeMap<String, MetadataValue>>,
    ) {
        self.results.push(RunItemJson::Check(CheckRunJson::pass(
            name,
            duration_ms,
            cached,
            metadata,
            prev_metadata,
        )));
        self.passed += 1;
    }

    pub fn add_skipped(&mut self, name: &str) {
        self.results
            .push(RunItemJson::Check(CheckRunJson::skipped(name)));
        self.skipped += 1;
    }

    pub fn add_fail(
        &mut self,
        name: &str,
        duration_ms: u64,
        exit_code: Option<i32>,
        output: Option<String>,
        metadata: &BTreeMap<String, MetadataValue>,
        prev_metadata: Option<&BTreeMap<String, MetadataValue>>,
    ) {
        self.results.push(RunItemJson::Check(CheckRunJson::fail(
            name,
            duration_ms,
            exit_code,
            output,
            metadata,
            prev_metadata,
        )));
        self.failed += 1;
    }

    pub fn add_subproject(&mut self, name: &str, path: &str, sub_results: RunResults) {
        self.passed += sub_results.passed;
        self.failed += sub_results.failed;
        self.skipped += sub_results.skipped;

        let summary = RunSummary {
            total: sub_results.passed + sub_results.failed + sub_results.skipped,
            passed: sub_results.passed,
            failed: sub_results.failed,
            skipped: sub_results.skipped,
        };

        self.results
            .push(RunItemJson::Subproject(SubprojectRunJson::new(
                name,
                path,
                sub_results.results,
                summary,
            )));
    }

    pub fn to_output(self) -> RunOutput {
        let total = self.passed + self.failed + self.skipped;
        RunOutput {
            results: self.results,
            summary: RunSummary {
                total,
                passed: self.passed,
                failed: self.failed,
                skipped: self.skipped,
            },
        }
    }

    #[allow(dead_code)]
    pub fn to_summary(&self) -> RunSummary {
        RunSummary {
            total: self.passed + self.failed + self.skipped,
            passed: self.passed,
            failed: self.failed,
            skipped: self.skipped,
        }
    }
}

/// Format duration for human display
pub fn format_duration(ms: u64) -> String {
    if ms < 1000 {
        format!("{}ms", ms)
    } else if ms < 60000 {
        format!("{:.1}s", ms as f64 / 1000.0)
    } else {
        let mins = ms / 60000;
        let secs = (ms % 60000) / 1000;
        format!("{}m{}s", mins, secs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::CheckCache;

    fn make_cache_with_metadata(metadata: BTreeMap<String, MetadataValue>) -> CheckCache {
        CheckCache {
            config_hash: Some("confighash".to_string()),
            content_hash: Some("contenthash".to_string()),
            file_hashes: BTreeMap::new(),
            metadata,
        }
    }

    #[test]
    fn test_status_json_verified_with_metadata() {
        let mut metadata = BTreeMap::new();
        metadata.insert("coverage".to_string(), MetadataValue::Float(85.5));
        metadata.insert("tests".to_string(), MetadataValue::Integer(42));
        let cache = make_cache_with_metadata(metadata);

        let result =
            CheckStatusJson::from_status("build", &VerificationStatus::Verified, Some(&cache));

        assert_eq!(result.status, "verified");
        let meta = result.metadata.expect("metadata should be present");
        assert_eq!(meta.get("tests"), Some(&serde_json::json!(42)));
        assert_eq!(meta.get("coverage"), Some(&serde_json::json!(85.5)));
    }

    #[test]
    fn test_status_json_verified_without_metadata() {
        let cache = make_cache_with_metadata(BTreeMap::new());

        let result =
            CheckStatusJson::from_status("build", &VerificationStatus::Verified, Some(&cache));

        assert_eq!(result.status, "verified");
        assert!(result.metadata.is_none());
    }

    #[test]
    fn test_status_json_verified_no_cache() {
        let result = CheckStatusJson::from_status("build", &VerificationStatus::Verified, None);

        assert_eq!(result.status, "verified");
        assert!(result.metadata.is_none());
    }

    #[test]
    fn test_status_json_unverified_with_metadata() {
        let mut metadata = BTreeMap::new();
        metadata.insert("lines".to_string(), MetadataValue::Integer(100));
        let cache = make_cache_with_metadata(metadata);

        let status = VerificationStatus::Unverified {
            reason: UnverifiedReason::FilesChanged {
                changed_files: vec!["src/main.rs".to_string()],
            },
        };

        let result = CheckStatusJson::from_status("build", &status, Some(&cache));

        assert_eq!(result.status, "unverified");
        assert_eq!(result.reason.as_deref(), Some("files_changed"));
        let meta = result.metadata.expect("metadata should be present");
        assert_eq!(meta.get("lines"), Some(&serde_json::json!(100)));
    }

    #[test]
    fn test_status_json_untracked_no_metadata() {
        let mut metadata = BTreeMap::new();
        metadata.insert("lines".to_string(), MetadataValue::Integer(100));
        let cache = make_cache_with_metadata(metadata);

        let result =
            CheckStatusJson::from_status("build", &VerificationStatus::Untracked, Some(&cache));

        assert_eq!(result.status, "untracked");
        assert!(result.metadata.is_none());
    }

    #[test]
    fn test_status_json_metadata_string_value() {
        let mut metadata = BTreeMap::new();
        metadata.insert("version".to_string(), MetadataValue::String("1.2.3".to_string()));
        let cache = make_cache_with_metadata(metadata);

        let result =
            CheckStatusJson::from_status("build", &VerificationStatus::Verified, Some(&cache));

        let meta = result.metadata.expect("metadata should be present");
        assert_eq!(meta.get("version"), Some(&serde_json::json!("1.2.3")));
    }

    #[test]
    fn test_status_json_serialization_omits_null_metadata() {
        let result = CheckStatusJson::from_status("build", &VerificationStatus::Verified, None);

        let json = serde_json::to_value(&result).unwrap();
        assert!(!json.as_object().unwrap().contains_key("metadata"));
    }

    #[test]
    fn test_status_json_serialization_includes_metadata() {
        let mut metadata = BTreeMap::new();
        metadata.insert("count".to_string(), MetadataValue::Integer(5));
        let cache = make_cache_with_metadata(metadata);

        let result =
            CheckStatusJson::from_status("build", &VerificationStatus::Verified, Some(&cache));

        let json = serde_json::to_value(&result).unwrap();
        let obj = json.as_object().unwrap();
        assert!(obj.contains_key("metadata"));
        assert_eq!(obj["metadata"]["count"], serde_json::json!(5));
    }
}
