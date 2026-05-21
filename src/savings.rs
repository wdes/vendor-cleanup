// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! `enrich-savings` subcommand: walk a registry YAML and fill in
//! `savings_bytes` (+ `savings_paths`) for each merged PR.

use crate::github;
use anyhow::{Context, Result};
use regex::Regex;
use serde_yaml::{Mapping, Value};
use std::path::Path;
use std::process::Command;

pub fn enrich(
    registry_path: &Path,
    force: bool,
    limit: usize,
    only_repo: Option<&str>,
) -> Result<()> {
    let raw = std::fs::read_to_string(registry_path)
        .with_context(|| format!("reading {}", registry_path.display()))?;
    let mut doc: Value = serde_yaml::from_str(&raw).context("parsing registry YAML")?;

    let mut done: usize = 0;
    let repos = doc
        .get_mut("repos")
        .and_then(|v| v.as_sequence_mut())
        .ok_or_else(|| anyhow::anyhow!("missing repos[] in registry"))?;

    for repo_entry in repos.iter_mut() {
        let repo = repo_entry
            .get("repo")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if let Some(filter) = only_repo {
            if repo != filter {
                continue;
            }
        }
        let prs = match repo_entry.get_mut("prs").and_then(|v| v.as_sequence_mut()) {
            Some(s) => s,
            None => continue,
        };
        for pr in prs.iter_mut() {
            let state = pr
                .get("state")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if state != "MERGED" {
                continue;
            }
            let already_set = pr
                .get("savings_bytes")
                .map(|v| !v.is_null())
                .unwrap_or(false);
            if already_set && !force {
                continue;
            }
            let num = match pr.get("pr").and_then(|v| v.as_u64()) {
                Some(n) => n,
                None => continue,
            };
            eprintln!("  [{repo}#{num}] computing...");

            let paths = diff_added_export_ignore_paths(&repo, num)?;
            if paths.is_empty() {
                set_savings(pr, 0, vec![]);
                done += 1;
                if limit > 0 && done >= limit {
                    break;
                }
                continue;
            }

            let branch = default_branch(&repo).unwrap_or_else(|| "main".into());
            let mut total: u64 = 0;
            let mut detail: Vec<(String, u64)> = Vec::new();
            for p in &paths {
                if let Some(sz) = tree_size(&repo, &branch, p)? {
                    total += sz;
                    detail.push((p.clone(), sz));
                }
            }
            set_savings(pr, total, detail);
            done += 1;
            if limit > 0 && done >= limit {
                break;
            }
        }
        if limit > 0 && done >= limit {
            break;
        }
    }

    let out = serde_yaml::to_string(&doc).context("serializing registry")?;
    std::fs::write(registry_path, out)?;
    eprintln!("enriched {done} PRs");
    Ok(())
}

fn set_savings(pr: &mut Value, bytes: u64, paths: Vec<(String, u64)>) {
    if let Some(map) = pr.as_mapping_mut() {
        map.insert(
            Value::String("savings_bytes".into()),
            Value::Number(bytes.into()),
        );
        let mut arr = Vec::new();
        for (p, b) in paths {
            let mut m = Mapping::new();
            m.insert(Value::String("path".into()), Value::String(p));
            m.insert(Value::String("bytes".into()), Value::Number(b.into()));
            arr.push(Value::Mapping(m));
        }
        map.insert(Value::String("savings_paths".into()), Value::Sequence(arr));
    }
}

/// Parse the diff of a PR and return every path that was added to
/// `.gitattributes` with `export-ignore`.
pub fn diff_added_export_ignore_paths(repo: &str, num: u64) -> Result<Vec<String>> {
    let out = Command::new("gh")
        .args(["pr", "diff", &num.to_string(), "--repo", repo])
        .output()
        .context("spawning gh pr diff")?;
    if !out.status.success() {
        return Ok(vec![]);
    }
    let body = String::from_utf8_lossy(&out.stdout);
    Ok(extract_added_paths(&body))
}

/// Pure helper that extracts the added-and-export-ignored paths from a unified
/// diff. Lines starting with `+` (excluding the `+++ b/` header) that match
/// `+/?<path>/? export-ignore` are returned.
pub fn extract_added_paths(diff: &str) -> Vec<String> {
    let re = Regex::new(r"^\+\s*/?([^\s]+?)/?\s+export-ignore\s*$").unwrap();
    let mut paths: Vec<String> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for line in diff.lines() {
        if line.starts_with("+++") || line.starts_with("---") {
            continue;
        }
        if let Some(caps) = re.captures(line) {
            let p = caps[1].to_string();
            if seen.insert(p.clone()) {
                paths.push(p);
            }
        }
    }
    paths
}

fn default_branch(repo: &str) -> Option<String> {
    let v = github::gh_api_json(&[&format!("repos/{repo}")]).ok()?;
    v.get("default_branch")
        .and_then(|s| s.as_str())
        .map(|s| s.to_string())
}

fn tree_size(repo: &str, branch: &str, path: &str) -> Result<Option<u64>> {
    let head = github::gh_api_json(&[&format!("repos/{repo}/branches/{branch}")])?;
    let sha = match head
        .get("commit")
        .and_then(|c| c.get("sha"))
        .and_then(|s| s.as_str())
    {
        Some(s) => s.to_string(),
        None => return Ok(None),
    };
    let tree = github::gh_api_json(&[&format!("repos/{repo}/git/trees/{sha}?recursive=1")])?;
    let arr = match tree.get("tree").and_then(|t| t.as_array()) {
        Some(a) => a,
        None => return Ok(None),
    };
    let norm = path.trim_start_matches('/').trim_end_matches('/');
    let mut total: u64 = 0;
    let mut hit = false;
    for entry in arr {
        if entry.get("type").and_then(|t| t.as_str()) != Some("blob") {
            continue;
        }
        let ep = entry.get("path").and_then(|s| s.as_str()).unwrap_or("");
        if ep == norm || ep.starts_with(&format!("{norm}/")) {
            total += entry.get("size").and_then(|s| s.as_u64()).unwrap_or(0);
            hit = true;
        }
    }
    Ok(if hit { Some(total) } else { None })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_simple_added_paths() {
        let diff = r#"
diff --git a/.gitattributes b/.gitattributes
--- a/.gitattributes
+++ b/.gitattributes
@@ -1,2 +1,5 @@
 *.php text eol=lf
+/tests/ export-ignore
+/.editorconfig export-ignore
+/CONTRIBUTING.md export-ignore
"#;
        let paths = extract_added_paths(diff);
        assert_eq!(paths, vec!["tests", ".editorconfig", "CONTRIBUTING.md"]);
    }

    #[test]
    fn skips_diff_headers_and_removed_lines() {
        let diff = r#"
+++ b/.gitattributes
--- a/.gitattributes
-/old export-ignore
+/new export-ignore
"#;
        let paths = extract_added_paths(diff);
        assert_eq!(paths, vec!["new"]);
    }

    #[test]
    fn dedupes_repeated_paths() {
        let diff = r#"
+/tests/ export-ignore
+/tests/ export-ignore
"#;
        let paths = extract_added_paths(diff);
        assert_eq!(paths, vec!["tests"]);
    }

    #[test]
    fn ignores_non_export_ignore_additions() {
        let diff = r#"
+# just a comment
+*.php text eol=lf
+/tests/ export-ignore
"#;
        let paths = extract_added_paths(diff);
        assert_eq!(paths, vec!["tests"]);
    }
}
