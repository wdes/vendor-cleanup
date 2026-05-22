// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Pre-flight checks that gate every PR open.

use crate::github;
use anyhow::Result;
use chrono::Datelike;

/// Repos where the maintainer has signalled they don't welcome our
/// `.gitattributes` cleanup contributions. Skip both scanning and PR
/// opening for these.
const DENYLISTED_REPOS: &[(&str, &str)] = &[(
    "mockery/mockery",
    "maintainer pushed back on our prior cleanup changes",
)];

/// Returns Some(reason) when `repo` is on the static denylist. Match is
/// case-insensitive against `OWNER/REPO`.
pub fn denylisted_repo(repo: &str) -> Option<&'static str> {
    let key = repo.to_ascii_lowercase();
    DENYLISTED_REPOS
        .iter()
        .find(|(slug, _)| slug.eq_ignore_ascii_case(&key))
        .map(|(_, reason)| *reason)
}

/// Returns Some(reason) when a non-author closed a similar `.gitattributes`
/// PR in `repo` within the last `max_age_years`. Returns None otherwise.
pub fn rejection_history(
    repo: &str,
    self_login: &str,
    max_age_years: i32,
) -> Result<Option<String>> {
    let prs = github::closed_gitattributes_prs(repo)?;
    let now_year = chrono::Utc::now().year();
    let arr = match prs.as_array() {
        Some(a) => a,
        None => return Ok(None),
    };
    for pr in arr {
        let Some(num) = pr.get("number").and_then(|n| n.as_u64()) else {
            continue;
        };
        let author = pr
            .get("author")
            .and_then(|a| a.get("login"))
            .and_then(|s| s.as_str())
            .unwrap_or("");
        let closed_at = pr.get("closedAt").and_then(|s| s.as_str()).unwrap_or("");
        if closed_at.len() < 4 {
            continue;
        }
        let closed_year: i32 = closed_at[..4].parse().unwrap_or(0);
        if (now_year - closed_year) > max_age_years {
            continue;
        }
        if !github::pr_touches_gitattributes(repo, num)? {
            continue;
        }
        let Some(closer) = github::pr_closer_login(repo, num)? else {
            continue;
        };
        if closer == author || closer == self_login {
            continue;
        }
        let date = &closed_at[..10.min(closed_at.len())];
        return Ok(Some(format!("#{num} closed by {closer} on {date}")));
    }
    Ok(None)
}

/// An entry pair: (line, ref). `line` is the literal text we'd add to
/// `.gitattributes` (path + " export-ignore"); `ref` is the commit/PR
/// citation we put in the PR body, or the literal "new" for entries we
/// create ourselves (.gitattributes, .gitignore).
pub type EntryPair = (String, String);

/// `(kept, dropped)` split by `filter_existing_paths`.
pub type EntrySplit<'a> = (Vec<&'a EntryPair>, Vec<&'a EntryPair>);

/// Returns the subset of `paths` that still exist in `repo` at `branch`.
/// Paths flagged with the literal ref `"new"` (housekeeping additions we
/// create ourselves) are kept without an upstream check.
pub fn filter_existing_paths<'a>(
    repo: &str,
    branch: &str,
    paths_with_ref: &'a [EntryPair],
) -> Result<EntrySplit<'a>> {
    let mut keep: Vec<&(String, String)> = Vec::new();
    let mut drop: Vec<&(String, String)> = Vec::new();
    for entry in paths_with_ref {
        if entry.1 == "new" {
            keep.push(entry);
            continue;
        }
        let stripped = entry
            .0
            .trim_start_matches('/')
            .trim_end_matches('/')
            .to_string();
        let path = format!("repos/{repo}/contents/{stripped}?ref={branch}");
        if github::gh_api_exists(&path)? {
            keep.push(entry);
        } else {
            drop.push(entry);
        }
    }
    Ok((keep, drop))
}

/// Pick the right commit trailer for the given repo: CLA-protected orgs get a
/// plain-text attribution so the Anthropic email doesn't trip the bot.
pub fn commit_trailer(repo: &str) -> &'static str {
    let org = repo.split('/').next().unwrap_or("").to_ascii_lowercase();
    const CLA_ORGS: &[&str] = &["googlecloudplatform", "googleapis", "google"];
    if CLA_ORGS.contains(&org.as_str()) {
        "Drafted with assistance from Claude Code (https://claude.com/claude-code)."
    } else {
        "Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trailer_for_google_orgs() {
        for r in [
            "GoogleCloudPlatform/grpc-gcp-php",
            "googleapis/gax-php",
            "google/recaptcha",
        ] {
            assert!(
                commit_trailer(r).contains("Claude Code"),
                "expected plain-text trailer for {r}, got {}",
                commit_trailer(r)
            );
            assert!(!commit_trailer(r).contains("Co-Authored-By"));
        }
    }

    #[test]
    fn trailer_for_other_orgs() {
        for r in [
            "mockery/mockery",
            "phar-io/manifest",
            "slevomat/coding-standard",
        ] {
            assert!(
                commit_trailer(r).starts_with("Co-Authored-By"),
                "expected co-author trailer for {r}"
            );
        }
    }

    #[test]
    fn denylist_skips_mockery() {
        assert!(denylisted_repo("mockery/mockery").is_some());
        // Case-insensitive
        assert!(denylisted_repo("Mockery/Mockery").is_some());
        // Not on list
        assert!(denylisted_repo("phar-io/manifest").is_none());
        assert!(denylisted_repo("getsentry/sentry-laravel").is_none());
    }
}
