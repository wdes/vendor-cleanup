// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Integration-style tests for the public `style` API.
//! These mirror the most common .gitattributes shapes seen in the registry.

use pretty_assertions::assert_eq;

// re-use the binary crate library surface: a tiny shim so we can invoke the
// `style` module from a black-box test. We have to declare a small bridge here
// because vendor-cleanup is a bin crate, not a lib.
//
// For now we duplicate the public types by including the source file. If the
// project grows we can split into a lib crate.

#[path = "../src/style.rs"]
mod style;
use style::{apply_entries, strip_slashes, Style};

#[test]
fn slevomat_real_world_sort() {
    // What slevomat/coding-standard looked like before, then sorted by the
    // PR #1854 after reviewer feedback.
    let before = "\
*.php text eol=lf
/.github export-ignore
.gitattributes export-ignore
.gitignore export-ignore
/codecov.yml export-ignore
/build export-ignore
/build.xml export-ignore
/phpunit.xml.dist export-ignore
/temp export-ignore
/tests export-ignore
";
    let style = Style::detect(before);
    assert!(
        !style.sorted,
        "slevomat .gitattributes is intentionally unsorted before"
    );
    // After review, we re-sorted manually; let's verify that a sorted input
    // round-trips correctly through apply_entries.
    let sorted_input = "\
*.php text eol=lf
/.editorconfig export-ignore
.gitattributes export-ignore
/.github export-ignore
.gitignore export-ignore
/build export-ignore
/build.xml export-ignore
/codecov.yml export-ignore
/doc export-ignore
/phpunit.xml.dist export-ignore
/temp export-ignore
/tests export-ignore
";
    let style = Style::detect(sorted_input);
    assert!(style.sorted);
    // Adding `/.idea` keeps sort
    let out = apply_entries(sorted_input, &["/.idea export-ignore"], style);
    let ei: Vec<&str> = out
        .lines()
        .filter(|l| l.contains("export-ignore"))
        .collect();
    let stripped: Vec<String> = ei
        .iter()
        .map(|l| strip_slashes(l.split_whitespace().next().unwrap()))
        .collect();
    let mut want = stripped.clone();
    want.sort();
    assert_eq!(stripped, want, "sorted property must hold after append");
}

#[test]
fn phar_io_real_world_alignment() {
    // phar-io/manifest aligns export-ignore at column 19.
    let before = "\
/.github           export-ignore
/build             export-ignore
/examples          export-ignore
/tests             export-ignore
/tools             export-ignore
/.gitattributes    export-ignore
/.gitignore        export-ignore
/.php_cs.dist.php  export-ignore
/build.xml         export-ignore
/phive.xml         export-ignore
/phpunit.xml       export-ignore
/psalm.xml         export-ignore
";
    let style = Style::detect(before);
    assert!(style.align_col > 0, "should detect alignment");

    let col = style.align_col;
    // pick a short new path that fits within the alignment column so we can
    // assert exact alignment is preserved (paths longer than align_col fall
    // back to a single space and won't reach the target column).
    let out = apply_entries(before, &["/.idea export-ignore"], style);
    let new_line = out
        .lines()
        .find(|l| l.contains(".idea"))
        .expect("new line should be present");
    assert_eq!(
        new_line.find("export-ignore").unwrap(),
        col,
        "alignment column preserved on new entry"
    );
}

#[test]
fn long_path_falls_back_to_single_space_when_align_too_narrow() {
    // If the new path is longer than align_col, we can't reach the alignment
    // column; we degrade to a single space and ship anyway.
    let before = "\
/a   export-ignore
/bb  export-ignore
";
    let style = Style::detect(before);
    assert!(style.align_col > 0);
    let out = apply_entries(
        before,
        &["/this-is-way-longer-than-align export-ignore"],
        style,
    );
    let new_line = out
        .lines()
        .find(|l| l.contains("way-longer"))
        .expect("new line present");
    // Single space between path and export-ignore
    assert!(new_line.contains(" export-ignore"));
    assert!(!new_line.contains("  export-ignore"));
}

#[test]
fn create_path_from_empty() {
    let style = Style {
        sorted: false,
        align_col: 0,
    };
    let out = apply_entries(
        "",
        &[
            "/examples/ export-ignore",
            "/.gitattributes export-ignore",
            "/.gitignore export-ignore",
        ],
        style,
    );
    assert_eq!(out.lines().count(), 3);
    assert!(out.starts_with("/examples/"));
}

#[test]
fn dedup_existing_with_trailing_slash_variation() {
    // upstream has `/tests` (no trailing); we propose `/tests/` (with)
    let before = "/tests export-ignore\n";
    let style = Style::detect(before);
    let out = apply_entries(before, &["/tests/ export-ignore"], style);
    assert_eq!(out, before, "dedupes regardless of trailing slash");
}
