// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

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
        Command::EnrichSavings {
            registry,
            force,
            limit,
            repo,
        } => savings::enrich(&registry, force, limit, repo.as_deref()),
    }
}
