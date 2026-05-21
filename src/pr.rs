// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Main `run` subcommand: per-target fork/clone/branch/modify/push/PR.

use crate::checks;
use crate::config::{Config, Target};
use crate::github;
use crate::style::{apply_entries, apply_removals, Style};
use anyhow::{anyhow, Context, Result};
use rand::Rng;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread::sleep;
use std::time::Duration;

pub fn run(config_path: &Path, go: bool, limit: usize) -> Result<()> {
    let cfg = Config::from_path(config_path)?;

    // Per-script git config: HTTPS via gh's credential helper, fsck disabled.
    std::env::set_var(
        "GIT_CONFIG_PARAMETERS",
        "'fetch.fsckObjects=false' 'receive.fsckObjects=false' 'transfer.fsckObjects=false' \
'url.https://github.com/.insteadOf=git@github.com:' \
'credential.helper=' \
'credential.https://github.com.helper=!gh auth git-credential'",
    );

    let mut opened = 0usize;
    for target in &cfg.targets {
        match process_target(target, &cfg, go) {
            Ok(Outcome::Opened(url)) => {
                println!("    -> opened: {url}");
                opened += 1;
                if limit > 0 && opened >= limit {
                    println!("hit --limit={limit}");
                    break;
                }
                let mut rng = rand::thread_rng();
                let secs = rng.gen_range(cfg.defaults.sleep_min..=cfg.defaults.sleep_max);
                println!("    sleeping {secs}s...");
                sleep(Duration::from_secs(secs));
            }
            Ok(Outcome::Skipped(reason)) => println!("    -> skipped: {reason}"),
            Ok(Outcome::DryRun) => println!("    -> dry-run"),
            Err(e) => println!("    -> error: {e:#}"),
        }
    }
    Ok(())
}

enum Outcome {
    Opened(String),
    Skipped(String),
    DryRun,
}

