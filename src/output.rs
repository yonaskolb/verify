use crate::cache::StalenessReason;
use chrono::{DateTime, Utc};
use serde::Serialize;

/// JSON output for `vfy status`
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
    pub last_run: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
}

impl CheckStatusJson {
    pub fn fresh(name: &str, cache: &crate::cache::CheckCache) -> Self {
        Self {
            name: name.to_string(),
            status: "fresh".to_string(),
            reason: None,
            stale_dependency: None,
            changed_files: None,
            last_run: Some(cache.last_run),
            duration_ms: Some(cache.duration_ms),
        }
    }

    pub fn stale(name: &str, reason: &StalenessReason, cache: Option<&crate::cache::CheckCache>) -> Self {
        let (reason_str, stale_dep, changed_files) = match reason {
            StalenessReason::FilesChanged { changed_files } => {
                (Some("files_changed".to_string()), None, Some(changed_files.clone()))
            }
            StalenessReason::DependencyStale { dependency } => {
                (Some("dependency_stale".to_string()), Some(dependency.clone()), None)
            }
            StalenessReason::LastRunFailed => (Some("last_run_failed".to_string()), None, None),
            StalenessReason::NoCachePaths => (Some("no_cache_paths".to_string()), None, None),
        };

        Self {
            name: name.to_string(),
            status: "stale".to_string(),
            reason: reason_str,
            stale_dependency: stale_dep,
            changed_files,
            last_run: cache.map(|c| c.last_run),
            duration_ms: cache.map(|c| c.duration_ms),
        }
    }

    pub fn never_run(name: &str) -> Self {
        Self {
            name: name.to_string(),
            status: "never_run".to_string(),
            reason: None,
            stale_dependency: None,
            changed_files: None,
            last_run: None,
            duration_ms: None,
        }
    }
}

/// JSON output for `vfy run`
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
    pub duration_ms: u64,
    pub cached: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
}

impl CheckRunJson {
    pub fn pass(name: &str, duration_ms: u64, cached: bool) -> Self {
        Self {
            name: name.to_string(),
            result: "pass".to_string(),
            duration_ms,
            cached,
            exit_code: Some(0),
            output: None,
        }
    }

    pub fn fail(name: &str, duration_ms: u64, exit_code: Option<i32>, output: Option<String>) -> Self {
        Self {
            name: name.to_string(),
            result: "fail".to_string(),
            duration_ms,
            cached: false,
            exit_code,
            output,
        }
    }

    pub fn skipped(name: &str, last_duration_ms: Option<u64>) -> Self {
        Self {
            name: name.to_string(),
            result: "skipped".to_string(),
            duration_ms: last_duration_ms.unwrap_or(0),
            cached: true,
            exit_code: None,
            output: None,
        }
    }
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
    pub fn add_pass(&mut self, name: &str, duration_ms: u64, cached: bool) {
        if cached {
            self.results
                .push(RunItemJson::Check(CheckRunJson::skipped(name, Some(duration_ms))));
            self.skipped += 1;
        } else {
            self.results
                .push(RunItemJson::Check(CheckRunJson::pass(name, duration_ms, cached)));
            self.passed += 1;
        }
    }

    pub fn add_fail(
        &mut self,
        name: &str,
        duration_ms: u64,
        exit_code: Option<i32>,
        output: Option<String>,
    ) {
        self.results
            .push(RunItemJson::Check(CheckRunJson::fail(name, duration_ms, exit_code, output)));
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

        self.results.push(RunItemJson::Subproject(SubprojectRunJson::new(
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

/// Format relative time for human display
pub fn format_relative_time(time: &DateTime<Utc>) -> String {
    let now = Utc::now();
    let duration = now.signed_duration_since(*time);

    if duration.num_seconds() < 60 {
        "just now".to_string()
    } else if duration.num_minutes() < 60 {
        format!("{}m ago", duration.num_minutes())
    } else if duration.num_hours() < 24 {
        format!("{}h ago", duration.num_hours())
    } else {
        format!("{}d ago", duration.num_days())
    }
}
