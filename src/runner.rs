use crate::cache::{CacheState, CheckResult, StalenessReason, StalenessStatus};
use crate::config::{Config, Subproject, Verification, VerificationItem};
use crate::graph::DependencyGraph;
use crate::hasher::{compute_check_hash, find_changed_files, HashResult};
use crate::metadata::{extract_metadata, MetadataValue};
use crate::output::{
    CheckStatusJson, RunResults, StatusItemJson, StatusOutput, SubprojectStatusJson,
};
use crate::ui::{
    create_running_indicator, finish_cached, finish_fail_with_metadata,
    finish_pass_with_metadata, Ui,
};
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

    // If no cache_paths defined, always run (rely on command's own caching)
    if check.cache_paths.is_empty() {
        return StalenessStatus::Stale {
            reason: StalenessReason::NoCachePaths,
        };
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
    let ui = Ui::new(false);
    let status_items = run_status_recursive(project_root, config, cache, &ui, json, 0)?;

    if json {
        let output = StatusOutput {
            checks: status_items,
        };
        println!("{}", serde_json::to_string_pretty(&output)?);
    }

    Ok(())
}

/// Recursively process status for config and all subprojects
fn run_status_recursive(
    project_root: &Path,
    config: &Config,
    cache: &CacheState,
    ui: &Ui,
    json: bool,
    indent: usize,
) -> Result<Vec<StatusItemJson>> {
    let graph = DependencyGraph::from_config(config)?;

    // Track which checks are stale (for dependency propagation)
    let mut is_stale: HashMap<String, bool> = HashMap::new();

    // Process verifications in execution order
    let waves = graph.execution_waves();
    let mut status_items: Vec<StatusItemJson> = Vec::new();

    // Build a map of verification name to position in config for ordering
    let mut verification_order: HashMap<String, usize> = HashMap::new();
    for (idx, item) in config.verifications.iter().enumerate() {
        verification_order.insert(item.name().to_string(), idx);
    }

    // Process all verifications first (in wave order for dependency propagation)
    let mut verification_statuses: HashMap<String, (StalenessStatus, Option<CheckStatusJson>)> =
        HashMap::new();

    for wave in waves {
        for name in wave {
            let check = config.get(&name).unwrap();
            let hash_result = compute_check_hash(project_root, &check.cache_paths)?;
            let staleness = compute_staleness(check, &hash_result, cache, &is_stale);

            // Record staleness for dependent checks
            is_stale.insert(name.clone(), !matches!(staleness, StalenessStatus::Fresh));

            let json_item = match &staleness {
                StalenessStatus::Fresh => {
                    let cached = cache.get(&name).unwrap();
                    Some(CheckStatusJson::fresh(&name, cached))
                }
                StalenessStatus::Stale { reason } => {
                    Some(CheckStatusJson::stale(&name, reason, cache.get(&name)))
                }
                StalenessStatus::NeverRun => Some(CheckStatusJson::never_run(&name)),
            };

            verification_statuses.insert(name.clone(), (staleness, json_item));
        }
    }

    // Now iterate through config items in order to preserve ordering
    for item in &config.verifications {
        match item {
            VerificationItem::Verification(v) => {
                let (staleness, json_item) = verification_statuses.remove(&v.name).unwrap();

                if json {
                    if let Some(item) = json_item {
                        status_items.push(StatusItemJson::Check(item));
                    }
                } else {
                    match &staleness {
                        StalenessStatus::Fresh => {
                            let cached = cache.get(&v.name).unwrap();
                            ui.print_status_fresh_indented(
                                &v.name,
                                &cached.last_run,
                                cached.duration_ms,
                                indent,
                            );
                        }
                        StalenessStatus::Stale { reason } => {
                            ui.print_status_stale_indented(&v.name, reason, indent);
                        }
                        StalenessStatus::NeverRun => {
                            ui.print_status_never_run_indented(&v.name, indent);
                        }
                    }
                }
            }
            VerificationItem::Subproject(s) => {
                let sub_items =
                    run_status_subproject(project_root, s, ui, json, indent)?;

                if json {
                    status_items.push(StatusItemJson::Subproject(SubprojectStatusJson::new(
                        &s.name,
                        s.path.to_string_lossy().as_ref(),
                        sub_items,
                    )));
                }
            }
        }
    }

    Ok(status_items)
}

