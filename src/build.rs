// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! `build` subcommand: generate a campaign YAML from either a `vendor/`
//! directory or a list of `OWNER/REPO[:BRANCH]` strings.
//!
//! For each target the builder:
//!   1. Resolves the repo and default branch.
//!   2. Lists candidate dev-files (either from the local vendor/ tree
//!      when scanning a folder, or a built-in default list otherwise).
//!   3. Drops paths already excluded by upstream's `.gitattributes`.
//!   4. Drops paths that don't exist in upstream HEAD.
//!   5. For each surviving path, fetches the last commit ref + date.
//!   6. Fetches the last commit ref + date that touched `.gitattributes`.
//!
//! All the steps that touch the network are isolated behind `github`;
//! the pure helpers (parsing composer.json source, parsing
//! .gitattributes for excluded paths, scanning a vendor dir) are unit
//! tested independently.

use crate::config::{Config, Defaults, Entry, RemoveEntry, Target};
use crate::github;
use anyhow::{anyhow, Context, Result};
use regex::Regex;
use serde_json::Value;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// Default candidates considered as dev/tooling files that don't belong
/// in a composer dist. Used when scanning by repo name only (no
/// vendor/ tree to inspect). Order matters: kept stable for review.
pub const DEFAULT_DEV_CANDIDATES: &[&str] = &[
    // folders
    "tests",
    "Tests",
    "test",
    "examples",
    "docs",
    "doc",
    "benchmark",
    "benchmarks",
    ".github",
    ".phpdoc",
    "guides",
    // files
    "phpunit.xml",
    "phpunit.xml.dist",
    "psalm.xml",
    "psalm.xml.dist",
    "phpcs.xml",
    "phpcs.xml.dist",
    "phpstan.neon",
    "phpstan.neon.dist",
    "phpstan-baseline.neon",
    ".php-cs-fixer.php",
    ".php-cs-fixer.dist.php",
    ".php_cs",
    ".php_cs.dist",
    "infection.json.dist",
    ".editorconfig",
    ".scrutinizer.yml",
    ".travis.yml",
    "appveyor.yml",
    ".gitlab-ci.yml",
    "codecov.yml",
    "Makefile",
    "CONTRIBUTING.md",
    "CLAUDE.md",
    "AGENTS.md",
    ".phpstorm.meta.php",
    "splitsh.json",
];

/// Resolve the GitHub OWNER/REPO from a parsed `composer.json` value.
/// Tries `support.source`, then `homepage`. Returns None when the URL
/// isn't a github.com URL.
pub fn parse_composer_source(composer: &Value) -> Option<String> {
    let candidates = [
        composer
            .get("support")
            .and_then(|s| s.get("source"))
            .and_then(|s| s.as_str()),
        composer.get("homepage").and_then(|s| s.as_str()),
        composer
            .get("support")
            .and_then(|s| s.get("issues"))
            .and_then(|s| s.as_str()),
    ];
    for c in candidates.into_iter().flatten() {
        if let Some(slug) = extract_github_slug(c) {
            return Some(slug);
        }
    }
    None
}

/// Extract `OWNER/REPO` from a github URL string.
pub fn extract_github_slug(url: &str) -> Option<String> {
    let re = Regex::new(r"github\.com[:/]([A-Za-z0-9_.-]+)/([A-Za-z0-9_.-]+?)(\.git|/|$)").ok()?;
    let caps = re.captures(url)?;
    let owner = &caps[1];
    let repo = &caps[2];
    // Strip trailing .git just in case the regex left it
    let repo = repo.trim_end_matches(".git");
    Some(format!("{owner}/{repo}"))
}

/// Paths that historically signal a Travis CI configuration. When one
/// of these is still in `.gitattributes` but the file no longer
/// exists upstream, the repo has migrated to GitHub Actions but
/// forgot to clean up the stale `export-ignore` entry. We then also
/// propose adding `/.github` so the new CI dir is excluded.
pub const TRAVIS_FILES: &[&str] = &[".travis.yml", ".travis-ci.yml"];

fn strip_slashes_str(s: &str) -> String {
    s.trim_start_matches('/').trim_end_matches('/').to_string()
}

