use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "vfy")]
#[command(author, version, about = "Run and cache project verification checks")]
pub struct Cli {
    /// Path to config file (default: vfy.yaml)
    #[arg(short, long, default_value = "vfy.yaml")]
    pub config: PathBuf,

    /// Output in JSON format
    #[arg(long)]
    pub json: bool,

    /// Verbose output
    #[arg(short, long, global = true)]
    pub verbose: bool,

    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Run verification checks (default command)
    Run {
        /// Specific check name(s) to run
        #[arg(value_name = "NAME")]
        names: Vec<String>,

        /// Force run even if cache is fresh
        #[arg(short, long)]
        force: bool,
    },

    /// Show status of all checks
    Status {
        /// Show detailed file-level information
        #[arg(long)]
        detailed: bool,
    },

    /// Initialize a new vfy.yaml config file
    Init {
        /// Overwrite existing config file
        #[arg(long)]
        force: bool,
    },

    /// Clear cache for specific checks or all
    Clean {
        /// Specific check name(s) to clear
        #[arg(value_name = "NAME")]
        names: Vec<String>,
    },
}

impl Default for Commands {
    fn default() -> Self {
        Commands::Run {
            names: vec![],
            force: false,
        }
    }
}
