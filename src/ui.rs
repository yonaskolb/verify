use crate::cache::StalenessReason;
use crate::metadata::{compute_delta, MetadataValue};
use crate::output::{format_duration, format_relative_time};
use chrono::{DateTime, Utc};
use console::{style, Term};
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use std::collections::HashMap;
use std::time::Duration;

/// Circle icon used for all states (colored differently)
pub const ICON_CIRCLE: &str = "\u{25CF}"; // ●

/// Terminal UI helper
pub struct Ui {
    term: Term,
    verbose: bool,
}

impl Ui {
    pub fn new(verbose: bool) -> Self {
        Self {
            term: Term::stderr(),
            verbose,
        }
    }

    /// Generate indentation string (4 spaces per level)
    fn indent_str(indent: usize) -> String {
        "    ".repeat(indent)
    }

    /// Print a subproject header
    pub fn print_subproject_header(&self, name: &str, indent: usize, has_stale: bool) {
        let prefix = Self::indent_str(indent);
        let icon_style = if has_stale {
            style(ICON_CIRCLE).yellow().bold()
        } else {
            style(ICON_CIRCLE).green().bold()
        };
        println!("{}{} {}", prefix, icon_style, style(name).bold());
    }

    /// Print status for a fresh check
    pub fn print_status_fresh(&self, name: &str, last_run: &DateTime<Utc>, duration_ms: u64) {
        self.print_status_fresh_indented(name, last_run, duration_ms, 0);
    }

    /// Print status for a fresh check with indentation
    pub fn print_status_fresh_indented(
        &self,
        name: &str,
        last_run: &DateTime<Utc>,
        duration_ms: u64,
        indent: usize,
    ) {
        let prefix = Self::indent_str(indent);
        println!(
            "{}{} {} - {} (ran {}, {})",
            prefix,
            style(ICON_CIRCLE).green().bold(),
            style(name).bold(),
            style("fresh").green(),
            format_relative_time(last_run),
            format_duration(duration_ms)
        );
    }

    /// Print status for a stale check
    pub fn print_status_stale(&self, name: &str, reason: &StalenessReason) {
        self.print_status_stale_indented(name, reason, 0);
    }

    /// Print status for a stale check with indentation
    pub fn print_status_stale_indented(&self, name: &str, reason: &StalenessReason, indent: usize) {
        let prefix = Self::indent_str(indent);
        let reason_str = match reason {
            StalenessReason::FilesChanged { changed_files } => {
                if changed_files.is_empty() {
                    "files changed".to_string()
                } else {
                    format!("{} file(s) changed", changed_files.len())
                }
            }
            StalenessReason::DependencyStale { dependency } => {
                format!("depends on: {}", dependency)
            }
            StalenessReason::LastRunFailed => "last run failed".to_string(),
            StalenessReason::NoCachePaths => "no cache paths".to_string(),
        };

        println!(
            "{}{} {} - {} ({})",
            prefix,
            style(ICON_CIRCLE).yellow().bold(),
            style(name).bold(),
            style("stale").yellow(),
            reason_str
        );
    }

    /// Print status for a never-run check
    pub fn print_status_never_run(&self, name: &str) {
        self.print_status_never_run_indented(name, 0);
    }

    /// Print status for a never-run check with indentation
    pub fn print_status_never_run_indented(&self, name: &str, indent: usize) {
        let prefix = Self::indent_str(indent);
        println!(
            "{}{} {} - {}",
            prefix,
            style(ICON_CIRCLE).dim(),
            style(name).bold(),
            style("never run").dim()
        );
    }

    /// Print when a check is skipped (cache fresh)
    pub fn print_skipped(&self, name: &str) {
        self.print_skipped_indented(name, 0);
    }

    /// Print when a check is skipped with indentation
    pub fn print_skipped_indented(&self, name: &str, indent: usize) {
        let prefix = Self::indent_str(indent);
        println!(
            "{}{} {} {}",
            prefix,
            style(ICON_CIRCLE).dim(),
            style(name).dim(),
            style("(cache fresh)").dim()
        );
    }

    /// Print when a check passes
    pub fn print_pass(&self, name: &str, duration_ms: u64) {
        self.print_pass_indented(name, duration_ms, 0);
    }

    /// Print when a check passes with indentation
    pub fn print_pass_indented(&self, name: &str, duration_ms: u64, indent: usize) {
        let prefix = Self::indent_str(indent);
        println!(
            "{}{} {} {}",
            prefix,
            style(ICON_CIRCLE).green().bold(),
            style(name).bold(),
            style(format!("({})", format_duration(duration_ms))).dim()
        );
    }

    /// Print when a check is cached (fresh)
    pub fn print_cached(&self, name: &str) {
        self.print_cached_indented(name, 0);
    }

    /// Print when a check is cached with indentation
    pub fn print_cached_indented(&self, name: &str, indent: usize) {
        let prefix = Self::indent_str(indent);
        println!(
            "{}{} {} {}",
            prefix,
            style(ICON_CIRCLE).green().bold(),
            style(name).bold(),
            style("(cached)").dim()
        );
    }