/// Parse an upstream `.gitattributes` content and return the set of
/// paths (normalized: no leading `/`, no trailing `/`) that are already
/// `export-ignore`d.
pub fn parsed_excluded_paths_from_gitattributes(text: &str) -> HashSet<String> {
    let re = Regex::new(r"^\s*/?([^\s]+?)/?\s+export-ignore\s*$").unwrap();
    let mut out = HashSet::new();
    for line in text.lines() {
        if let Some(caps) = re.captures(line) {
            out.insert(caps[1].to_string());
        }
    }
    out
}

/// Scan a local vendor/ tree and return for each <vendor>/<pkg>:
/// (`OWNER/REPO`, list of dev-file paths present in that package).
pub fn scan_vendor_dir(vendor: &Path) -> Result<Vec<(String, Vec<String>)>> {
    let mut out: Vec<(String, Vec<String>)> = Vec::new();
    for vendor_entry in
        std::fs::read_dir(vendor).with_context(|| format!("reading {}", vendor.display()))?
    {
        let vendor_entry = vendor_entry?;
        if !vendor_entry.file_type()?.is_dir() {
            continue;
        }
        // Skip composer internal dirs like vendor/composer/, vendor/bin/, etc.
        let vendor_name = vendor_entry.file_name();
        let vendor_name = vendor_name.to_string_lossy();
        if vendor_name == "bin" || vendor_name == "composer" || vendor_name.starts_with('.') {
            continue;
        }
        for pkg_entry in std::fs::read_dir(vendor_entry.path())? {
            let pkg_entry = pkg_entry?;
            if !pkg_entry.file_type()?.is_dir() {
                continue;
            }
            let composer = pkg_entry.path().join("composer.json");
            if !composer.exists() {
                continue;
            }
            let composer_txt = std::fs::read_to_string(&composer)
                .with_context(|| format!("reading {}", composer.display()))?;
            let composer_json: Value = match serde_json::from_str(&composer_txt) {
                Ok(v) => v,
                Err(_) => continue,
            };
            let Some(slug) = parse_composer_source(&composer_json) else {
                continue;
            };
            let present = list_dev_files_in_dir(&pkg_entry.path())?;
            if !present.is_empty() {
                out.push((slug, present));
            }
        }
    }
    Ok(out)
}

/// Return which of `DEFAULT_DEV_CANDIDATES` exist as files/dirs in
/// the given package directory.
pub fn list_dev_files_in_dir(pkg_dir: &Path) -> Result<Vec<String>> {
    let mut found = Vec::new();
    for cand in DEFAULT_DEV_CANDIDATES {
        if pkg_dir.join(cand).exists() {
            found.push((*cand).to_string());
        }
    }
    Ok(found)
}

#[derive(Debug, Clone)]
pub struct BuildArgs {
    pub vendor: Option<PathBuf>,
    pub repos: Vec<String>,
    pub user_login: String,
    pub fork_dir: Option<PathBuf>,
    pub output: PathBuf,
}

