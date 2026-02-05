use crate::cache::{CacheState, StalenessReason, StalenessStatus};
use crate::config::{Config, Subproject, Verification, VerificationItem};
use crate::graph::DependencyGraph;
use crate::hasher::{HashResult, compute_check_hash, find_changed_files};
use crate::metadata::{MetadataValue, extract_metadata};
use crate::output::{
    CheckStatusJson, RunResults, StatusItemJson, StatusOutput, SubprojectStatusJson,
};
use crate::ui::{
    Ui, create_running_indicator, finish_cached, finish_fail_with_metadata,
    finish_pass_with_metadata,
};
use anyhow::Result;
use std::collections::{BTreeMap, HashMap};
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Instant;

/// Result of executing a single check
#[allow(dead_code)]
#[derive(Debug)]
pub struct CheckExecution {
    pub name: String,
    pub success: bool,
    pub duration_ms: u64,
    pub exit_code: Option<i32>,
    pub output: Option<String>,
    pub content_hash: String,
    pub hash_result: HashResult,
}

/// Execute a single command
fn execute_command(
    command: &str,
    project_root: &Path,
    _timeout_secs: Option<u64>,
    verbose: bool,
    env_vars: &[(&str, &str)],
) -> (bool, Option<i32>, String) {
    if verbose {
        // Stream output in real-time while also capturing it
        let mut cmd = Command::new("sh");
        cmd.arg("-c")
            .arg(command)
            .current_dir(project_root)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (key, value) in env_vars {
            cmd.env(key, value);
        }
        let mut child = match cmd.spawn() {
            Ok(child) => child,
            Err(e) => return (false, None, format!("Failed to execute command: {}", e)),
        };

        let mut combined_output = String::new();

        // Read stdout
        if let Some(stdout) = child.stdout.take() {
            let reader = BufReader::new(stdout);
            for line in reader.lines() {
                if let Ok(line) = line {
                    println!("{}", line);
                    combined_output.push_str(&line);
                    combined_output.push('\n');
                }
            }
        }

        // Read stderr
        if let Some(stderr) = child.stderr.take() {
            let reader = BufReader::new(stderr);
            for line in reader.lines() {
                if let Ok(line) = line {
                    eprintln!("{}", line);
                    combined_output.push_str(&line);
                    combined_output.push('\n');
                }
            }
        }

        let status = child.wait();
        match status {
            Ok(status) => (status.success(), status.code(), combined_output),
            Err(e) => (false, None, format!("Failed to wait for command: {}", e)),
        }
    } else {
        // Original behavior: capture all output at once
        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg(command).current_dir(project_root);
        for (key, value) in env_vars {
            cmd.env(key, value);
        }
        let result = cmd.output();

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

    // Then check file changes and config changes
    let config_hash = check.config_hash();
    let status = cache.check_staleness(&check.name, &hash_result.combined_hash, &config_hash);

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

/// Get list of stale files by comparing cached vs current file hashes directly.
/// Used in per_file mode to preserve progress even when overall check failed.
fn get_stale_files_from_cache(
    cached_file_hashes: &std::collections::BTreeMap<String, String>,
    current_hashes: &HashResult,
) -> Vec<String> {
    current_hashes
        .file_hashes
        .iter()
        .filter(|(path, current_hash)| {
            // File is stale if not in cache or hash changed
            match cached_file_hashes.get(*path) {
                None => true,
                Some(cached) => cached != *current_hash,
            }
        })
        .map(|(path, _)| path.clone())
        .collect()
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
                            ui.print_status_fresh_indented(&v.name, indent);
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
                let sub_items = run_status_subproject(project_root, s, ui, json, indent)?;

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
    let subproject_config_path = subproject_dir.join("verify.yaml");

    let sub_config = Config::load_with_base(&subproject_config_path, &subproject_dir)?;
    let sub_cache = CacheState::load(&subproject_dir)?;

    // For human output, print subproject header
    if !json {
        // Determine if subproject has any stale checks
        let has_stale = check_has_stale(&subproject_dir, &sub_config, &sub_cache)?;
        ui.print_subproject_header(&subproject.name, indent, has_stale);
    }

    // Recursively process subproject
    run_status_recursive(
        &subproject_dir,
        &sub_config,
        &sub_cache,
        ui,
        json,
        indent + 1,
    )
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
        let sub_config_path = subproject_dir.join("verify.yaml");
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

    // Clean up orphaned cache entries (checks no longer in config)
    let valid_names: std::collections::HashSet<String> = config
        .verifications
        .iter()
        .map(|item| item.name().to_string())
        .collect();
    cache.cleanup_orphaned(&valid_names);

    // Save cache for root project
    cache.save(project_root)?;

    let failed_count = final_results.failed;
    let total_duration_ms = start_time.elapsed().as_millis() as u64;

    if json {
        let output = final_results.to_output();
        println!("{}", serde_json::to_string_pretty(&output)?);
    } else {
        ui.print_summary(
            final_results.passed,
            final_results.failed,
            final_results.skipped,
            total_duration_ms,
        );
    }

    // Return exit code
    if failed_count > 0 { Ok(1) } else { Ok(0) }
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
                    let sub_results =
                        run_checks_subproject(project_root, sub, &[], force, json, ui, indent)?;
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
                let sub_results =
                    run_checks_subproject(project_root, s, names, force, json, ui, indent)?;
                let had_failures = sub_results.failed > 0;
                executed.insert(s.name.clone(), had_failures);
                results.add_subproject(&s.name, s.path.to_string_lossy().as_ref(), sub_results);
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
    let dep_failed = check
        .depends_on
        .iter()
        .any(|dep| executed.get(dep).copied().unwrap_or(false));

    // Compute staleness
    let hash_result = compute_check_hash(project_root, &check.cache_paths)?;

    // Build staleness map from executed checks
    let dep_staleness: HashMap<String, bool> =
        executed.iter().map(|(k, v)| (k.clone(), *v)).collect();

    let staleness = if dep_failed {
        StalenessStatus::Stale {
            reason: StalenessReason::DependencyStale {
                dependency: check
                    .depends_on
                    .iter()
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
            let cached_metadata = cached.map(|c| &c.metadata);
            finish_cached(
                &pb,
                &check.name,
                cached_metadata.unwrap_or(&BTreeMap::new()),
                indent,
            );
        }
        results.add_skipped(&check.name);
        executed.insert(check.name.clone(), false);
        return Ok(());
    }

    // Get previous cache for metadata deltas
    let prev_cache = cache.get(&check.name);
    let prev_metadata = prev_cache.map(|c| c.metadata.clone());

    // Handle per_file mode
    if check.per_file {
        return execute_per_file(
            project_root,
            check,
            cache,
            &hash_result,
            &staleness,
            json,
            ui,
            indent,
            executed,
            results,
            prev_metadata,
        );
    }

    // In verbose mode, print start indicator instead of using progress bar
    // (progress bar redraws interfere with streamed output)
    let pb = if !json && !ui.is_verbose() {
        Some(create_running_indicator(&check.name, indent))
    } else {
        if !json && ui.is_verbose() {
            ui.print_running(&check.name, indent);
        }
        None
    };

    // Execute the check
    let start = Instant::now();
    let (success, exit_code, output) = execute_command(
        &check.command,
        project_root,
        check.timeout_secs,
        ui.is_verbose(),
        &[],
    );
    let duration = start.elapsed();
    let duration_ms = duration.as_millis() as u64;

    // Extract metadata from output (only on success)
    let metadata = if success && !check.metadata.is_empty() {
        extract_metadata(&output, &check.metadata)
    } else {
        BTreeMap::new()
    };

    // Update cache
    let config_hash = check.config_hash();
    cache.update(
        &check.name,
        success,
        config_hash,
        Some(hash_result.combined_hash.clone()),
        hash_result.file_hashes,
        metadata.clone(),
        check.per_file,
    );

    // Record result
    executed.insert(check.name.clone(), !success);

    if success {
        if let Some(pb) = pb {
            finish_pass_with_metadata(
                &pb,
                &check.name,
                duration_ms,
                &metadata,
                prev_metadata.as_ref(),
                indent,
            );
        } else if !json {
            // Verbose mode: print completion line
            ui.print_pass_indented(&check.name, duration_ms, indent);
        }
        results.add_pass(
            &check.name,
            duration_ms,
            false,
            &metadata,
            prev_metadata.as_ref(),
        );
    } else {
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
        } else if !json {
            // Verbose mode: print failure line
            ui.print_fail_indented(&check.name, duration_ms, None, indent);
        }
        // Print error output separately (can't be part of progress bar)
        // In verbose mode, output was already streamed, so skip
        if !json && !ui.is_verbose() {
            ui.print_fail_output(Some(&output), indent);
        }
        results.add_fail(
            &check.name,
            duration_ms,
            exit_code,
            Some(output),
            &metadata,
            prev_metadata.as_ref(),
        );
    }

    // Save cache immediately after check completes
    cache.save(project_root)?;

    Ok(())
}

/// Execute a verification in per_file mode
fn execute_per_file(
    project_root: &Path,
    check: &Verification,
    cache: &mut CacheState,
    hash_result: &HashResult,
    _staleness: &StalenessStatus,
    json: bool,
    ui: &Ui,
    indent: usize,
    executed: &mut HashMap<String, bool>,
    results: &mut RunResults,
    prev_metadata: Option<BTreeMap<String, MetadataValue>>,
) -> Result<()> {
    let config_hash = check.config_hash();

    // For per_file mode, compute stale files by comparing cached vs current file hashes.
    // This preserves progress when overall check failed - only re-run files that
    // haven't passed yet (or whose content changed).
    // Also invalidate all progress if config changed.
    let cached = cache.get(&check.name);
    let config_changed = cached
        .and_then(|c| c.config_hash.as_ref())
        .map(|h| h != &config_hash)
        .unwrap_or(true);

    let cached_file_hashes = if config_changed {
        // Config changed, invalidate all file hashes
        std::collections::BTreeMap::new()
    } else {
        cached.map(|c| &c.file_hashes).cloned().unwrap_or_default()
    };
    let stale_files = get_stale_files_from_cache(&cached_file_hashes, hash_result);
    let total_files = hash_result.file_hashes.len();
    let fresh_count = total_files.saturating_sub(stale_files.len());
    // If no stale files - show cached count and return early
    if stale_files.is_empty() {
        if !json {
            ui.print_per_file_cached(&check.name, total_files, indent);
        }
        results.add_skipped(&check.name);
        executed.insert(check.name.clone(), false);
        return Ok(());
    }

    // Show cached count first if any files are fresh
    if fresh_count > 0 && !json {
        ui.print_per_file_cached(&check.name, fresh_count, indent);
    }

    let start = Instant::now();
    let mut last_output = String::new();
    let mut failed_files: Vec<(String, Option<i32>, String)> = Vec::new();

    // Run command for each stale file
    for file_path in &stale_files {
        // Create progress bar showing "check_name: file_path"
        let display_name = format!("{}: {}", check.name, file_path);
        let file_pb = if !json && !ui.is_verbose() {
            Some(create_running_indicator(&display_name, indent))
        } else {
            if !json && ui.is_verbose() {
                ui.print_running(&display_name, indent);
            }
            None
        };

        let env_vars = [("VERIFY_FILE", file_path.as_str())];

        let file_start = Instant::now();
        let (success, exit_code, output) = execute_command(
            &check.command,
            project_root,
            check.timeout_secs,
            ui.is_verbose(),
            &env_vars,
        );
        let file_duration_ms = file_start.elapsed().as_millis() as u64;

        if success {
            // Finish file progress bar as passed
            if let Some(pb) = file_pb {
                let empty = BTreeMap::new();
                finish_pass_with_metadata(
                    &pb,
                    &display_name,
                    file_duration_ms,
                    &empty,
                    None,
                    indent,
                );
            } else if !json {
                // Verbose mode: print completion line
                ui.print_pass_indented(&display_name, file_duration_ms, indent);
            }

            // Update the file hash in cache (partial progress) and save immediately
            // so progress is preserved if process is interrupted
            if let Some(file_hash) = hash_result.file_hashes.get(file_path) {
                cache.update_per_file_hash(&check.name, &config_hash, file_path, file_hash.clone());
                cache.save(project_root)?;
            }
        } else {
            // Finish file progress bar as failed
            if let Some(pb) = file_pb {
                finish_fail_with_metadata(
                    &pb,
                    &display_name,
                    &check.command,
                    file_duration_ms,
                    &BTreeMap::new(),
                    None,
                    indent,
                );
            } else if !json {
                // Verbose mode: print failure line
                ui.print_fail_indented(&display_name, file_duration_ms, None, indent);
            }

            // Print failure output (in verbose mode, output was already streamed)
            if !json && !ui.is_verbose() {
                ui.print_fail_output(Some(&output), indent);
            }

            // Track the failure but continue processing other files
            failed_files.push((file_path.clone(), exit_code, output.clone()));
        }

        last_output = output;
    }

    // If any files failed, mark check as failed
    if !failed_files.is_empty() {
        let total_duration_ms = start.elapsed().as_millis() as u64;
        cache.mark_per_file_failed(&check.name, &config_hash);
        executed.insert(check.name.clone(), true);

        // Combine all failure outputs
        let combined_output = failed_files
            .iter()
            .map(|(file, _, output)| format!("=== {} ===\n{}", file, output))
            .collect::<Vec<_>>()
            .join("\n");

        let empty_metadata = BTreeMap::new();
        results.add_fail(
            &check.name,
            total_duration_ms,
            failed_files.first().and_then(|(_, code, _)| *code),
            Some(combined_output),
            &empty_metadata,
            prev_metadata.as_ref(),
        );

        // Save cache immediately after per_file check fails
        cache.save(project_root)?;

        return Ok(());
    }

    // Extract metadata from last output (if configured)
    let metadata = if !check.metadata.is_empty() {
        extract_metadata(&last_output, &check.metadata)
    } else {
        BTreeMap::new()
    };

    // Finalize cache - all files passed
    let total_duration_ms = start.elapsed().as_millis() as u64;
    cache.finalize_per_file(
        &check.name,
        &config_hash,
        hash_result.combined_hash.clone(),
        hash_result.file_hashes.clone(),
        metadata.clone(),
    );

    executed.insert(check.name.clone(), false);
    results.add_pass(
        &check.name,
        total_duration_ms,
        false,
        &metadata,
        prev_metadata.as_ref(),
    );

    // Save cache immediately after per_file check completes
    cache.save(project_root)?;

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
    let subproject_config_path = subproject_dir.join("verify.yaml");

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

    // Clean up orphaned cache entries
    let valid_names: std::collections::HashSet<String> = sub_config
        .verifications
        .iter()
        .map(|item| item.name().to_string())
        .collect();
    sub_cache.cleanup_orphaned(&valid_names);

    // Save subproject cache
    sub_cache.save(&subproject_dir)?;

    Ok(sub_results)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hasher::HashResult;
    use std::collections::BTreeMap;

    // Helper to create a basic Verification for testing
    fn make_verification(
        name: &str,
        cache_paths: Vec<&str>,
        depends_on: Vec<&str>,
    ) -> Verification {
        Verification {
            name: name.to_string(),
            command: "echo test".to_string(),
            cache_paths: cache_paths.into_iter().map(|s| s.to_string()).collect(),
            depends_on: depends_on.into_iter().map(|s| s.to_string()).collect(),
            timeout_secs: None,
            metadata: HashMap::new(),
            per_file: false,
        }
    }

    // Helper to create a HashResult
    fn make_hash_result(combined: &str, files: Vec<(&str, &str)>) -> HashResult {
        HashResult {
            combined_hash: combined.to_string(),
            file_hashes: files
                .into_iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        }
    }

    // ==================== get_stale_files_from_cache tests ====================

    #[test]
    fn test_get_stale_files_from_cache_all_new() {
        let cached: BTreeMap<String, String> = BTreeMap::new();
        let mut current_hashes = BTreeMap::new();
        current_hashes.insert("file1.txt".to_string(), "abc123".to_string());
        current_hashes.insert("file2.txt".to_string(), "def456".to_string());

        let hash_result = HashResult {
            combined_hash: "combined".to_string(),
            file_hashes: current_hashes,
        };

        let stale = get_stale_files_from_cache(&cached, &hash_result);
        assert_eq!(stale.len(), 2);
        assert!(stale.contains(&"file1.txt".to_string()));
        assert!(stale.contains(&"file2.txt".to_string()));
    }

    #[test]
    fn test_get_stale_files_from_cache_all_fresh() {
        let mut cached: BTreeMap<String, String> = BTreeMap::new();
        cached.insert("file1.txt".to_string(), "abc123".to_string());

        let mut current_hashes = BTreeMap::new();
        current_hashes.insert("file1.txt".to_string(), "abc123".to_string());

        let hash_result = HashResult {
            combined_hash: "combined".to_string(),
            file_hashes: current_hashes,
        };

        let stale = get_stale_files_from_cache(&cached, &hash_result);
        assert!(stale.is_empty());
    }

    #[test]
    fn test_get_stale_files_from_cache_partial_progress() {
        // Simulates: file1 passed (in cache), file2 failed (not in cache)
        let mut cached: BTreeMap<String, String> = BTreeMap::new();
        cached.insert("file1.txt".to_string(), "abc123".to_string());

        let mut current_hashes = BTreeMap::new();
        current_hashes.insert("file1.txt".to_string(), "abc123".to_string());
        current_hashes.insert("file2.txt".to_string(), "def456".to_string());

        let hash_result = HashResult {
            combined_hash: "combined".to_string(),
            file_hashes: current_hashes,
        };

        let stale = get_stale_files_from_cache(&cached, &hash_result);
        assert_eq!(stale.len(), 1);
        assert_eq!(stale[0], "file2.txt");
    }

    #[test]
    fn test_get_stale_files_from_cache_hash_changed() {
        let mut cached: BTreeMap<String, String> = BTreeMap::new();
        cached.insert("file1.txt".to_string(), "old_hash".to_string());

        let mut current_hashes = BTreeMap::new();
        current_hashes.insert("file1.txt".to_string(), "new_hash".to_string());

        let hash_result = HashResult {
            combined_hash: "combined".to_string(),
            file_hashes: current_hashes,
        };

        let stale = get_stale_files_from_cache(&cached, &hash_result);
        assert_eq!(stale.len(), 1);
        assert_eq!(stale[0], "file1.txt");
    }

    // ==================== compute_staleness tests ====================

    #[test]
    fn test_compute_staleness_dependency_stale() {
        // When a dependency is marked as stale, the check should be stale
        let check = make_verification("test", vec!["src/**/*.rs"], vec!["build"]);
        let hash_result = make_hash_result("hash123", vec![("src/main.rs", "abc")]);
        let cache = CacheState::new();

        let mut dep_staleness = HashMap::new();
        dep_staleness.insert("build".to_string(), true); // dependency is stale

        let result = compute_staleness(&check, &hash_result, &cache, &dep_staleness);

        match result {
            StalenessStatus::Stale {
                reason: StalenessReason::DependencyStale { dependency },
            } => {
                assert_eq!(dependency, "build");
            }
            other => panic!("Expected DependencyStale, got {:?}", other),
        }
    }

    #[test]
    fn test_compute_staleness_dependency_fresh() {
        // When dependency is fresh (false), check proceeds to file-based staleness
        let check = make_verification("test", vec!["src/**/*.rs"], vec!["build"]);
        let hash_result = make_hash_result("hash123", vec![("src/main.rs", "abc")]);
        let cache = CacheState::new();

        let mut dep_staleness = HashMap::new();
        dep_staleness.insert("build".to_string(), false); // dependency is fresh

        let result = compute_staleness(&check, &hash_result, &cache, &dep_staleness);

        // Should be NeverRun since cache is empty (not DependencyStale)
        assert_eq!(result, StalenessStatus::NeverRun);
    }

    #[test]
    fn test_compute_staleness_multiple_dependencies_one_stale() {
        // If any dependency is stale, check is stale
        let check = make_verification("test", vec!["src/**/*.rs"], vec!["build", "lint", "format"]);
        let hash_result = make_hash_result("hash123", vec![]);
        let cache = CacheState::new();

        let mut dep_staleness = HashMap::new();
        dep_staleness.insert("build".to_string(), false);
        dep_staleness.insert("lint".to_string(), true); // this one is stale
        dep_staleness.insert("format".to_string(), false);

        let result = compute_staleness(&check, &hash_result, &cache, &dep_staleness);

        match result {
            StalenessStatus::Stale {
                reason: StalenessReason::DependencyStale { dependency },
            } => {
                assert_eq!(dependency, "lint");
            }
            other => panic!("Expected DependencyStale(lint), got {:?}", other),
        }
    }

    #[test]
    fn test_compute_staleness_unknown_dependency_treated_as_stale() {
        // Unknown dependencies default to stale (true)
        let check = make_verification("test", vec!["src/**/*.rs"], vec!["unknown_dep"]);
        let hash_result = make_hash_result("hash123", vec![]);
        let cache = CacheState::new();

        let dep_staleness = HashMap::new(); // empty - unknown_dep not present

        let result = compute_staleness(&check, &hash_result, &cache, &dep_staleness);

        match result {
            StalenessStatus::Stale {
                reason: StalenessReason::DependencyStale { dependency },
            } => {
                assert_eq!(dependency, "unknown_dep");
            }
            other => panic!("Expected DependencyStale(unknown_dep), got {:?}", other),
        }
    }

    #[test]
    fn test_compute_staleness_no_cache_paths() {
        // When cache_paths is empty, always return NoCachePaths
        let check = make_verification("test", vec![], vec![]); // no cache_paths
        let hash_result = make_hash_result("hash123", vec![]);
        let cache = CacheState::new();
        let dep_staleness = HashMap::new();

        let result = compute_staleness(&check, &hash_result, &cache, &dep_staleness);

        match result {
            StalenessStatus::Stale {
                reason: StalenessReason::NoCachePaths,
            } => {}
            other => panic!("Expected NoCachePaths, got {:?}", other),
        }
    }

    #[test]
    fn test_compute_staleness_no_cache_paths_with_fresh_deps() {
        // Even with fresh dependencies, no cache_paths means always run
        let check = make_verification("test", vec![], vec!["build"]);
        let hash_result = make_hash_result("hash123", vec![]);
        let cache = CacheState::new();

        let mut dep_staleness = HashMap::new();
        dep_staleness.insert("build".to_string(), false);

        let result = compute_staleness(&check, &hash_result, &cache, &dep_staleness);

        match result {
            StalenessStatus::Stale {
                reason: StalenessReason::NoCachePaths,
            } => {}
            other => panic!("Expected NoCachePaths, got {:?}", other),
        }
    }

    #[test]
    fn test_compute_staleness_never_run() {
        // Check has never been run (not in cache)
        let check = make_verification("new_check", vec!["src/**/*.rs"], vec![]);
        let hash_result = make_hash_result("hash123", vec![("src/main.rs", "abc")]);
        let cache = CacheState::new(); // empty cache
        let dep_staleness = HashMap::new();

        let result = compute_staleness(&check, &hash_result, &cache, &dep_staleness);

        assert_eq!(result, StalenessStatus::NeverRun);
    }

    #[test]
    fn test_compute_staleness_fresh() {
        // Check is fresh when hash matches and config hasn't changed
        let check = make_verification("test", vec!["src/**/*.rs"], vec![]);
        let config_hash = check.config_hash();
        let hash_result = make_hash_result("hash123", vec![("src/main.rs", "abc")]);

        let mut cache = CacheState::new();
        cache.update(
            "test",
            true,
            config_hash,
            Some("hash123".to_string()),
            BTreeMap::new(),
            BTreeMap::new(),
            false,
        );

        let dep_staleness = HashMap::new();

        let result = compute_staleness(&check, &hash_result, &cache, &dep_staleness);

        assert_eq!(result, StalenessStatus::Fresh);
    }

    #[test]
    fn test_compute_staleness_files_changed() {
        // Check is stale when files have changed
        let check = make_verification("test", vec!["src/**/*.rs"], vec![]);
        let config_hash = check.config_hash();
        let hash_result = make_hash_result("new_hash", vec![("src/main.rs", "new_content")]);

        let mut cache = CacheState::new();
        let mut old_file_hashes = BTreeMap::new();
        old_file_hashes.insert("src/main.rs".to_string(), "old_content".to_string());
        cache.update(
            "test",
            true,
            config_hash,
            Some("old_hash".to_string()),
            old_file_hashes,
            BTreeMap::new(),
            true, // per_file to store file_hashes
        );

        let dep_staleness = HashMap::new();

        let result = compute_staleness(&check, &hash_result, &cache, &dep_staleness);

        match result {
            StalenessStatus::Stale {
                reason: StalenessReason::FilesChanged { changed_files },
            } => {
                // find_changed_files returns "M file" for modified files
                assert!(changed_files.contains(&"M src/main.rs".to_string()));
            }
            other => panic!("Expected FilesChanged, got {:?}", other),
        }
    }

    #[test]
    fn test_compute_staleness_config_changed() {
        // Check is stale when config has changed
        let check = make_verification("test", vec!["src/**/*.rs"], vec![]);
        let hash_result = make_hash_result("hash123", vec![("src/main.rs", "abc")]);

        let mut cache = CacheState::new();
        // Store with different config hash
        cache.update(
            "test",
            true,
            "old_config_hash".to_string(),
            Some("hash123".to_string()),
            BTreeMap::new(),
            BTreeMap::new(),
            false,
        );

        let dep_staleness = HashMap::new();

        let result = compute_staleness(&check, &hash_result, &cache, &dep_staleness);

        match result {
            StalenessStatus::Stale {
                reason: StalenessReason::ConfigChanged,
            } => {}
            other => panic!("Expected ConfigChanged, got {:?}", other),
        }
    }

    #[test]
    fn test_compute_staleness_after_failure() {
        // After a failed run, check should need to run again
        let check = make_verification("test", vec!["src/**/*.rs"], vec![]);
        let config_hash = check.config_hash();
        let hash_result = make_hash_result("hash123", vec![("src/main.rs", "abc")]);

        let mut cache = CacheState::new();
        // Update with failure (success = false clears content_hash)
        cache.update(
            "test",
            false,
            config_hash,
            Some("hash123".to_string()),
            BTreeMap::new(),
            BTreeMap::new(),
            false,
        );

        let dep_staleness = HashMap::new();

        let result = compute_staleness(&check, &hash_result, &cache, &dep_staleness);

        // After failure, content_hash is None, so it's NeverRun
        assert_eq!(result, StalenessStatus::NeverRun);
    }

    #[test]
    fn test_compute_staleness_dependency_checked_before_cache_paths() {
        // Dependency staleness should be checked before checking empty cache_paths
        // This is important: even with no cache_paths, if dep is stale we report DependencyStale
        let check = make_verification("test", vec![], vec!["build"]); // no cache_paths
        let hash_result = make_hash_result("hash123", vec![]);
        let cache = CacheState::new();

        let mut dep_staleness = HashMap::new();
        dep_staleness.insert("build".to_string(), true); // dependency stale

        let result = compute_staleness(&check, &hash_result, &cache, &dep_staleness);

        // Should be DependencyStale, not NoCachePaths
        match result {
            StalenessStatus::Stale {
                reason: StalenessReason::DependencyStale { dependency },
            } => {
                assert_eq!(dependency, "build");
            }
            other => panic!("Expected DependencyStale, got {:?}", other),
        }
    }

    #[test]
    fn test_compute_staleness_changed_files_enrichment() {
        // When stale due to files, the changed files list should be populated
        let check = make_verification("test", vec!["src/**/*.rs"], vec![]);
        let config_hash = check.config_hash();

        // Current state has 3 files, 2 changed
        let hash_result = make_hash_result(
            "new_combined",
            vec![
                ("src/main.rs", "new_main"),
                ("src/lib.rs", "same_lib"),
                ("src/util.rs", "new_util"),
            ],
        );

        let mut cache = CacheState::new();
        let mut old_hashes = BTreeMap::new();
        old_hashes.insert("src/main.rs".to_string(), "old_main".to_string());
        old_hashes.insert("src/lib.rs".to_string(), "same_lib".to_string());
        old_hashes.insert("src/util.rs".to_string(), "old_util".to_string());

        cache.update(
            "test",
            true,
            config_hash,
            Some("old_combined".to_string()),
            old_hashes,
            BTreeMap::new(),
            true, // per_file to track file_hashes
        );

        let dep_staleness = HashMap::new();

        let result = compute_staleness(&check, &hash_result, &cache, &dep_staleness);

        match result {
            StalenessStatus::Stale {
                reason: StalenessReason::FilesChanged { changed_files },
            } => {
                // find_changed_files returns "M file" for modified files
                assert_eq!(changed_files.len(), 2);
                assert!(changed_files.contains(&"M src/main.rs".to_string()));
                assert!(changed_files.contains(&"M src/util.rs".to_string()));
                // lib.rs unchanged should not be in the list
                assert!(!changed_files.iter().any(|f| f.contains("lib.rs")));
            }
            other => panic!("Expected FilesChanged with 2 files, got {:?}", other),
        }
    }

    #[test]
    fn test_compute_staleness_no_dependencies() {
        // Check with no dependencies should proceed directly to cache check
        let check = make_verification("test", vec!["src/**/*.rs"], vec![]); // no depends_on
        let config_hash = check.config_hash();
        let hash_result = make_hash_result("hash123", vec![("src/main.rs", "abc")]);

        let mut cache = CacheState::new();
        cache.update(
            "test",
            true,
            config_hash,
            Some("hash123".to_string()),
            BTreeMap::new(),
            BTreeMap::new(),
            false,
        );

        let dep_staleness = HashMap::new();

        let result = compute_staleness(&check, &hash_result, &cache, &dep_staleness);

        assert_eq!(result, StalenessStatus::Fresh);
    }

    #[test]
    fn test_compute_staleness_all_dependencies_fresh() {
        // When all dependencies are fresh, proceed to file-based staleness
        let check = make_verification("test", vec!["src/**/*.rs"], vec!["build", "lint"]);
        let config_hash = check.config_hash();
        let hash_result = make_hash_result("hash123", vec![("src/main.rs", "abc")]);

        let mut cache = CacheState::new();
        cache.update(
            "test",
            true,
            config_hash,
            Some("hash123".to_string()),
            BTreeMap::new(),
            BTreeMap::new(),
            false,
        );

        let mut dep_staleness = HashMap::new();
        dep_staleness.insert("build".to_string(), false);
        dep_staleness.insert("lint".to_string(), false);

        let result = compute_staleness(&check, &hash_result, &cache, &dep_staleness);

        assert_eq!(result, StalenessStatus::Fresh);
    }

    // ==================== execute_command tests ====================
    // These tests verify actual command execution behavior

    #[test]
    fn test_execute_command_success() {
        let temp_dir = tempfile::tempdir().unwrap();
        let (success, exit_code, output) =
            execute_command("echo 'hello world'", temp_dir.path(), None, false, &[]);

        assert!(success);
        assert_eq!(exit_code, Some(0));
        assert!(output.contains("hello world"));
    }

    #[test]
    fn test_execute_command_failure() {
        let temp_dir = tempfile::tempdir().unwrap();
        let (success, exit_code, _output) =
            execute_command("exit 1", temp_dir.path(), None, false, &[]);

        assert!(!success);
        assert_eq!(exit_code, Some(1));
    }

    #[test]
    fn test_execute_command_nonzero_exit_code() {
        let temp_dir = tempfile::tempdir().unwrap();
        let (success, exit_code, _output) =
            execute_command("exit 42", temp_dir.path(), None, false, &[]);

        assert!(!success);
        assert_eq!(exit_code, Some(42));
    }

    #[test]
    fn test_execute_command_captures_stdout() {
        let temp_dir = tempfile::tempdir().unwrap();
        let (success, _, output) =
            execute_command("echo 'stdout test'", temp_dir.path(), None, false, &[]);

        assert!(success);
        assert!(output.contains("stdout test"));
    }

    #[test]
    fn test_execute_command_captures_stderr() {
        let temp_dir = tempfile::tempdir().unwrap();
        let (success, _, output) =
            execute_command("echo 'stderr test' >&2", temp_dir.path(), None, false, &[]);

        assert!(success);
        assert!(output.contains("stderr test"));
    }

    #[test]
    fn test_execute_command_captures_both_stdout_stderr() {
        let temp_dir = tempfile::tempdir().unwrap();
        let (success, _, output) = execute_command(
            "echo 'stdout'; echo 'stderr' >&2",
            temp_dir.path(),
            None,
            false,
            &[],
        );

        assert!(success);
        assert!(output.contains("stdout"));
        assert!(output.contains("stderr"));
    }

    #[test]
    fn test_execute_command_with_env_var() {
        let temp_dir = tempfile::tempdir().unwrap();
        let env_vars = [("MY_TEST_VAR", "test_value")];
        let (success, _, output) =
            execute_command("echo $MY_TEST_VAR", temp_dir.path(), None, false, &env_vars);

        assert!(success);
        assert!(output.contains("test_value"));
    }

    #[test]
    fn test_execute_command_with_verify_file_env() {
        // Test the specific VERIFY_FILE env var used in per_file mode
        let temp_dir = tempfile::tempdir().unwrap();
        let env_vars = [("VERIFY_FILE", "src/main.rs")];
        let (success, _, output) =
            execute_command("echo $VERIFY_FILE", temp_dir.path(), None, false, &env_vars);

        assert!(success);
        assert!(output.contains("src/main.rs"));
    }

    #[test]
    fn test_execute_command_multiple_env_vars() {
        let temp_dir = tempfile::tempdir().unwrap();
        let env_vars = [("VAR1", "value1"), ("VAR2", "value2")];
        let (success, _, output) =
            execute_command("echo $VAR1 $VAR2", temp_dir.path(), None, false, &env_vars);

        assert!(success);
        assert!(output.contains("value1"));
        assert!(output.contains("value2"));
    }

    #[test]
    fn test_execute_command_working_directory() {
        let temp_dir = tempfile::tempdir().unwrap();
        // Create a file in the temp directory
        std::fs::write(temp_dir.path().join("test.txt"), "content").unwrap();

        let (success, _, output) =
            execute_command("ls test.txt", temp_dir.path(), None, false, &[]);

        assert!(success);
        assert!(output.contains("test.txt"));
    }

    #[test]
    fn test_execute_command_multiline_output() {
        let temp_dir = tempfile::tempdir().unwrap();
        let (success, _, output) = execute_command(
            "echo 'line1'; echo 'line2'; echo 'line3'",
            temp_dir.path(),
            None,
            false,
            &[],
        );

        assert!(success);
        assert!(output.contains("line1"));
        assert!(output.contains("line2"));
        assert!(output.contains("line3"));
    }

    #[test]
    fn test_execute_command_verbose_mode() {
        let temp_dir = tempfile::tempdir().unwrap();
        // In verbose mode, output should still be captured
        let (success, exit_code, output) =
            execute_command("echo 'verbose test'", temp_dir.path(), None, true, &[]);

        assert!(success);
        assert_eq!(exit_code, Some(0));
        assert!(output.contains("verbose test"));
    }

    #[test]
    fn test_execute_command_empty_output() {
        let temp_dir = tempfile::tempdir().unwrap();
        let (success, _, output) = execute_command("true", temp_dir.path(), None, false, &[]);

        assert!(success);
        assert!(output.is_empty() || output.trim().is_empty());
    }

    #[test]
    fn test_execute_command_special_characters_in_output() {
        let temp_dir = tempfile::tempdir().unwrap();
        let (success, _, output) = execute_command(
            r#"echo 'special: $VAR "quoted" `backticks`'"#,
            temp_dir.path(),
            None,
            false,
            &[],
        );

        assert!(success);
        assert!(output.contains("special:"));
    }

    #[test]
    fn test_execute_command_piped_commands() {
        let temp_dir = tempfile::tempdir().unwrap();
        let (success, _, output) = execute_command(
            "echo 'abc\ndef\nghi' | grep 'def'",
            temp_dir.path(),
            None,
            false,
            &[],
        );

        assert!(success);
        assert!(output.contains("def"));
        assert!(!output.contains("abc"));
    }

    #[test]
    fn test_execute_command_command_not_found() {
        let temp_dir = tempfile::tempdir().unwrap();
        let (success, exit_code, _output) = execute_command(
            "nonexistent_command_12345",
            temp_dir.path(),
            None,
            false,
            &[],
        );

        assert!(!success);
        // Exit code 127 typically means command not found
        assert!(exit_code == Some(127) || exit_code.is_some());
    }

    #[test]
    fn test_execute_command_reads_file_in_workdir() {
        let temp_dir = tempfile::tempdir().unwrap();
        let file_path = temp_dir.path().join("input.txt");
        std::fs::write(&file_path, "file contents here").unwrap();

        let (success, _, output) =
            execute_command("cat input.txt", temp_dir.path(), None, false, &[]);

        assert!(success);
        assert!(output.contains("file contents here"));
    }

    #[test]
    fn test_execute_command_writes_file_in_workdir() {
        let temp_dir = tempfile::tempdir().unwrap();

        let (success, _, _output) = execute_command(
            "echo 'written content' > output.txt",
            temp_dir.path(),
            None,
            false,
            &[],
        );

        assert!(success);

        let written_content = std::fs::read_to_string(temp_dir.path().join("output.txt")).unwrap();
        assert!(written_content.contains("written content"));
    }

    #[test]
    fn test_execute_command_env_var_in_complex_command() {
        let temp_dir = tempfile::tempdir().unwrap();
        let file_path = temp_dir.path().join("test_file.txt");
        std::fs::write(&file_path, "test content").unwrap();

        let env_vars = [("VERIFY_FILE", "test_file.txt")];
        let (success, _, output) =
            execute_command("cat $VERIFY_FILE", temp_dir.path(), None, false, &env_vars);

        assert!(success);
        assert!(output.contains("test content"));
    }
}
