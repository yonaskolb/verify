use crate::cache::{CacheState, CheckResult, StalenessReason, StalenessStatus};
use crate::config::{Config, Verification};
use crate::graph::DependencyGraph;
use crate::hasher::{compute_check_hash, find_changed_files, HashResult};
use crate::output::{CheckStatusJson, RunResults, StatusOutput};
use crate::ui::Ui;
use anyhow::Result;
use rayon::prelude::*;
use std::collections::HashMap;
use std::path::Path;
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// Result of executing a single check
#[derive(Debug)]
pub struct CheckExecution {
    pub name: String,
    pub result: CheckResult,
    pub duration_ms: u64,
    pub exit_code: Option<i32>,
    pub output: Option<String>,
    pub content_hash: String,
    pub hash_result: HashResult,
}

/// Execute a single command
fn execute_command(command: &str, project_root: &Path, _timeout_secs: Option<u64>) -> (bool, Option<i32>, String) {
    let result = Command::new("sh")
        .arg("-c")
        .arg(command)
        .current_dir(project_root)
        .output();

    match result {
        Ok(output) => {
            let success = output.status.success();
            let exit_code = output.status.code();
            let combined_output = format!(
                "{}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            (success, exit_code, combined_output)
        }
        Err(e) => (false, None, format!("Failed to execute command: {}", e)),
    }
}

/// Compute staleness for a check, considering dependencies
fn compute_staleness(
    check: &Verification,
    hash_result: &HashResult,
    cache: &CacheState,
    dep_staleness: &HashMap<String, bool>,
) -> StalenessStatus {
    // First check if any dependency is stale
    for dep in &check.depends_on {
        if dep_staleness.get(dep).copied().unwrap_or(true) {
            return StalenessStatus::Stale {
                reason: StalenessReason::DependencyStale {
                    dependency: dep.clone(),
                },
            };
        }
    }

    // Then check file changes
    let status = cache.check_staleness(&check.name, &hash_result.combined_hash);

    // Enrich with changed files if stale due to files
    match &status {
        StalenessStatus::Stale {
            reason: StalenessReason::FilesChanged { .. },
        } => {
            if let Some(cached) = cache.get(&check.name) {
                let changed = find_changed_files(&cached.file_hashes, &hash_result.file_hashes);
                StalenessStatus::Stale {
                    reason: StalenessReason::FilesChanged {
                        changed_files: changed,
                    },
                }
            } else {
                status
            }
        }
        _ => status,
    }
}

/// Run the status command
pub fn run_status(
    project_root: &Path,
    config: &Config,
    cache: &CacheState,
    json: bool,
    _detailed: bool,
) -> Result<()> {
    let graph = DependencyGraph::from_config(config)?;
    let ui = Ui::new(false);

    // Track which checks are stale (for dependency propagation)
    let mut is_stale: HashMap<String, bool> = HashMap::new();

    // Process in execution order to properly handle dependencies
    let waves = graph.execution_waves();
    let mut status_items: Vec<CheckStatusJson> = Vec::new();

    for wave in waves {
        for name in wave {
            let check = config.get(&name).unwrap();
            let hash_result = compute_check_hash(project_root, &check.cache_paths)?;
            let staleness = compute_staleness(check, &hash_result, cache, &is_stale);

            // Record staleness for dependent checks
            is_stale.insert(
                name.clone(),
                !matches!(staleness, StalenessStatus::Fresh),
            );

            if json {
                let item = match &staleness {
                    StalenessStatus::Fresh => {
                        let cached = cache.get(&name).unwrap();
                        CheckStatusJson::fresh(&name, cached)
                    }
                    StalenessStatus::Stale { reason } => {
                        CheckStatusJson::stale(&name, reason, cache.get(&name))
                    }
                    StalenessStatus::NeverRun => CheckStatusJson::never_run(&name),
                };
                status_items.push(item);
            } else {
                match &staleness {
                    StalenessStatus::Fresh => {
                        let cached = cache.get(&name).unwrap();
                        ui.print_status_fresh(&name, &cached.last_run, cached.duration_ms);
                    }
                    StalenessStatus::Stale { reason } => {
                        ui.print_status_stale(&name, reason);
                    }
                    StalenessStatus::NeverRun => {
                        ui.print_status_never_run(&name);
                    }
                }
            }
        }
    }

    if json {
        let output = StatusOutput {
            checks: status_items,
        };
        println!("{}", serde_json::to_string_pretty(&output)?);
    }

    Ok(())
}

/// Run verification checks
pub fn run_checks(
    project_root: &Path,
    config: &Config,
    cache: &mut CacheState,
    names: Vec<String>,
    run_all: bool,
    force: bool,
    json: bool,
    verbose: bool,
) -> Result<i32> {
    let graph = DependencyGraph::from_config(config)?;
    let ui = Ui::new(verbose);

    // Get checks to run (respecting dependencies)
    let checks_to_run = graph.checks_to_run(config, &names);

    // Compute hashes for all checks
    let mut hash_results: HashMap<String, HashResult> = HashMap::new();
    for check in &checks_to_run {
        let hash_result = compute_check_hash(project_root, &check.cache_paths)?;
        hash_results.insert(check.name.clone(), hash_result);
    }

    // Track staleness and execution status
    let is_stale: Arc<Mutex<HashMap<String, bool>>> = Arc::new(Mutex::new(HashMap::new()));
    let results: Arc<Mutex<RunResults>> = Arc::new(Mutex::new(RunResults::default()));

    // Execute in waves
    let waves = graph.execution_waves();

    for wave in waves {
        // Filter to only checks we're running
        let wave_checks: Vec<&Verification> = wave
            .iter()
            .filter_map(|name| checks_to_run.iter().find(|c| &c.name == name).copied())
            .collect();

        if wave_checks.is_empty() {
            continue;
        }

        // Determine which checks in this wave need to run
        let checks_needing_run: Vec<(&Verification, HashResult, StalenessStatus)> = wave_checks
            .iter()
            .map(|check| {
                let hash_result = hash_results.remove(&check.name).unwrap();
                let staleness = {
                    let stale_map = is_stale.lock().unwrap();
                    compute_staleness(check, &hash_result, cache, &stale_map)
                };
                (*check, hash_result, staleness)
            })
            .collect();

        // Print wave start for human output
        if !json {
            let running_names: Vec<String> = checks_needing_run
                .iter()
                .filter(|(_, _, s)| force || run_all || !matches!(s, StalenessStatus::Fresh))
                .map(|(c, _, _)| c.name.clone())
                .collect();

            if !running_names.is_empty() {
                ui.print_wave_start(&running_names);
            }
        }

        // Execute checks in parallel
        let check_results: Vec<Option<CheckExecution>> = checks_needing_run
            .into_par_iter()
            .map(|(check, hash_result, staleness)| {
                let should_run = force || run_all || !matches!(staleness, StalenessStatus::Fresh);

                if !should_run {
                    // Skip - cache fresh
                    let mut results = results.lock().unwrap();
                    let cached = cache.get(&check.name);
                    results.add_pass(
                        &check.name,
                        cached.map(|c| c.duration_ms).unwrap_or(0),
                        true,
                    );

                    // Mark as not stale
                    is_stale.lock().unwrap().insert(check.name.clone(), false);

                    return None;
                }

                // Execute the check
                let start = Instant::now();
                let (success, exit_code, output) =
                    execute_command(&check.command, project_root, check.timeout_secs);
                let duration = start.elapsed();
                let duration_ms = duration.as_millis() as u64;

                let result = if success {
                    CheckResult::Pass
                } else {
                    CheckResult::Fail
                };

                // Mark staleness based on result
                is_stale
                    .lock()
                    .unwrap()
                    .insert(check.name.clone(), result == CheckResult::Fail);

                Some(CheckExecution {
                    name: check.name.clone(),
                    result,
                    duration_ms,
                    exit_code,
                    output: Some(output),
                    content_hash: hash_result.combined_hash.clone(),
                    hash_result,
                })
            })
            .collect();

        // Process results and update cache
        for execution in check_results.into_iter().flatten() {
            // Update cache
            cache.update(
                &execution.name,
                execution.result,
                execution.duration_ms,
                Some(execution.content_hash),
                execution.hash_result.file_hashes,
            );

            // Update results
            let mut run_results = results.lock().unwrap();
            match execution.result {
                CheckResult::Pass => {
                    if !json {
                        ui.print_pass(&execution.name, execution.duration_ms);
                    }
                    run_results.add_pass(&execution.name, execution.duration_ms, false);
                }
                CheckResult::Fail => {
                    if !json {
                        ui.print_fail(
                            &execution.name,
                            execution.duration_ms,
                            execution.output.as_deref(),
                        );
                    }
                    run_results.add_fail(
                        &execution.name,
                        execution.duration_ms,
                        execution.exit_code,
                        execution.output,
                    );
                }
            }
        }
    }

    // Save cache
    cache.save(project_root)?;

    // Output results
    let final_results = Arc::try_unwrap(results)
        .unwrap()
        .into_inner()
        .unwrap();

    let failed_count = final_results.failed;

    if json {
        let output = final_results.to_output();
        println!("{}", serde_json::to_string_pretty(&output)?);
    } else {
        ui.print_summary(final_results.passed, final_results.failed, final_results.skipped);
    }

    // Return exit code
    if failed_count > 0 {
        Ok(1)
    } else {
        Ok(0)
    }
}