pub fn build(args: BuildArgs) -> Result<()> {
    if args.vendor.is_none() && args.repos.is_empty() {
        return Err(anyhow!("provide --vendor or --repos"));
    }

    // Step 1: discover (slug, candidate_files) pairs.
    let mut pairs: Vec<(String, Vec<String>)> = Vec::new();
    if let Some(v) = &args.vendor {
        eprintln!("Scanning {}...", v.display());
        pairs.extend(scan_vendor_dir(v)?);
    }
    for raw in &args.repos {
        // OWNER/REPO[:BRANCH]
        let (slug, _branch_hint) = match raw.split_once(':') {
            Some((s, b)) => (s.to_string(), Some(b.to_string())),
            None => (raw.clone(), None),
        };
        let cands: Vec<String> = DEFAULT_DEV_CANDIDATES
            .iter()
            .map(|s| (*s).to_string())
            .collect();
        pairs.push((slug, cands));
    }
    // dedupe by slug (vendor scan may also intersect explicit --repos)
    pairs.sort_by(|a, b| a.0.cmp(&b.0));
    pairs.dedup_by(|a, b| a.0 == b.0);

    eprintln!("Building targets for {} repo(s)...", pairs.len());

    // Step 2: enrich each pair into a Target.
    let mut targets: Vec<Target> = Vec::new();
    for (slug, candidates) in pairs {
        match build_one_target(&slug, &candidates) {
            Ok(Some(t)) => {
                let n = t.entries.len();
                eprintln!("  + {slug} -> {n} entr{}", if n == 1 { "y" } else { "ies" });
                targets.push(t);
            }
            Ok(None) => eprintln!("  - {slug}: no remaining candidates"),
            Err(e) => eprintln!("  ! {slug}: skipped ({e:#})"),
        }
    }

    // Step 3: serialize to YAML.
    let cfg = Config {
        defaults: Defaults {
            fork_dir: args
                .fork_dir
                .unwrap_or_else(|| PathBuf::from("/path/to/your/forks")),
            user_login: args.user_login.clone(),
            branch: format!("{}/gitattributes-export-ignore", args.user_login),
            pr_title: "Update .gitattributes to exclude dev files from composer dist".into(),
            sleep_min: 10,
            sleep_max: 20,
        },
        targets,
        skipped: Vec::new(),
    };
    let yaml = serde_yaml::to_string(&cfg)?;
    std::fs::write(&args.output, yaml)
        .with_context(|| format!("writing {}", args.output.display()))?;
    eprintln!("Wrote {}", args.output.display());
    Ok(())
}

/// Detect stale `export-ignore` entries in upstream `.gitattributes`:
/// files/dirs declared as excluded that no longer exist in upstream
/// HEAD. Returns the list of paths to remove with a short reason.
///
/// `excluded_paths` is the normalized set from
/// `parsed_excluded_paths_from_gitattributes`.
/// `path_exists` is a callback that returns true when the path still
/// exists at upstream HEAD (we inject this so the function is unit-
/// testable without hitting the network).
pub fn detect_stale_entries<F>(
    excluded_paths: &HashSet<String>,
    mut path_exists: F,
) -> Vec<RemoveEntry>
where
    F: FnMut(&str) -> bool,
{
    let mut out = Vec::new();
    let mut sorted: Vec<&String> = excluded_paths.iter().collect();
    sorted.sort();
    for norm in sorted {
        if path_exists(norm) {
            continue;
        }
        let line = format!("/{norm} export-ignore");
        let reason = if TRAVIS_FILES.contains(&norm.as_str()) {
            "Travis CI config no longer exists upstream (project migrated to GitHub Actions)."
                .to_string()
        } else {
            format!("`{norm}` no longer exists in upstream HEAD.")
        };
        out.push(RemoveEntry { line, reason });
    }
    out
}

/// When a removal list contains a Travis file, and `.github` exists
/// upstream and isn't already excluded, return Some(github_addition).
/// Used by the builder to surface the Travis → GitHub Actions
/// migration in a single PR.
pub fn github_addition_for_travis_removal<F>(
    removals: &[RemoveEntry],
    excluded_paths: &HashSet<String>,
    mut path_exists: F,
) -> Option<(&'static str, &'static str)>
where
    F: FnMut(&str) -> bool,
{
    let has_travis_removal = removals.iter().any(|r| {
        let norm = strip_slashes_str(r.line.split_whitespace().next().unwrap_or(""));
        TRAVIS_FILES.contains(&norm.as_str())
    });
    if !has_travis_removal {
        return None;
    }
    if excluded_paths.contains(".github") {
        return None;
    }
    if !path_exists(".github") {
        return None;
    }
    Some((
        ".github/",
        "Added alongside the Travis cleanup since CI lives under .github/workflows/ now.",
    ))
}