/// Run status for a subproject
fn run_status_subproject(
    parent_root: &Path,
    subproject: &Subproject,
    ui: &Ui,
    json: bool,
    indent: usize,
) -> Result<Vec<StatusItemJson>> {
    let subproject_dir = parent_root.join(&subproject.path);
    let subproject_config_path = subproject_dir.join("vfy.yaml");

    let sub_config = Config::load_with_base(&subproject_config_path, &subproject_dir)?;
    let sub_cache = CacheState::load(&subproject_dir)?;

    // For human output, print subproject header
    if !json {
        // Determine if subproject has any stale checks
        let has_stale = check_has_stale(&subproject_dir, &sub_config, &sub_cache)?;
        ui.print_subproject_header(&subproject.name, indent, has_stale);
    }

    // Recursively process subproject
    run_status_recursive(&subproject_dir, &sub_config, &sub_cache, ui, json, indent + 1)
}

/// Check if a config has any stale checks
fn check_has_stale(project_root: &Path, config: &Config, cache: &CacheState) -> Result<bool> {
    let graph = DependencyGraph::from_config(config)?;
    let mut is_stale: HashMap<String, bool> = HashMap::new();

    for wave in graph.execution_waves() {
        for name in wave {
            if let Some(check) = config.get(&name) {
                let hash_result = compute_check_hash(project_root, &check.cache_paths)?;
                let staleness = compute_staleness(check, &hash_result, cache, &is_stale);
                let stale = !matches!(staleness, StalenessStatus::Fresh);
                is_stale.insert(name.clone(), stale);
                if stale {
                    return Ok(true);
                }
            }
        }
    }

    // Also check subprojects
    for subproject in config.subprojects() {
        let subproject_dir = project_root.join(&subproject.path);
        let sub_config_path = subproject_dir.join("vfy.yaml");
        if sub_config_path.exists() {
            let sub_config = Config::load_with_base(&sub_config_path, &subproject_dir)?;
            let sub_cache = CacheState::load(&subproject_dir)?;
            if check_has_stale(&subproject_dir, &sub_config, &sub_cache)? {
                return Ok(true);
            }
        }
    }

    Ok(false)
}

/// Run verification checks
pub fn run_checks(
    project_root: &Path,
    config: &Config,
    cache: &mut CacheState,
    names: Vec<String>,
    force: bool,
    json: bool,
    verbose: bool,
) -> Result<i32> {
    let start_time = Instant::now();
    let ui = Ui::new(verbose);
    let final_results =
        run_checks_recursive(project_root, config, cache, &names, force, json, &ui, 0)?;

    // Save cache for root project
    cache.save(project_root)?;

    let failed_count = final_results.failed;
    let total_duration_ms = start_time.elapsed().as_millis() as u64;

    if json {
        let output = final_results.to_output();
        println!("{}", serde_json::to_string_pretty(&output)?);
    } else {
        ui.print_summary(final_results.passed, final_results.failed, final_results.skipped, total_duration_ms);
    }

    // Return exit code
    if failed_count > 0 {
        Ok(1)
    } else {
        Ok(0)
    }
}

/// Recursively run checks for config and all subprojects
fn run_checks_recursive(
    project_root: &Path,
    config: &Config,
    cache: &mut CacheState,
    names: &[String],
    force: bool,
    json: bool,
    ui: &Ui,
    indent: usize,
) -> Result<RunResults> {
    let mut final_results = RunResults::default();

    // Track which items have been executed and their staleness
    let mut executed: HashMap<String, bool> = HashMap::new(); // name -> had_failures

    // Process items in config order, but handle dependencies first
    for item in &config.verifications {
        execute_item_with_deps(
            project_root,
            config,
            cache,
            item,
            names,
            force,
            json,
            ui,
            indent,
            &mut executed,
            &mut final_results,
        )?;
    }

    Ok(final_results)
}

