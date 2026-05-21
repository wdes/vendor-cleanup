// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Detect and preserve the style of an existing `.gitattributes` file
//! when appending new `export-ignore` entries.

use regex::Regex;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Style {
    /// Existing `export-ignore` lines are alphabetically sorted by stripped path.
    pub sorted: bool,
    /// Column where `export-ignore` is aligned via padding. 0 means no alignment.
    pub align_col: usize,
}

impl Style {
    /// Detect the style of an existing `.gitattributes` content.
    pub fn detect(content: &str) -> Self {
        let ei_re = Regex::new(r"\s+export-ignore\s*$").unwrap();
        let lines: Vec<&str> = content.lines().filter(|l| ei_re.is_match(l)).collect();

        if lines.is_empty() {
            return Style {
                sorted: false,
                align_col: 0,
            };
        }

        // sort check
        let stripped: Vec<String> = lines
            .iter()
            .map(|l| {
                let first = l.split_whitespace().next().unwrap_or("");
                strip_slashes(first)
            })
            .collect();
        let mut sorted_clone = stripped.clone();
        sorted_clone.sort();
        let sorted = stripped == sorted_clone;

        // alignment check: most-frequent column of `export-ignore` if hit by
        // ≥ 2 lines AND the path lengths differ.
        let mut col_freq: std::collections::HashMap<usize, usize> =
            std::collections::HashMap::new();
        for l in &lines {
            if let Some(idx) = l.find("export-ignore") {
                *col_freq.entry(idx).or_insert(0) += 1;
            }
        }
        let (top_col, top_count) = col_freq
            .iter()
            .max_by_key(|(_, n)| *n)
            .map(|(c, n)| (*c, *n))
            .unwrap_or((0, 0));
        let distinct_lens = lines
            .iter()
            .map(|l| l.split_whitespace().next().unwrap_or("").len())
            .collect::<std::collections::HashSet<_>>()
            .len();
        let align_col = if top_count >= 2 && distinct_lens >= 2 {
            top_col
        } else {
            0
        };
        Style { sorted, align_col }
    }
}

/// Strip leading `/` and trailing `/` from a path entry.
pub fn strip_slashes(s: &str) -> String {
    s.trim_start_matches('/').trim_end_matches('/').to_string()
}

/// Remove every `export-ignore` line in `content` whose normalized
/// path appears in `paths_to_remove`. Comparison is slash-insensitive
/// (so `"travis.yml"` removes both `/travis.yml` and `/.travis.yml`
/// variants when normalized identically; pass paths without the
/// leading dot if you want exact match — strip_slashes is applied to
/// both sides).
pub fn apply_removals(content: &str, paths_to_remove: &[&str]) -> String {
    if paths_to_remove.is_empty() {
        return content.to_string();
    }
    let normalized: std::collections::HashSet<String> =
        paths_to_remove.iter().map(|p| strip_slashes(p)).collect();
    let ei_re = Regex::new(r"^\s*/?([^\s]+?)/?\s+export-ignore\s*$").unwrap();
    let mut out = String::with_capacity(content.len());
    let preserve_trailing_nl = content.ends_with('\n');
    let lines: Vec<&str> = content.lines().collect();
    for line in &lines {
        if let Some(caps) = ei_re.captures(line) {
            if normalized.contains(&caps[1]) {
                continue;
            }
        }
        out.push_str(line);
        out.push('\n');
    }
    if !preserve_trailing_nl {
        // Strip the trailing newline we just added if the input didn't have one.
        if out.ends_with('\n') {
            out.pop();
        }
    }
    out
}