/// Build a single Target for a repo, taking a list of candidate paths
/// (from a vendor scan or the default list). Returns Ok(None) when no
/// entries survive the filter.
fn build_one_target(slug: &str, candidates: &[String]) -> Result<Option<Target>> {
    let info = github::gh_api_json(&[&format!("repos/{slug}")])?;
    let branch = info
        .get("default_branch")
        .and_then(|s| s.as_str())
        .ok_or_else(|| anyhow!("no default_branch for {slug}"))?
        .to_string();

    // Fetch existing upstream .gitattributes (if any) to know what's already excluded.
    let (last_ga_ref, already_excluded, create) = fetch_gitattributes_state(slug, &branch)?;

    // For each candidate, drop if already excluded, then check upstream existence,
    // then fetch the last commit ref that touched it.
    let mut entries: Vec<Entry> = Vec::new();
    for cand in candidates {
        let norm = cand.trim_start_matches('/').trim_end_matches('/');
        if already_excluded.contains(norm) {
            continue;
        }
        if !upstream_path_exists(slug, &branch, norm)? {
            continue;
        }
        let r#ref = last_touch_ref(slug, &branch, norm)?.unwrap_or_else(|| "unknown".into());
        // Use the trailing-slash style by checking dir/file. Files: no trailing slash.
        // Directories: add trailing slash for readability.
        let is_dir = upstream_is_dir(slug, &branch, norm).unwrap_or(false);
        let line = if is_dir {
            format!("/{norm}/ export-ignore")
        } else {
            format!("/{norm} export-ignore")
        };
        entries.push(Entry { line, r#ref });
    }

    // Stale-entry detection: look at what upstream excludes and confirm
    // each path still exists. If not, mark for removal. Travis -> .github
    // migration is the most common case we want to fix.
    let mut removals: Vec<RemoveEntry> = Vec::new();
    if !create {
        let stale = detect_stale_entries(&already_excluded, |p| {
            upstream_path_exists(slug, &branch, p).unwrap_or(true)
        });
        if let Some((gh_path, reason)) =
            github_addition_for_travis_removal(&stale, &already_excluded, |p| {
                upstream_path_exists(slug, &branch, p).unwrap_or(false)
            })
        {
            entries.push(Entry {
                line: format!("/{gh_path} export-ignore"),
                r#ref: reason.to_string(),
            });
        }
        removals = stale;
    }

    if create {
        // When .gitattributes does not exist upstream, also add housekeeping entries.
        entries.push(Entry {
            line: "/.gitattributes export-ignore".into(),
            r#ref: "new".into(),
        });
        entries.push(Entry {
            line: "/.gitignore export-ignore".into(),
            r#ref: "new".into(),
        });
    }

    if entries.is_empty() && removals.is_empty() {
        return Ok(None);
    }

    Ok(Some(Target {
        repo: slug.to_string(),
        branch,
        create,
        last_gitattributes_ref: last_ga_ref,
        entries,
        remove: removals,
    }))
}

/// Return `(last_gitattributes_ref, already_excluded_paths, create_if_missing)`.
fn fetch_gitattributes_state(
    slug: &str,
    branch: &str,
) -> Result<(Option<String>, HashSet<String>, bool)> {
    // Fetch the file contents (base64) via the contents API.
    let path = format!("repos/{slug}/contents/.gitattributes?ref={branch}");
    let resp = github::gh_api_json(&[&path]);
    let mut excluded: HashSet<String> = HashSet::new();
    let create = match resp {
        Err(_) => true,
        Ok(v) => {
            if let Some(b64) = v.get("content").and_then(|s| s.as_str()) {
                let cleaned: String = b64.chars().filter(|c| !c.is_whitespace()).collect();
                if let Ok(bytes) = base64_decode(&cleaned) {
                    if let Ok(text) = String::from_utf8(bytes) {
                        excluded = parsed_excluded_paths_from_gitattributes(&text);
                    }
                }
                false
            } else {
                true
            }
        }
    };

    // Last commit touching .gitattributes.
    let mut last_ga: Option<String> = None;
    if !create {
        let commits_path = format!("repos/{slug}/commits?path=.gitattributes&per_page=1");
        if let Ok(arr) = github::gh_api_json(&[&commits_path]) {
            last_ga = commit_summary_first(&arr);
        }
    }
    Ok((last_ga, excluded, create))
}

fn last_touch_ref(slug: &str, branch: &str, path: &str) -> Result<Option<String>> {
    let path_q = format!("repos/{slug}/commits?path={path}&sha={branch}&per_page=1");
    let v = github::gh_api_json(&[&path_q])?;
    Ok(commit_summary_first(&v))
}

fn upstream_path_exists(slug: &str, branch: &str, path: &str) -> Result<bool> {
    github::gh_api_exists(&format!("repos/{slug}/contents/{path}?ref={branch}"))
}

fn upstream_is_dir(slug: &str, branch: &str, path: &str) -> Result<bool> {
    let path_q = format!("repos/{slug}/contents/{path}?ref={branch}");
    let v = github::gh_api_json(&[&path_q])?;
    // For dirs, contents API returns an array; for files, an object with "type":"file".
    Ok(v.is_array())
}

/// Build a short "sha1234 YYYY-MM-DD (#NNN)" string from the first
/// entry in a GitHub /commits response. Returns None when missing.
pub fn commit_summary_first(v: &Value) -> Option<String> {
    let first = v.as_array()?.first()?;
    let sha = first.get("sha").and_then(|s| s.as_str())?;
    let date = first
        .get("commit")
        .and_then(|c| c.get("committer"))
        .and_then(|c| c.get("date"))
        .and_then(|s| s.as_str())
        .map(|s| s.chars().take(10).collect::<String>())?;
    let msg = first
        .get("commit")
        .and_then(|c| c.get("message"))
        .and_then(|s| s.as_str())
        .unwrap_or("");
    // Try to extract a "(#NNN)" PR number from the message subject line.
    let subject = msg.lines().next().unwrap_or("");
    let pr_re = Regex::new(r"\(#(\d+)\)").unwrap();
    let pr_part = pr_re
        .captures(subject)
        .map(|c| format!(" (#{})", &c[1]))
        .unwrap_or_default();
    Some(format!("{} {date}{pr_part}", &sha[..7.min(sha.len())]))
}

/// Minimal base64 decoder (avoid pulling a crate; GitHub's content
/// responses are standard base64 with `\n` newlines we strip beforehand).
fn base64_decode(s: &str) -> Result<Vec<u8>> {
    const TABLE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let s = s.trim_end_matches('=');
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let mut buf: u32 = 0;
    let mut bits: u32 = 0;
    for ch in s.bytes() {
        let v = match TABLE.iter().position(|&b| b == ch) {
            Some(p) => p as u32,
            None => {
                if ch.is_ascii_whitespace() {
                    continue;
                }
                return Err(anyhow!("invalid base64 char: {ch:?}"));
            }
        };
        buf = (buf << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::Write;

    #[test]
    fn extracts_github_slug_from_https() {
        assert_eq!(
            extract_github_slug("https://github.com/foo/bar"),
            Some("foo/bar".into())
        );
        assert_eq!(
            extract_github_slug("https://github.com/foo/bar.git"),
            Some("foo/bar".into())
        );
        assert_eq!(
            extract_github_slug("https://github.com/foo/bar/issues"),
            Some("foo/bar".into())
        );
    }

    #[test]
    fn extracts_github_slug_from_ssh() {
        assert_eq!(
            extract_github_slug("git@github.com:foo/bar.git"),
            Some("foo/bar".into())
        );
    }

    #[test]
    fn no_slug_when_not_github() {
        assert_eq!(extract_github_slug("https://gitlab.com/foo/bar"), None);
        assert_eq!(extract_github_slug("https://example.com/foo"), None);
    }

    #[test]
    fn composer_source_prefers_support_source() {
        let c = json!({
            "support": {"source": "https://github.com/foo/bar"},
            "homepage": "https://github.com/baz/qux"
        });
        assert_eq!(parse_composer_source(&c), Some("foo/bar".into()));
    }

    #[test]
    fn composer_source_falls_back_to_homepage() {
        let c = json!({"homepage": "https://github.com/baz/qux"});
        assert_eq!(parse_composer_source(&c), Some("baz/qux".into()));
    }

    #[test]
    fn composer_source_falls_back_to_support_issues() {
        let c = json!({"support": {"issues": "https://github.com/baz/qux/issues"}});
        assert_eq!(parse_composer_source(&c), Some("baz/qux".into()));
    }

    #[test]
    fn composer_source_none_when_no_github_url() {
        let c = json!({"homepage": "https://example.com"});
        assert_eq!(parse_composer_source(&c), None);
    }

    #[test]
    fn parses_excluded_paths_from_gitattributes() {
        let text = "\
*.php text eol=lf
/tests export-ignore
/.github/ export-ignore
.gitattributes export-ignore
/phpunit.xml.dist export-ignore
# comment
";
        let set = parsed_excluded_paths_from_gitattributes(text);
        assert!(set.contains("tests"));
        assert!(set.contains(".github"));
        assert!(set.contains(".gitattributes"));
        assert!(set.contains("phpunit.xml.dist"));
        assert!(!set.contains("comment"));
    }

    #[test]
    fn scan_vendor_dir_finds_packages() {
        let tmp = tempfile::tempdir().unwrap();
        let v = tmp.path().to_path_buf();
        // vendor/foo/bar with composer.json + tests/
        let pkg = v.join("foo/bar");
        std::fs::create_dir_all(&pkg).unwrap();
        let composer = serde_json::json!({
            "name": "foo/bar",
            "support": {"source": "https://github.com/foo/bar"}
        });
        let mut f = std::fs::File::create(pkg.join("composer.json")).unwrap();
        f.write_all(composer.to_string().as_bytes()).unwrap();
        std::fs::create_dir_all(pkg.join("tests")).unwrap();
        std::fs::create_dir_all(pkg.join(".github")).unwrap();
        std::fs::File::create(pkg.join("phpunit.xml.dist")).unwrap();

        // vendor/composer should be ignored
        std::fs::create_dir_all(v.join("composer/foo")).unwrap();

        let out = scan_vendor_dir(&v).unwrap();
        assert_eq!(out.len(), 1, "exactly one package found");
        assert_eq!(out[0].0, "foo/bar");
        let files: HashSet<&str> = out[0].1.iter().map(|s| s.as_str()).collect();
        assert!(files.contains("tests"));
        assert!(files.contains(".github"));
        assert!(files.contains("phpunit.xml.dist"));
    }

    #[test]
    fn base64_decodes_standard() {
        let bytes = base64_decode("aGVsbG8=").unwrap();
        assert_eq!(bytes, b"hello");
        let bytes = base64_decode("Zm9vYmFy").unwrap();
        assert_eq!(bytes, b"foobar");
        // With newlines in the middle (as GitHub returns)
        let bytes = base64_decode("aGVs\nbG8=").unwrap();
        assert_eq!(bytes, b"hello");
    }

    #[test]
    fn commit_summary_extracts_pr_number() {
        let v = json!([{
            "sha": "abcdef1234567",
            "commit": {
                "committer": {"date": "2024-01-15T10:00:00Z"},
                "message": "Fix the thing (#1234)\n\nbody"
            }
        }]);
        assert_eq!(
            commit_summary_first(&v),
            Some("abcdef1 2024-01-15 (#1234)".into())
        );
    }

    #[test]
    fn commit_summary_without_pr() {
        let v = json!([{
            "sha": "abc1234",
            "commit": {
                "committer": {"date": "2024-01-15T10:00:00Z"},
                "message": "Initial commit"
            }
        }]);
        assert_eq!(commit_summary_first(&v), Some("abc1234 2024-01-15".into()));
    }

    // ---- stale-entry detection / Travis -> GitHub Actions migration ----

    #[test]
    fn detect_stale_entries_flags_missing_paths() {
        let mut excluded = HashSet::new();
        excluded.insert(".travis.yml".to_string());
        excluded.insert("tests".to_string());
        excluded.insert("phpunit.xml.dist".to_string());

        // tests/ exists, phpunit.xml.dist exists, .travis.yml is gone.
        let stale = detect_stale_entries(&excluded, |p| p != ".travis.yml");
        assert_eq!(stale.len(), 1);
        assert_eq!(stale[0].line, "/.travis.yml export-ignore");
        assert!(
            stale[0].reason.contains("GitHub Actions"),
            "Travis removal reason should call out the migration, got: {}",
            stale[0].reason
        );
    }

    #[test]
    fn detect_stale_entries_handles_travis_ci_yml_variant() {
        let mut excluded = HashSet::new();
        excluded.insert(".travis-ci.yml".to_string());
        let stale = detect_stale_entries(&excluded, |_| false);
        assert_eq!(stale.len(), 1);
        assert_eq!(stale[0].line, "/.travis-ci.yml export-ignore");
        assert!(stale[0].reason.contains("GitHub Actions"));
    }

    #[test]
    fn detect_stale_entries_uses_generic_reason_for_non_travis() {
        let mut excluded = HashSet::new();
        excluded.insert("some-old-folder".to_string());
        let stale = detect_stale_entries(&excluded, |_| false);
        assert_eq!(stale.len(), 1);
        assert_eq!(stale[0].line, "/some-old-folder export-ignore");
        assert!(stale[0].reason.contains("no longer exists"));
        assert!(!stale[0].reason.contains("GitHub Actions"));
    }

    #[test]
    fn detect_stale_entries_returns_empty_when_all_exist() {
        let mut excluded = HashSet::new();
        excluded.insert("tests".to_string());
        excluded.insert(".travis.yml".to_string());
        let stale = detect_stale_entries(&excluded, |_| true);
        assert!(stale.is_empty());
    }

    #[test]
    fn travis_removal_triggers_github_when_github_exists() {
        let mut excluded = HashSet::new();
        excluded.insert(".travis.yml".to_string());
        let stale = detect_stale_entries(&excluded, |_| false);
        let res = github_addition_for_travis_removal(&stale, &excluded, |p| p == ".github");
        assert!(res.is_some());
        let (path, reason) = res.unwrap();
        assert_eq!(path, ".github/");
        assert!(reason.contains("Travis"));
    }

    #[test]
    fn travis_removal_does_not_trigger_github_when_already_excluded() {
        let mut excluded = HashSet::new();
        excluded.insert(".travis.yml".to_string());
        excluded.insert(".github".to_string());
        let stale = detect_stale_entries(&excluded, |p| p == ".github");
        // .github is excluded → not stale; only .travis.yml is stale.
        // But .github IS already in excluded set → don't propose adding again.
        let res = github_addition_for_travis_removal(&stale, &excluded, |_| true);
        assert!(res.is_none());
    }

    #[test]
    fn travis_removal_does_not_trigger_github_when_github_missing_upstream() {
        let mut excluded = HashSet::new();
        excluded.insert(".travis.yml".to_string());
        let stale = detect_stale_entries(&excluded, |_| false);
        let res = github_addition_for_travis_removal(&stale, &excluded, |p| p != ".github");
        assert!(res.is_none());
    }

    #[test]
    fn no_github_addition_without_travis_removal() {
        // Some unrelated stale entry, no travis involvement -> no github addition.
        let mut excluded = HashSet::new();
        excluded.insert("dead-folder".to_string());
        let stale = detect_stale_entries(&excluded, |_| false);
        let res = github_addition_for_travis_removal(&stale, &excluded, |_| true);
        assert!(res.is_none());
    }

    #[test]
    fn travis_to_github_e2e() {
        use crate::style::{apply_entries, apply_removals, Style};

        let before = "\
*.php text eol=lf
/.travis.yml export-ignore
/phpunit.xml.dist export-ignore
/tests/ export-ignore
";
        let excluded = parsed_excluded_paths_from_gitattributes(before);
        assert!(excluded.contains(".travis.yml"));

        let upstream_has = |p: &str| matches!(p, "tests" | "phpunit.xml.dist" | ".github");
        let stale = detect_stale_entries(&excluded, upstream_has);
        assert_eq!(stale.len(), 1);
        assert_eq!(stale[0].line, "/.travis.yml export-ignore");

        let gh = github_addition_for_travis_removal(&stale, &excluded, upstream_has);
        let (gh_path, _) = gh.expect("should propose adding /.github/");
        assert_eq!(gh_path, ".github/");

        let after_remove = apply_removals(before, &["/.travis.yml"]);
        assert!(!after_remove.contains(".travis.yml"));

        let new_line = format!("/{gh_path} export-ignore");
        let style = Style::detect(&after_remove);
        let final_content = apply_entries(&after_remove, &[&new_line], style);

        assert!(!final_content.contains(".travis.yml"));
        assert!(final_content.contains("/.github/ export-ignore"));
        assert!(final_content.contains("/phpunit.xml.dist export-ignore"));
        assert!(final_content.contains("/tests/ export-ignore"));
        assert!(final_content.contains("*.php text eol=lf"));
    }
}
