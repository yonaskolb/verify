use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "verify")]
#[command(author, version, about = "Run and cache project verification checks")]
pub struct Cli {
    /// Path to config file (default: verify.yaml)
    #[arg(short, long, default_value = "verify.yaml", global = true)]
    pub config: PathBuf,

    /// Output in JSON format
    #[arg(long, global = true)]
    pub json: bool,

    /// Verbose output
    #[arg(short, long, global = true)]
    pub verbose: bool,

    /// Stage verify.lock after successful run (for git hooks)
    #[arg(long, global = true)]
    pub stage: bool,

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

        /// Stage verify.lock after successful run (for git hooks)
        #[arg(long)]
        stage: bool,
    },

    /// Show status of checks
    Status {
        /// Specific check name to show status for
        #[arg(value_name = "NAME")]
        name: Option<String>,

        /// Show detailed file-level information
        #[arg(long)]
        detailed: bool,

        /// Exit with code 1 if any check is unverified
        #[arg(long)]
        verify: bool,
    },

    /// Initialize a new verify.yaml config file
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
            stage: false,
        }
    }
}
