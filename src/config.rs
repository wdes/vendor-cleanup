// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! YAML config schema for vendor-cleanup `run` subcommand.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Defaults {
    #[serde(default = "default_fork_dir")]
    pub fork_dir: PathBuf,
    #[serde(default = "default_user_login")]
    pub user_login: String,
    #[serde(default = "default_branch")]
    pub branch: String,
    #[serde(default = "default_pr_title")]
    pub pr_title: String,
    #[serde(default = "default_sleep_min")]
    pub sleep_min: u64,
    #[serde(default = "default_sleep_max")]
    pub sleep_max: u64,
}

fn default_fork_dir() -> PathBuf {
    dirs_home().join("forks")
}
fn default_user_login() -> String {
    "you".into()
}
fn default_branch() -> String {
    "vendor-cleanup/gitattributes-export-ignore".into()
}
fn default_pr_title() -> String {
    "Update .gitattributes to exclude dev files from composer dist".into()
}
fn default_sleep_min() -> u64 {
    10
}
fn default_sleep_max() -> u64 {
    20
}

fn dirs_home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"))
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Entry {
    pub line: String,
    pub r#ref: String,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Target {
    pub repo: String,
    #[serde(default = "default_branch_name")]
    pub branch: String,
    #[serde(default)]
    pub create: bool,
    #[serde(default)]
    pub last_gitattributes_ref: Option<String>,
    pub entries: Vec<Entry>,
}

fn default_branch_name() -> String {
    "main".into()
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct SkippedRepo {
    pub repo: String,
    pub reason: String,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Config {
    #[serde(default = "default_defaults")]
    pub defaults: Defaults,
    pub targets: Vec<Target>,
    #[serde(default)]
    pub skipped: Vec<SkippedRepo>,
}

fn default_defaults() -> Defaults {
    serde_yaml::from_str("{}").unwrap()
}

impl Config {
    pub fn from_path(path: &Path) -> Result<Self> {
        let s =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let cfg: Config = serde_yaml::from_str(&s)
            .with_context(|| format!("parsing YAML at {}", path.display()))?;
        Ok(cfg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_tmp(content: &str) -> tempfile::NamedTempFile {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(content.as_bytes()).unwrap();
        f
    }

    #[test]
    fn parses_minimal_config() {
        let yaml = r#"
targets:
  - repo: foo/bar
    branch: main
    entries:
      - { line: "/tests/ export-ignore", ref: "abc1234 2025-01-01" }
"#;
        let f = write_tmp(yaml);
        let cfg = Config::from_path(f.path()).unwrap();
        assert_eq!(cfg.targets.len(), 1);
        assert_eq!(cfg.targets[0].repo, "foo/bar");
        assert_eq!(cfg.targets[0].entries[0].line, "/tests/ export-ignore");
        assert_eq!(cfg.targets[0].entries[0].r#ref, "abc1234 2025-01-01");
        assert!(!cfg.targets[0].create);
    }

    #[test]
    fn parses_full_config() {
        let yaml = r#"
defaults:
  fork_dir: /tmp/forks
  user_login: alice
  branch: alice/x
  pr_title: Custom title
  sleep_min: 5
  sleep_max: 7
targets:
  - repo: foo/a
    branch: develop
    create: true
    last_gitattributes_ref: "abc1234 (2024-01-01)"
    entries:
      - line: "/docs/ export-ignore"
        ref: "deadbee 2025-02-02"
skipped:
  - repo: foo/skip
    reason: maintainer said no
"#;
        let f = write_tmp(yaml);
        let cfg = Config::from_path(f.path()).unwrap();
        assert_eq!(cfg.defaults.user_login, "alice");
        assert_eq!(cfg.defaults.sleep_min, 5);
        assert!(cfg.targets[0].create);
        assert_eq!(cfg.targets[0].branch, "develop");
        assert_eq!(cfg.skipped.len(), 1);
    }

    #[test]
    fn defaults_when_missing() {
        let yaml = r#"
targets:
  - repo: foo/x
    entries: []
"#;
        let f = write_tmp(yaml);
        let cfg = Config::from_path(f.path()).unwrap();
        assert_eq!(cfg.defaults.user_login, "you");
        assert_eq!(cfg.defaults.sleep_min, 10);
        assert_eq!(cfg.defaults.sleep_max, 20);
        assert_eq!(cfg.targets[0].branch, "main");
    }
}
