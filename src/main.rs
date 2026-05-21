// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

mod build;
mod checks;
mod config;
mod github;
mod pr;
mod savings;
mod style;

#[derive(Parser)]
#[command(version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Open .gitattributes export-ignore PRs based on a YAML config.
    Run {
        /// Path to the YAML config file.
        #[arg(long)]
        config: PathBuf,

        /// Actually open PRs (default is dry-run).
        #[arg(long)]
        go: bool,

        /// Stop after N PRs are opened.
        #[arg(long, default_value_t = 0)]
        limit: usize,
    },

    /// Build a campaign YAML from a `vendor/` directory and/or a list
    /// of `OWNER/REPO[:BRANCH]` strings.
    Build {
        /// Local `vendor/` directory to scan (composer install layout).
        #[arg(long)]
        vendor: Option<PathBuf>,

        /// Explicit list of `OWNER/REPO` repositories (combine with --vendor or use alone).
        #[arg(long = "repo", value_name = "OWNER/REPO")]
        repos: Vec<String>,

        /// Your GitHub login (will appear in branch + fork URL).
        #[arg(long)]
        user_login: String,

        /// Where the script should keep local clones (written to the
        /// emitted YAML; defaults to a placeholder you'll want to edit).
        #[arg(long)]
        fork_dir: Option<PathBuf>,

        /// Output YAML path.
        #[arg(long, short)]
        output: PathBuf,
    },

    /// Enrich a PR registry YAML with per-PR savings_bytes computed
    /// from upstream tree sizes.
    EnrichSavings {
        /// Path to the registry YAML.
        registry: PathBuf,

        /// Recompute even when savings_bytes is already set.
        #[arg(long)]
        force: bool,

        /// Enrich at most N PRs (0 = unlimited).
        #[arg(long, default_value_t = 0)]
        limit: usize,

        /// Restrict to a specific OWNER/REPO.
        #[arg(long)]
        repo: Option<String>,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Run { config, go, limit } => pr::run(&config, go, limit),
        Command::Build {
            vendor,
            repos,
            user_login,
            fork_dir,
            output,
        } => build::build(build::BuildArgs {
            vendor,
            repos,
            user_login,
            fork_dir,
            output,
        }),
        Command::EnrichSavings {
            registry,
            force,
            limit,
            repo,
        } => savings::enrich(&registry, force, limit, repo.as_deref()),
    }
}