fn process_target(t: &Target, cfg: &Config, go: bool) -> Result<Outcome> {
    println!(
        ">>> {}  (base={}, entries={})",
        t.repo,
        t.branch,
        t.entries.len()
    );

    // 1. Idempotency
    let n = github::count_prs_from_head(&t.repo, &cfg.defaults.user_login, &cfg.defaults.branch)?;
    if n >= 1 {
        return Ok(Outcome::Skipped("PR already exists".into()));
    }

    // 2. Rejection-history check
    if let Some(reason) = checks::rejection_history(&t.repo, &cfg.defaults.user_login, 5)? {
        return Ok(Outcome::Skipped(format!("rejection history: {reason}")));
    }

    // 3. Upstream existence check
    let pairs: Vec<(String, String)> = t
        .entries
        .iter()
        .map(|e| (e.line.clone(), e.r#ref.clone()))
        .collect();
    let (kept, dropped) = checks::filter_existing_paths(&t.repo, &t.branch, &pairs)?;
    for d in &dropped {
        println!("    ! dropping `{}` (not in upstream)", d.0);
    }
    if kept.is_empty() {
        return Ok(Outcome::Skipped(
            "nothing remains after upstream check".into(),
        ));
    }

    let last_ga = t
        .last_gitattributes_ref
        .clone()
        .unwrap_or_else(|| "N/A".into());
    let body = build_pr_body(&last_ga, &kept, &t.remove);

    // Surface stale entries (e.g., a leftover `.travis.yml` excluded but no
    // longer present upstream) in both dry-run and real runs so the user
    // sees what we're cleaning up.
    if !t.remove.is_empty() {
        println!(
            "    ! {} stale entr{} in upstream `.gitattributes` will be removed:",
            t.remove.len(),
            if t.remove.len() == 1 { "y" } else { "ies" }
        );
        for r in &t.remove {
            println!("      - {}   ({})", r.line, r.reason);
        }
    }

    if !go {
        let action = if t.create { "CREATE" } else { "APPEND" };
        println!("    file action: {action} .gitattributes");
        for k in &kept {
            println!("        + {}    [{}]", k.0, k.1);
        }
        println!("    --- PR title ---");
        println!("    {}", cfg.defaults.pr_title);
        println!("    --- PR body ---");
        for line in body.lines() {
            println!("    {line}");
        }
        return Ok(Outcome::DryRun);
    }

    // 4. Ensure fork + local clone
    let fork_name = t.repo.split_once('/').map(|x| x.1).unwrap_or(&t.repo);
    let local_dir: PathBuf = cfg.defaults.fork_dir.join(fork_name);
    if !fork_exists(&cfg.defaults.user_login, fork_name)? {
        run_cmd(&[
            "gh",
            "repo",
            "fork",
            &t.repo,
            "--clone=false",
            "--default-branch-only",
        ])?;
        sleep(Duration::from_secs(3));
    }
    if !local_dir.exists() {
        run_cmd(&[
            "git",
            "clone",
            &format!("git@github.com:{}/{fork_name}.git", cfg.defaults.user_login),
            local_dir.to_str().unwrap(),
        ])?;
    }

    // 5. Branch from upstream
    let dir_str = local_dir.to_str().unwrap();
    if Command::new("git")
        .args(["-C", dir_str, "remote", "get-url", "upstream"])
        .output()?
        .status
        .code()
        != Some(0)
    {
        run_cmd(&[
            "git",
            "-C",
            dir_str,
            "remote",
            "add",
            "upstream",
            &format!("git@github.com:{}.git", t.repo),
        ])?;
    }
    run_cmd(&["git", "-C", dir_str, "fetch", "upstream", &t.branch])?;
    run_cmd(&[
        "git",
        "-C",
        dir_str,
        "checkout",
        "-B",
        &cfg.defaults.branch,
        &format!("upstream/{}", t.branch),
    ])?;

    // 6. Modify .gitattributes
    let ga_path = local_dir.join(".gitattributes");
    let (content, style) = if t.create || !ga_path.exists() {
        (
            String::new(),
            Style {
                sorted: false,
                align_col: 0,
            },
        )
    } else {
        let c = std::fs::read_to_string(&ga_path)?;
        let s = Style::detect(&c);
        println!(
            "    style: sorted={} align_col={}",
            s.sorted as u8, s.align_col
        );
        (c, s)
    };
    // First apply removals (stale entries like a leftover `.travis.yml` after
    // GH Actions migration), then append the new entries.
    let removal_paths: Vec<&str> = t
        .remove
        .iter()
        .map(|r| r.line.split_whitespace().next().unwrap_or(""))
        .collect();
    let after_removals = apply_removals(&content, &removal_paths);
    let entry_strs: Vec<&str> = kept.iter().map(|p| p.0.as_str()).collect();
    let new_content = apply_entries(&after_removals, &entry_strs, style);
    if new_content == content && !t.create {
        return Ok(Outcome::Skipped("no diff after dedup".into()));
    }
    std::fs::write(&ga_path, &new_content)?;

    // 7. Commit + push + PR
    let trailer = checks::commit_trailer(&t.repo);
    let msg = format!("{}\n\n{}\n\n{}", cfg.defaults.pr_title, body, trailer);
    run_cmd(&["git", "-C", dir_str, "add", ".gitattributes"])?;
    run_cmd(&["git", "-C", dir_str, "commit", "-m", &msg])?;
    run_cmd(&[
        "git",
        "-C",
        dir_str,
        "push",
        "-u",
        "origin",
        &cfg.defaults.branch,
    ])?;

    let head = format!("{}:{}", cfg.defaults.user_login, cfg.defaults.branch);
    let out = Command::new("gh")
        .args([
            "pr",
            "create",
            "--repo",
            &t.repo,
            "--base",
            &t.branch,
            "--head",
            &head,
            "--title",
            &cfg.defaults.pr_title,
            "--body",
            &body,
        ])
        .output()?;
    if !out.status.success() {
        return Err(anyhow!(
            "gh pr create failed: {}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    let url = String::from_utf8_lossy(&out.stdout).trim().to_string();
    Ok(Outcome::Opened(url))
}

fn fork_exists(owner: &str, name: &str) -> Result<bool> {
    let out = Command::new("gh")
        .args(["repo", "view", &format!("{owner}/{name}")])
        .output()?;
    Ok(out.status.success())
}

fn run_cmd(argv: &[&str]) -> Result<()> {
    let status = Command::new(argv[0])
        .args(&argv[1..])
        .status()
        .with_context(|| format!("spawning {:?}", argv))?;
    if !status.success() {
        return Err(anyhow!("command failed: {:?}", argv));
    }
    Ok(())
}

pub fn build_pr_body(
    last_ga: &str,
    entries: &[&(String, String)],
    removals: &[crate::config::RemoveEntry],
) -> String {
    let mut s = String::from(
        "This avoids shipping dev/tooling files in the composer dist that aren't needed at runtime.\n\n",
    );
    s.push_str(&format!("Last `.gitattributes` update was {last_ga}.\n"));
    if !removals.is_empty() {
        s.push_str(
            "\n### Stale entries to remove\n\
             These `export-ignore` rules point to files that no longer exist upstream:\n",
        );
        for r in removals {
            s.push_str(&format!("- `{}` - {}\n", r.line, r.reason));
        }
    }
    if !entries.is_empty() {
        s.push_str(
            "\n### Entries to add\n\
             Files added or modified since the last `.gitattributes` update, still \
             shipped in the composer dist:\n",
        );
        for e in entries {
            s.push_str(&format!("- `{}` - last touched in {}\n", e.0, e.1));
        }
    }
    s.push_str("\nBackground reading: https://blog.madewithlove.be/post/gitattributes/");
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn body_lists_each_entry() {
        let entries = [
            (
                "/tests/ export-ignore".to_string(),
                "abc1234 2025-01-01".to_string(),
            ),
            (
                "/.editorconfig export-ignore".to_string(),
                "deadbee 2024-06-06".to_string(),
            ),
        ];
        let refs: Vec<&(String, String)> = entries.iter().collect();
        let body = build_pr_body("xyz9876 (2023-01-01)", &refs, &[]);
        assert!(body.contains("xyz9876 (2023-01-01)"));
        assert!(body.contains("/tests/ export-ignore"));
        assert!(body.contains("abc1234 2025-01-01"));
        assert!(body.contains(".editorconfig"));
        assert!(body.contains("deadbee 2024-06-06"));
        assert!(body.contains("Background reading:"));
        // No em-dash anywhere in the body
        assert!(!body.contains('\u{2014}'));
    }

    #[test]
    fn body_groups_removals_and_additions() {
        use crate::config::RemoveEntry;
        let entries = [(
            "/.github/ export-ignore".to_string(),
            "Added alongside the Travis cleanup".to_string(),
        )];
        let refs: Vec<&(String, String)> = entries.iter().collect();
        let removals = vec![RemoveEntry {
            line: "/.travis.yml export-ignore".to_string(),
            reason:
                "Travis CI config no longer exists upstream (project migrated to GitHub Actions)."
                    .to_string(),
        }];
        let body = build_pr_body("aaa1111 (2020-01-01)", &refs, &removals);
        assert!(body.contains("### Stale entries to remove"));
        assert!(body.contains("/.travis.yml export-ignore"));
        assert!(body.contains("migrated to GitHub Actions"));
        assert!(body.contains("### Entries to add"));
        assert!(body.contains("/.github/ export-ignore"));
        assert!(!body.contains('\u{2014}'));
    }
}
