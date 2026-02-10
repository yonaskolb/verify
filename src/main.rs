mod cache;
mod cli;
mod config;
mod graph;
mod hasher;
mod metadata;
mod output;
mod runner;
mod ui;

use anyhow::Result;
use clap::Parser;
use cli::{Cli, Commands};
use std::path::Path;
use std::process::ExitCode;

fn main() -> ExitCode {
    match run() {
        Ok(code) => ExitCode::from(code as u8),
        Err(e) => {
            let ui = ui::Ui::new(false);
            ui.print_error(&format!("{:#}", e));
            ExitCode::from(2)
        }
    }
}

fn run() -> Result<i32> {
    let cli = Cli::parse();

    // Determine project root (directory containing config file)
    let config_path = &cli.config;
    let project_root = config_path
        .parent()
        .map(|p| {
            if p.as_os_str().is_empty() {
                Path::new(".")
            } else {
                p
            }
        })
        .unwrap_or(Path::new("."))
        .to_path_buf();

    let ui = ui::Ui::new(cli.verbose);

    match cli.command.unwrap_or_default() {
        Commands::Init { force } => {
            config::init_config(config_path, force)?;
            ui.print_init_success(&config_path.display().to_string());
            Ok(0)
        }

        Commands::Clean { names } => {
            cache::clean_cache(&project_root, names.clone())?;
            ui.print_cache_cleaned(&names);
            Ok(0)
        }

        Commands::Status {
            name,
            detailed,
            verify,
        } => {
            let config = config::Config::load(config_path)?;

            // Validate check name if provided
            if let Some(ref name) = name {
                if config.get(name).is_none() {
                    anyhow::bail!("Unknown check: {}", name);
                }
            }

            let cache = cache::CacheState::load(&project_root)?;
            let has_unverified =
                runner::run_status(&project_root, &config, &cache, cli.json, detailed, name)?;
            if verify && has_unverified {
                Ok(1)
            } else {
                Ok(0)
            }
        }

        Commands::Run {
            names,
            force,
            stage,
        } => {
            let config = config::Config::load(config_path)?;
            let mut cache = cache::CacheState::load(&project_root)?;

            // Validate requested check names exist
            for name in &names {
                if config.get(name).is_none() {
                    anyhow::bail!("Unknown check: {}", name);
                }
            }

            let result = runner::run_checks(
                &project_root,
                &config,
                &mut cache,
                names,
                force,
                cli.json,
                cli.verbose,
            )?;

            // Stage verify.lock if requested (from either cli.stage or run --stage) and checks passed
            if (cli.stage || stage) && result == 0 {
                let lock_path = project_root.join("verify.lock");
                if lock_path.exists() {
                    std::process::Command::new("git")
                        .args(["add", "verify.lock"])
                        .current_dir(&project_root)
                        .status()
                        .ok(); // Ignore errors (might not be in git repo)
                }
            }

            Ok(result)
        }
    }
}