    /// Print when a check fails
    pub fn print_fail(&self, name: &str, duration_ms: u64, output: Option<&str>) {
        self.print_fail_indented(name, duration_ms, output, 0);
    }

    /// Print when a check fails with indentation
    pub fn print_fail_indented(
        &self,
        name: &str,
        duration_ms: u64,
        output: Option<&str>,
        indent: usize,
    ) {
        let prefix = Self::indent_str(indent);
        println!(
            "{}{} {} {}",
            prefix,
            style(ICON_CIRCLE).red().bold(),
            style(name).bold(),
            style(format!("({})", format_duration(duration_ms))).dim()
        );

        self.print_fail_output(output, indent);
    }

    /// Print the output from a failed check (separate from the status line)
    pub fn print_fail_output(&self, output: Option<&str>, indent: usize) {
        let prefix = Self::indent_str(indent);
        if let Some(output) = output {
            // Print indented output, limited lines
            let lines: Vec<&str> = output.lines().collect();
            let max_lines = if self.verbose { lines.len() } else { 10 };
            let output_prefix = format!("{}  ", prefix);

            for line in lines.iter().take(max_lines) {
                println!("{}{}", output_prefix, style(line).dim());
            }

            if lines.len() > max_lines {
                println!(
                    "{}{} more lines (use --verbose to see all)",
                    output_prefix,
                    style(format!("... {} ", lines.len() - max_lines)).dim()
                );
            }
        }
    }

    /// Print wave header
    pub fn print_wave_start(&self, names: &[String]) {
        self.print_wave_start_indented(names, 0);
    }

    /// Print wave header with indentation
    pub fn print_wave_start_indented(&self, names: &[String], indent: usize) {
        let prefix = Self::indent_str(indent);
        if names.len() == 1 {
            println!(
                "{}{} {}",
                prefix,
                style(ICON_CIRCLE).yellow().bold(),
                style(&names[0]).bold()
            );
        } else {
            println!(
                "{}{} {} {}",
                prefix,
                style(ICON_CIRCLE).yellow().bold(),
                names.join(", "),
                style("(parallel)").dim()
            );
        }
    }

    /// Print summary at end of run
    pub fn print_summary(&self, passed: usize, failed: usize, skipped: usize) {
        println!();

        if failed == 0 {
            if skipped > 0 && passed == 0 {
                // All cached - show a nice green message
                println!(
                    "{}: {} cached",
                    style("Summary").bold(),
                    style(skipped).green()
                );
            } else if skipped > 0 {
                println!(
                    "{}: {} passed, {} cached",
                    style("Summary").bold(),
                    style(passed).green(),
                    style(skipped).green()
                );
            } else {
                println!(
                    "{}: {} passed",
                    style("Summary").bold(),
                    style(passed).green()
                );
            }
        } else {
            println!(
                "{}: {} passed, {} failed, {} cached",
                style("Summary").bold(),
                style(passed).green(),
                style(failed).red(),
                style(skipped).dim()
            );
        }
    }

    /// Print when all checks are fresh
    pub fn print_all_fresh(&self) {
        println!("{}", style("All checks are fresh, nothing to run").green());
    }

    /// Print error message
    pub fn print_error(&self, msg: &str) {
        eprintln!("{} {}", style("error:").red().bold(), msg);
    }

    /// Print hint message
    pub fn print_hint(&self, msg: &str) {
        eprintln!("{} {}", style("hint:").yellow(), msg);
    }

    /// Print success message for init
    pub fn print_init_success(&self, path: &str) {
        println!(
            "{} Created {}",
            style(ICON_CIRCLE).green().bold(),
            style(path).bold()
        );
        println!("  Run {} to see check status", style("vfy status").cyan());
        println!("  Run {} to execute checks", style("vfy").cyan());
    }

    /// Print cache cleaned message
    pub fn print_cache_cleaned(&self, names: &[String]) {
        if names.is_empty() {
            println!("{} Cleared all cached results", style(ICON_CIRCLE).green().bold());
        } else {
            println!(
                "{} Cleared cache for: {}",
                style(ICON_CIRCLE).green().bold(),
                names.join(", ")
            );
        }
    }
}

/// Create a running indicator that shows a blue circle and can be updated in-place
pub fn create_running_indicator(name: &str, indent: usize) -> ProgressBar {
    let prefix = "    ".repeat(indent);
    let pb = ProgressBar::new_spinner();
    pb.set_style(
        ProgressStyle::default_spinner()
            .template(&format!("{}{{spinner:.yellow.bold}} {{msg}}", prefix))
            .unwrap()
            .tick_chars(&format!("{0}{0}{0}{0}{0}{0}{0}{0}{0}{0}", ICON_CIRCLE)),
    );
    pb.set_message(name.to_string());
    pb.enable_steady_tick(Duration::from_millis(100));
    pb
}