/// Execute an item (verification or subproject) and its dependencies
fn execute_item_with_deps(
    project_root: &Path,
    config: &Config,
    cache: &mut CacheState,
    item: &VerificationItem,
    names: &[String],
    force: bool,
    json: bool,
    ui: &Ui,
    indent: usize,
    executed: &mut HashMap<String, bool>,
    results: &mut RunResults,
) -> Result<()> {
    let item_name = item.name().to_string();

    // Skip if already executed
    if executed.contains_key(&item_name) {
        return Ok(());
    }

    // Skip if not in requested names (when names is non-empty)
    if !names.is_empty() && !names.contains(&item_name) {
        return Ok(());
    }

    // For verifications, first execute any dependencies
    if let VerificationItem::Verification(v) = item {
        for dep_name in &v.depends_on {
            // Check if dependency is a subproject
            if let Some(sub) = config.get_subproject(dep_name) {
                // Execute subproject if not already done (run all checks in the subproject)
                if !executed.contains_key(dep_name) {
                    let sub_results = run_checks_subproject(
                        project_root,
                        sub,
                        &[],
                        force,
                        json,
                        ui,
                        indent,
                    )?;
                    let had_failures = sub_results.failed > 0;
                    executed.insert(dep_name.clone(), had_failures);
                    results.add_subproject(
                        dep_name,
                        sub.path.to_string_lossy().as_ref(),
                        sub_results,
                    );
                }
            } else if let Some(dep_v) = config.get(dep_name) {
                // Execute verification dependency if not already done
                if !executed.contains_key(dep_name) {
                    execute_verification(
                        project_root,
                        dep_v,
                        cache,
                        force,
                        json,
                        ui,
                        indent,
                        executed,
                        results,
                    )?;
                }
            }
        }
    }

    // Now execute the item itself
    match item {
        VerificationItem::Verification(v) => {
            // Skip if not in requested names (when names is non-empty)
            if !names.is_empty() && !names.contains(&v.name) {
                return Ok(());
            }
            execute_verification(
                project_root,
                v,
                cache,
                force,
                json,
                ui,
                indent,
                executed,
                results,
            )?;
        }
        VerificationItem::Subproject(s) => {
            // Skip if not in requested names (when names is non-empty)
            if !names.is_empty() && !names.contains(&s.name) {
                return Ok(());
            }
            if !executed.contains_key(&s.name) {
                let sub_results = run_checks_subproject(
                    project_root,
                    s,
                    names,
                    force,
                    json,
                    ui,
                    indent,
                )?;
                let had_failures = sub_results.failed > 0;
                executed.insert(s.name.clone(), had_failures);
                results.add_subproject(
                    &s.name,
                    s.path.to_string_lossy().as_ref(),
                    sub_results,
                );
            }
        }
    }

    Ok(())
}

