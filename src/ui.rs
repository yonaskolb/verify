use crate::cache::StalenessReason;
use crate::output::{format_duration, format_relative_time};
use chrono::{DateTime, Utc};
use console::{style, Term};
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use std::time::Duration;

/// Icons for different states
pub const ICON_PASS: &str = "\u{2713}"; // ✓
pub const ICON_FAIL: &str = "\u{2717}"; // ✗
pub const ICON_STALE: &str = "\u{25CB}"; // ○
pub const ICON_NEVER: &str = "?";
pub const ICON_RUNNING: &str = "\u{25CF}"; // ●
pub const ICON_SKIPPED: &str = "\u{25CB}"; // ○

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

    /// Print status for a fresh check
    pub fn print_status_fresh(&self, name: &str, last_run: &DateTime<Utc>, duration_ms: u64) {
        println!(
            "{} {} - {} (ran {}, {})",
            style(ICON_PASS).green().bold(),
            style(name).bold(),
            style("fresh").green(),
            format_relative_time(last_run),
            format_duration(duration_ms)
        );
    }

    /// Print status for a stale check
    pub fn print_status_stale(&self, name: &str, reason: &StalenessReason) {
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
        };

        println!(
            "{} {} - {} ({})",
            style(ICON_STALE).yellow().bold(),
            style(name).bold(),
            style("stale").yellow(),
            reason_str
        );
    }

    /// Print status for a never-run check
    pub fn print_status_never_run(&self, name: &str) {
        println!(
            "{} {} - {}",
            style(ICON_NEVER).dim(),
            style(name).bold(),
            style("never run").dim()
        );
    }

    /// Print when a check is skipped (cache fresh)
    pub fn print_skipped(&self, name: &str) {
        println!(
            "{} {} {}",
            style(ICON_SKIPPED).dim(),
            style(name).dim(),
            style("(cache fresh)").dim()
        );
    }

    /// Print when a check passes
    pub fn print_pass(&self, name: &str, duration_ms: u64) {
        println!(
            "{} {} {}",
            style(ICON_PASS).green().bold(),
            style(name).bold(),
            style(format!("({})", format_duration(duration_ms))).dim()
        );
    }

    /// Print when a check fails
    pub fn print_fail(&self, name: &str, duration_ms: u64, output: Option<&str>) {
        println!(
            "{} {} {}",
            style(ICON_FAIL).red().bold(),
            style(name).bold(),
            style(format!("({})", format_duration(duration_ms))).dim()
        );

        if let Some(output) = output {
            // Print indented output, limited lines
            let lines: Vec<&str> = output.lines().collect();
            let max_lines = if self.verbose { lines.len() } else { 10 };

            for line in lines.iter().take(max_lines) {
                println!("  {}", style(line).dim());
            }

            if lines.len() > max_lines {
                println!(
                    "  {} more lines (use --verbose to see all)",
                    style(format!("... {} ", lines.len() - max_lines)).dim()
                );
            }
        }
    }

    /// Print wave header
    pub fn print_wave_start(&self, names: &[String]) {
        if names.len() == 1 {
            println!(
                "{} {}",
                style(ICON_RUNNING).blue().bold(),
                style(&names[0]).bold()
            );
        } else {
            println!(
                "{} {} {}",
                style(ICON_RUNNING).blue().bold(),
                names.join(", "),
                style("(parallel)").dim()
            );
        }
    }

    /// Print summary at end of run
    pub fn print_summary(&self, passed: usize, failed: usize, skipped: usize) {
        println!();
        let total = passed + failed + skipped;

        if failed == 0 {
            if skipped == total {
                println!("{}", style("All checks cached and fresh").green());
            } else {
                println!(
                    "{}: {} passed, {} skipped",
                    style("Summary").bold(),
                    style(passed).green(),
                    style(skipped).dim()
                );
            }
        } else {
            println!(
                "{}: {} passed, {} failed, {} skipped",
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
            style(ICON_PASS).green().bold(),
            style(path).bold()
        );
        println!("  Run {} to see check status", style("vfy status").cyan());
        println!("  Run {} to execute checks", style("vfy").cyan());
    }

    /// Print cache cleaned message
    pub fn print_cache_cleaned(&self, names: &[String]) {
        if names.is_empty() {
            println!("{} Cleared all cached results", style(ICON_PASS).green().bold());
        } else {
            println!(
                "{} Cleared cache for: {}",
                style(ICON_PASS).green().bold(),
                names.join(", ")
            );
        }
    }
}

/// Create a spinner for a running check
pub fn create_spinner(name: &str) -> ProgressBar {
    let pb = ProgressBar::new_spinner();
    pb.set_style(
        ProgressStyle::default_spinner()
            .template("{spinner:.blue} {msg}")
            .unwrap(),
    );
    pb.set_message(format!("Running {}...", name));
    pb.enable_steady_tick(Duration::from_millis(100));
    pb
}

/// Multi-progress for parallel execution
pub fn create_multi_progress() -> MultiProgress {
    MultiProgress::new()
}
