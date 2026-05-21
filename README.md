# vendor-cleanup

Open `.gitattributes export-ignore` PRs at scale against PHP/Composer packages
that still ship dev files (`tests/`, `phpunit.xml`, `.github/`, …) in their
composer dist.

Designed to be polite, evidence-citing, and safe: each PR body references the
last `.gitattributes` change in the repo plus the commit/PR that introduced
every file we want excluded. Pre-flight checks skip repos where the maintainer
has already closed similar PRs, and drop entries whose paths no longer exist
upstream.

## Why

Many PHP libraries ship `tests/`, IDE configs and CI workflows in the composer
dist tarball, bloating every server's `vendor/` directory with files that are
only useful for development. Adding `export-ignore` rules in `.gitattributes`
is the supported fix; this tool batches the PR-opening across many libraries.

## Features

- **Rejection-history check**: skip the repo if a non-author closed a similar
  PR in the last 5 years (catches maintainers who said no before).
- **Upstream existence check**: drop entries whose paths no longer exist in
  upstream HEAD (catches stale-rename false positives like `phpunit.xml` ->
  `phpunit.xml.dist`).
- **Style detection**: preserve the existing `.gitattributes` style:
  - Alpha-sort if the existing file is sorted.
  - Pad with spaces if `export-ignore` is aligned in a fixed column.
- **CLA-org awareness**: for `GoogleCloudPlatform/*`, `googleapis/*`,
  `google/*`, replace `Co-Authored-By` trailer with a plain-text attribution
  so the Google CLA bot doesn't block on the Anthropic co-author email.
- **HTTPS auth via `gh auth git-credential`**: no SSH key / yubikey taps mid
  campaign.
- **Idempotent**: skip repos where a PR already exists on the same head branch.
- **Skip list with reasons**: track repos that maintainers won't accept.

## Quick start

```bash
gh auth login                # if not already
./bin/vendor-cleanup --config examples/sample-campaign.yaml         # dry-run
./bin/vendor-cleanup --config examples/sample-campaign.yaml --go    # really do it
./bin/vendor-cleanup --config <file> --go --limit=2              # stop after 2 PRs
```

## Configuration

YAML file. See `examples/` for a real sample.

```yaml
defaults:
  fork_dir: /home/me/forks
  user_login: my-gh-handle
  branch: gh-handle/gitattributes-export-ignore
  pr_title: "Update .gitattributes to exclude dev files from composer dist"

targets:
  - repo: schmittjoh/serializer
    branch: master
    create: false
    last_gitattributes_ref: "cd24e3c (2023-01-06)"
    entries:
      - line: "/doc/ export-ignore"
        ref: "e5baafe 2025-07-17 (#1604)"
      - line: "/phpstan.neon.dist export-ignore"
        ref: "3d937ad 2024-11-25"
      - line: "/CONTRIBUTING.md export-ignore"
        ref: "f674fba 2026-03-26"

  - repo: shuchkin/simplexlsx
    branch: master
    create: true               # no existing .gitattributes upstream
    entries:
      - line: "/examples/ export-ignore"
        ref: "ce6559b 2022-02-21"

skipped:
  - repo: maennchen/ZipStream-PHP
    reason: |
      Owner has repeatedly closed export-ignore PRs (#206, #285, #339, #422).
      They want GitHub Source Download to include tests. Do not re-propose.
```

## How the PR body looks

```
This avoids shipping dev/tooling files in the composer dist that aren't
needed at runtime.

Last `.gitattributes` update was 2aa81ac (2022-10-19, #601). The files below
have been added or modified since, and are still included in the composer dist:
- `/phpstan.neon export-ignore` - last touched in 1e01c7b 2023-08-03 (#740)
- `/phpstan-baseline.neon export-ignore` - last touched in 1d1a873 2026-03-12 (#1103)
- `/.php-cs-fixer.php export-ignore` - last touched in 5a3f81b 2024-11-24 (#954)
- `/CONTRIBUTING.md export-ignore` - last touched in fbcd9bd 2024-01-23 (#839)

Background reading: https://blog.madewithlove.be/post/gitattributes/
```

## Helping populate the config

There's a companion scanner that reads a project's `composer.lock` + `vendor/`
and emits a draft YAML config with last-touch commit refs per entry. See
`bin/vendor-cleanup-scan`. (TODO: extract from internal use.)

## License

MPL-2.0. See `LICENSE`.
