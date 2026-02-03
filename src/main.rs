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
        .map(|p| if p.as_os_str().is_empty() { Path::new(".") } else { p })
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

        Commands::Status { detailed } => {
            let config = config::Config::load(config_path)?;
            let cache = cache::CacheState::load(&project_root)?;
            runner::run_status(&project_root, &config, &cache, cli.json, detailed)?;
            Ok(0)
        }

        Commands::Run { names, all, force } => {
            let config = config::Config::load(config_path)?;
            let mut cache = cache::CacheState::load(&project_root)?;

            // Validate requested check names exist
            for name in &names {
                if config.get(name).is_none() {
                    anyhow::bail!("Unknown check: {}", name);
                }
            }

            runner::run_checks(
                &project_root,
                &config,
                &mut cache,
                names,
                all,
                force,
                cli.json,
                cli.verbose,
            )
        }
    }
}