/// Apply the given entries to `content`, respecting the detected style.
/// Returns the new content. Entries whose path is already present are deduped.
pub fn apply_entries(content: &str, entries_to_add: &[&str], style: Style) -> String {
    let ei_re = Regex::new(r"^\s*/?([^\s]+?)/?\s+export-ignore\s*$").unwrap();
    let mut existing_paths: std::collections::HashSet<String> = std::collections::HashSet::new();
    for line in content.lines() {
        if let Some(caps) = ei_re.captures(line) {
            existing_paths.insert(caps[1].to_string());
        }
    }

    let mut new_lines: Vec<String> = Vec::new();
    for raw in entries_to_add {
        let key = raw.split_whitespace().next().unwrap_or("");
        let norm = strip_slashes(key);
        if existing_paths.contains(&norm) {
            continue;
        }
        let formatted = if style.align_col > 0 {
            // align_col is the 0-indexed column where "export-ignore" should start.
            let pad = style.align_col.saturating_sub(key.len()).max(1);
            format!("{}{}export-ignore", key, " ".repeat(pad))
        } else {
            (*raw).to_string()
        };
        new_lines.push(formatted);
        existing_paths.insert(norm);
    }

    let mut body = String::new();
    body.push_str(content);
    if !content.is_empty() && !content.ends_with('\n') {
        body.push('\n');
    }
    for nl in &new_lines {
        body.push_str(nl);
        body.push('\n');
    }

    if style.sorted && !new_lines.is_empty() {
        // Re-sort the export-ignore block: non-EI lines first in original order,
        // then sorted EI lines.
        let lines: Vec<&str> = body.lines().collect();
        let (ei, other): (Vec<&str>, Vec<&str>) = lines
            .iter()
            .partition(|l| Regex::new(r"\s+export-ignore\s*$").unwrap().is_match(l));
        let mut ei_sorted = ei;
        ei_sorted.sort_by_key(|l| strip_slashes(l.split_whitespace().next().unwrap_or("")));
        let mut out = String::new();
        for l in other.iter().chain(ei_sorted.iter()) {
            out.push_str(l);
            out.push('\n');
        }
        body = out;
    }

    body
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_no_style_on_empty_file() {
        let s = Style::detect("");
        assert!(!s.sorted);
        assert_eq!(s.align_col, 0);
    }

    #[test]
    fn detects_sorted_simple() {
        let content = "\
/.editorconfig export-ignore
/.github export-ignore
/tests/ export-ignore
";
        let s = Style::detect(content);
        assert!(s.sorted);
        assert_eq!(s.align_col, 0);
    }

    #[test]
    fn detects_unsorted_simple() {
        let content = "\
/tests/ export-ignore
/.github export-ignore
";
        let s = Style::detect(content);
        assert!(!s.sorted);
    }

    #[test]
    fn detects_alignment() {
        // export-ignore at column 24 (1-indexed find()) on multiple lines
        let content = "\
/.github           export-ignore
/build             export-ignore
/build.xml         export-ignore
/.editorconfig     export-ignore
";
        let s = Style::detect(content);
        assert_ne!(s.align_col, 0);
        // Re-find on the first EI line to validate
        let first = content.lines().next().unwrap();
        assert_eq!(s.align_col, first.find("export-ignore").unwrap());
    }

    #[test]
    fn no_alignment_when_all_paths_same_length() {
        let content = "\
/aaa export-ignore
/bbb export-ignore
";
        let s = Style::detect(content);
        assert_eq!(s.align_col, 0);
    }

    #[test]
    fn append_dedups_existing_paths() {
        let content = "/tests/ export-ignore\n";
        let new = apply_entries(content, &["/tests/ export-ignore"], Style::detect(content));
        // No new line should be added.
        assert_eq!(new.lines().count(), 1);
    }

    #[test]
    fn append_dedups_normalized_slashes() {
        let content = "/tests export-ignore\n";
        // proposing "/tests/" (with trailing slash) should still dedup
        let new = apply_entries(content, &["/tests/ export-ignore"], Style::detect(content));
        assert_eq!(new.lines().count(), 1);
    }

    #[test]
    fn apply_removals_drops_named_lines() {
        let content = "\
*.php text eol=lf
/.travis.yml export-ignore
/phpunit.xml export-ignore
/tests/ export-ignore
";
        let out = apply_removals(content, &["/.travis.yml"]);
        assert!(!out.contains(".travis.yml"));
        assert!(out.contains("/phpunit.xml export-ignore"));
        assert!(out.contains("/tests/ export-ignore"));
        assert!(out.contains("*.php text eol=lf"));
    }

    #[test]
    fn apply_removals_is_slash_insensitive() {
        let content = "/.travis.yml export-ignore\n";
        // Removal passed without leading slash should still match
        let out = apply_removals(content, &[".travis.yml"]);
        assert_eq!(out, "");
    }

    #[test]
    fn apply_removals_no_op_when_empty() {
        let content = "/tests export-ignore\n";
        let out = apply_removals(content, &[]);
        assert_eq!(out, content);
    }

    #[test]
    fn apply_removals_keeps_unrelated_export_ignore() {
        // Removing /.travis.yml must NOT remove unrelated entries that contain "travis" substring.
        let content = "\
/.travis.yml export-ignore
/.travis-deploy/ export-ignore
";
        let out = apply_removals(content, &["/.travis.yml"]);
        assert!(!out.contains(".travis.yml"));
        assert!(out.contains(".travis-deploy"));
    }

    #[test]
    fn append_preserves_sort_when_sorted() {
        let content = "\
.gitattributes export-ignore
/build export-ignore
/tests export-ignore
";
        let style = Style::detect(content);
        assert!(style.sorted);
        let new = apply_entries(content, &["/.editorconfig export-ignore"], style);
        let ei_lines: Vec<&str> = new
            .lines()
            .filter(|l| l.contains("export-ignore"))
            .collect();
        let stripped: Vec<String> = ei_lines
            .iter()
            .map(|l| strip_slashes(l.split_whitespace().next().unwrap()))
            .collect();
        let mut want = stripped.clone();
        want.sort();
        assert_eq!(stripped, want, "EI block should remain sorted after append");
    }

    #[test]
    fn append_uses_alignment_padding() {
        let content = "\
/.github           export-ignore
/build             export-ignore
/build.xml         export-ignore
";
        let style = Style::detect(content);
        assert_ne!(style.align_col, 0);
        let col = style.align_col;
        let new = apply_entries(content, &["/.editorconfig export-ignore"], style);
        // Find the new line
        let added = new.lines().find(|l| l.contains(".editorconfig")).unwrap();
        assert_eq!(
            added.find("export-ignore").unwrap(),
            col,
            "new line should align to column {col}, got: {added:?}"
        );
    }

    #[test]
    fn append_no_alignment_keeps_raw() {
        let content = "/tests export-ignore\n";
        let style = Style::detect(content);
        let new = apply_entries(content, &["/docs/ export-ignore"], style);
        assert!(new.contains("/docs/ export-ignore"));
    }

    #[test]
    fn strip_slashes_normalizes() {
        assert_eq!(strip_slashes("/tests/"), "tests");
        assert_eq!(strip_slashes("/.editorconfig"), ".editorconfig");
        assert_eq!(strip_slashes("plain"), "plain");
    }
}