/// Execute a single verification
fn execute_verification(
    project_root: &Path,
    check: &Verification,
    cache: &mut CacheState,
    force: bool,
    json: bool,
    ui: &Ui,
    indent: usize,
    executed: &mut HashMap<String, bool>,
    results: &mut RunResults,
) -> Result<()> {
    // Skip if already executed
    if executed.contains_key(&check.name) {
        return Ok(());
    }

    // Check if any dependency failed
    let dep_failed = check.depends_on.iter().any(|dep| {
        executed.get(dep).copied().unwrap_or(false)
    });

    // Compute staleness
    let hash_result = compute_check_hash(project_root, &check.cache_paths)?;

    // Build staleness map from executed checks
    let dep_staleness: HashMap<String, bool> = executed
        .iter()
        .map(|(k, v)| (k.clone(), *v))
        .collect();

    let staleness = if dep_failed {
        StalenessStatus::Stale {
            reason: StalenessReason::DependencyStale {
                dependency: check.depends_on.iter()
                    .find(|d| executed.get(*d).copied().unwrap_or(false))
                    .unwrap_or(&check.depends_on[0])
                    .clone(),
            },
        }
    } else {
        compute_staleness(check, &hash_result, cache, &dep_staleness)
    };

    let should_run = force || !matches!(staleness, StalenessStatus::Fresh);

    if !should_run {
        // Skip - cache fresh, show with in-place green indicator
        let cached = cache.get(&check.name);
        if !json {
            let pb = create_running_indicator(&check.name, indent);
            finish_cached(&pb, &check.name, indent);
        }
        // For cached (skipped) checks, pass empty metadata since we didn't run the command
        let empty_metadata = HashMap::new();
        results.add_pass(
            &check.name,
            cached.map(|c| c.duration_ms).unwrap_or(0),
            true,
            &empty_metadata,
            None,
            None,
        );
        executed.insert(check.name.clone(), false);
        return Ok(());
    }

    // Create running indicator (blue circle that updates in place)
    let pb = if !json {
        Some(create_running_indicator(&check.name, indent))
    } else {
        None
    };

    // Get previous cache for duration and metadata deltas
    let prev_cache = cache.get(&check.name);
    let prev_duration = prev_cache.map(|c| c.duration_ms);
    let prev_metadata = prev_cache.map(|c| c.metadata.clone());

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

    // Extract metadata from output
    let metadata = if !check.metadata.is_empty() {
        extract_metadata(&output, &check.metadata)
    } else {
        HashMap::new()
    };

    // Update cache
    cache.update(
        &check.name,
        result,
        duration_ms,
        Some(hash_result.combined_hash.clone()),
        hash_result.file_hashes,
        metadata.clone(),
    );

    // Record result
    executed.insert(check.name.clone(), result == CheckResult::Fail);

    match result {
        CheckResult::Pass => {
            if let Some(pb) = pb {
                finish_pass_with_metadata(
                    &pb,
                    &check.name,
                    duration_ms,
                    &metadata,
                    prev_metadata.as_ref(),
                    indent,
                );
            }
            results.add_pass(&check.name, duration_ms, false, &metadata, prev_metadata.as_ref(), prev_duration);
        }
        CheckResult::Fail => {
            if let Some(pb) = pb {
                finish_fail_with_metadata(
                    &pb,
                    &check.name,
                    &check.command,
                    duration_ms,
                    &metadata,
                    prev_metadata.as_ref(),
                    indent,
                );
            }
            // Print error output separately (can't be part of progress bar)
            if !json {
                ui.print_fail_output(Some(&output), indent);
            }
            results.add_fail(&check.name, duration_ms, exit_code, Some(output), &metadata, prev_metadata.as_ref(), prev_duration);
        }
    }

    Ok(())
}

/// Run checks for a subproject
fn run_checks_subproject(
    parent_root: &Path,
    subproject: &Subproject,
    names: &[String],
    force: bool,
    json: bool,
    ui: &Ui,
    indent: usize,
) -> Result<RunResults> {
    let subproject_dir = parent_root.join(&subproject.path);
    let subproject_config_path = subproject_dir.join("vfy.yaml");

    let sub_config = Config::load_with_base(&subproject_config_path, &subproject_dir)?;
    let mut sub_cache = CacheState::load(&subproject_dir)?;

    // For human output, print subproject header
    if !json {
        ui.print_subproject_header(&subproject.name, indent, false);
    }

    // Recursively run checks with the same name filter
    let sub_results = run_checks_recursive(
        &subproject_dir,
        &sub_config,
        &mut sub_cache,
        names,
        force,
        json,
        ui,
        indent + 1,
    )?;

    // Save subproject cache
    sub_cache.save(&subproject_dir)?;

    Ok(sub_results)
}
