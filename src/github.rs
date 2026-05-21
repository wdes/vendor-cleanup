// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Thin wrappers over the `gh` CLI for all GitHub interactions.

use anyhow::{anyhow, Context, Result};
use serde_json::Value;
use std::process::Command;

/// Run `gh api <args>` and parse the response as JSON.
pub fn gh_api_json(args: &[&str]) -> Result<Value> {
    let out = Command::new("gh")
        .arg("api")
        .args(args)
        .output()
        .context("spawning gh api")?;
    if !out.status.success() {
        return Err(anyhow!(
            "gh api {:?} failed: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    let v: Value = serde_json::from_slice(&out.stdout)
        .with_context(|| format!("parsing JSON from gh api {:?}", args))?;
    Ok(v)
}

/// Run `gh api -i <path>` and return whether the HTTP status is 2xx.
pub fn gh_api_exists(path: &str) -> Result<bool> {
    let out = Command::new("gh")
        .arg("api")
        .arg("-i")
        .arg(path)
        .output()
        .context("spawning gh api -i")?;
    let first = String::from_utf8_lossy(&out.stdout)
        .lines()
        .next()
        .unwrap_or("")
        .to_string();
    Ok(first.contains("200 OK"))
}

/// Return how many PRs from `owner:branch` (head) target `repo` (any state).
pub fn count_prs_from_head(repo: &str, owner: &str, branch: &str) -> Result<usize> {
    let filter = format!(
        r#"[.[] | select(.headRefName == "{branch}" and .headRepositoryOwner.login == "{owner}")] | length"#
    );
    let out = Command::new("gh")
        .args([
            "pr",
            "list",
            "--repo",
            repo,
            "--state",
            "all",
            "--json",
            "number,headRefName,headRepositoryOwner",
            "--jq",
            &filter,
        ])
        .output()
        .context("spawning gh pr list")?;
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    Ok(s.parse().unwrap_or(0))
}

/// List closed-not-merged PRs in `repo` whose title/body mentions
/// `gitattributes` or `export-ignore`.
pub fn closed_gitattributes_prs(repo: &str) -> Result<Value> {
    let out = Command::new("gh")
        .args([
            "pr",
            "list",
            "--repo",
            repo,
            "--state",
            "closed",
            "--search",
            "gitattributes OR export-ignore",
            "--json",
            "number,author,closedAt,mergedAt,state",
            "--jq",
            r#"[.[] | select(.state == "CLOSED" and .mergedAt == null)]"#,
        ])
        .output()
        .context("spawning gh pr list closed")?;
    if !out.status.success() {
        return Ok(Value::Array(vec![]));
    }
    let v: Value = serde_json::from_slice(&out.stdout).unwrap_or(Value::Array(vec![]));
    Ok(v)
}

/// Does a PR's diff touch `.gitattributes`?
pub fn pr_touches_gitattributes(repo: &str, num: u64) -> Result<bool> {
    let out = Command::new("gh")
        .args([
            "pr",
            "view",
            &num.to_string(),
            "--repo",
            repo,
            "--json",
            "files",
            "--jq",
            r#"[.files[].path] | any(. == ".gitattributes")"#,
        ])
        .output()
        .context("spawning gh pr view")?;
    Ok(String::from_utf8_lossy(&out.stdout).trim() == "true")
}

/// Login of the actor who fired the most recent `closed` event on a PR.
pub fn pr_closer_login(repo: &str, num: u64) -> Result<Option<String>> {
    let path = format!("repos/{repo}/issues/{num}/timeline");
    let v = gh_api_json(&[&path])?;
    let arr = v.as_array().ok_or_else(|| anyhow!("timeline not array"))?;
    for ev in arr.iter().rev() {
        if ev.get("event").and_then(|x| x.as_str()) == Some("closed") {
            if let Some(login) = ev
                .get("actor")
                .and_then(|a| a.get("login"))
                .and_then(|s| s.as_str())
            {
                return Ok(Some(login.to_string()));
            }
        }
    }
    Ok(None)
}