/// Finish a running indicator with pass state (green circle)
pub fn finish_pass(pb: &ProgressBar, name: &str, duration_ms: u64, indent: usize) {
    let prefix = "    ".repeat(indent);
    pb.set_style(
        ProgressStyle::default_spinner()
            .template(&format!("{}{{msg}}", prefix))
            .unwrap(),
    );
    pb.finish_with_message(format!(
        "{} {} {}",
        style(ICON_CIRCLE).green().bold(),
        style(name).bold(),
        style(format!("({})", format_duration(duration_ms))).dim()
    ));
}

/// Finish a running indicator with cached state (green circle)
pub fn finish_cached(pb: &ProgressBar, name: &str, indent: usize) {
    let prefix = "    ".repeat(indent);
    pb.set_style(
        ProgressStyle::default_spinner()
            .template(&format!("{}{{msg}}", prefix))
            .unwrap(),
    );
    pb.finish_with_message(format!(
        "{} {} {}",
        style(ICON_CIRCLE).green().bold(),
        style(name).bold(),
        style("(cached)").dim()
    ));
}

/// Finish a running indicator with fail state (red circle)
pub fn finish_fail(pb: &ProgressBar, name: &str, command: &str, duration_ms: u64, indent: usize) {
    let prefix = "    ".repeat(indent);
    pb.finish_and_clear();
    println!(
        "{}{} {} {}",
        prefix,
        style(ICON_CIRCLE).red().bold(),
        style(name).bold(),
        style(format!("({})", format_duration(duration_ms))).dim()
    );
    // Print the command in red
    println!("{}  {}", prefix, style(command).red());
}

/// Format duration with optional delta from previous run
fn format_duration_with_delta(current: u64, prev: Option<u64>) -> String {
    let current_str = format_duration(current);
    match prev {
        None => format!("({})", current_str),
        Some(p) => {
            let delta = current as i64 - p as i64;
            if delta == 0 {
                format!("({})", current_str)
            } else if delta > 0 {
                format!("({}, +{})", current_str, format_duration(delta as u64))
            } else {
                format!("({}, -{})", current_str, format_duration((-delta) as u64))
            }
        }
    }
}

/// Format a numeric delta for display
fn format_delta(d: f64) -> String {
    if d == d.trunc() {
        format!("{:.0}", d) // integer-like
    } else {
        format!("{:.1}", d) // float
    }
}

/// Print metadata with deltas, indented
fn print_metadata(
    metadata: &HashMap<String, MetadataValue>,
    prev: Option<&HashMap<String, MetadataValue>>,
    indent: usize,
) {
    let prefix = "    ".repeat(indent);
    for (key, value) in metadata {
        let delta = prev.and_then(|p| p.get(key).and_then(|pv| compute_delta(value, pv)));

        match delta {
            Some(d) if d > 0.0 => {
                println!(
                    "{}  {}: {} {}",
                    prefix,
                    style(key).dim(),
                    value,
                    style(format!("(+{})", format_delta(d))).green()
                )
            }
            Some(d) if d < 0.0 => {
                println!(
                    "{}  {}: {} {}",
                    prefix,
                    style(key).dim(),
                    value,
                    style(format!("({})", format_delta(d))).red()
                )
            }
            _ => println!("{}  {}: {}", prefix, style(key).dim(), value),
        }
    }
}

/// Finish a running indicator with pass state + metadata display
pub fn finish_pass_with_metadata(
    pb: &ProgressBar,
    name: &str,
    duration_ms: u64,
    prev_duration: Option<u64>,
    metadata: &HashMap<String, MetadataValue>,
    prev_metadata: Option<&HashMap<String, MetadataValue>>,
    indent: usize,
) {
    let prefix = "    ".repeat(indent);
    let duration_str = format_duration_with_delta(duration_ms, prev_duration);

    pb.set_style(
        ProgressStyle::default_spinner()
            .template(&format!("{}{{msg}}", prefix))
            .unwrap(),
    );
    pb.finish_with_message(format!(
        "{} {} {}",
        style(ICON_CIRCLE).green().bold(),
        style(name).bold(),
        style(duration_str).dim()
    ));

    // Print metadata below (if any)
    if !metadata.is_empty() {
        print_metadata(metadata, prev_metadata, indent);
    }
}

/// Finish a running indicator with fail state + metadata display
pub fn finish_fail_with_metadata(
    pb: &ProgressBar,
    name: &str,
    command: &str,
    duration_ms: u64,
    prev_duration: Option<u64>,
    metadata: &HashMap<String, MetadataValue>,
    prev_metadata: Option<&HashMap<String, MetadataValue>>,
    indent: usize,
) {
    let prefix = "    ".repeat(indent);
    let duration_str = format_duration_with_delta(duration_ms, prev_duration);

    pb.finish_and_clear();
    println!(
        "{}{} {} {}",
        prefix,
        style(ICON_CIRCLE).red().bold(),
        style(name).bold(),
        style(duration_str).dim()
    );

    // Print the command in red
    println!("{}  {}", prefix, style(command).red());

    // Print metadata below (if any)
    if !metadata.is_empty() {
        print_metadata(metadata, prev_metadata, indent);
    }
}

/// Multi-progress for parallel execution
pub fn create_multi_progress() -> MultiProgress {
    MultiProgress::new()
}
