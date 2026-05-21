#!/bin/bash
#
# Open .gitattributes export-ignore PRs across multiple repos.
#
# Workflow per repo:
#   1. Fork to williamdes (if not already forked)
#   2. Clone fork into /mnt/Dev/@williamdes/@forked-repos/<repo>
#   3. Create branch williamdes/gitattributes-export-ignore
#   4. Append/create .gitattributes with the lines for that repo
#   5. Commit, push, open PR upstream with body citing the introducing commit
#   6. Sleep random 10-20s before the next one
#
# Usage:
#   ./run-prs.sh            # dry-run: print actions without doing them
#   ./run-prs.sh --go       # really do it
#
# Each PR is opened against the repo's default branch.
# Overlaps with already-open PRs (verified 2026-05-20) are excluded:
#   - php-fig/http-message: open PR #110 by savinmikhail already does this
#   - MarkBaker/PHPComplex #28 covers .github/ - we only add /examples/
#   - MarkBaker/PHPMatrix #28 covers .github/, phpstan.neon, infection.json.dist - we only add /examples/

set -euo pipefail

DRY_RUN=1
MAX_PRS=0  # 0 = unlimited
for arg in "$@"; do
    case "$arg" in
        --go) DRY_RUN=0 ;;
        --limit=*) MAX_PRS="${arg#--limit=}" ;;
    esac
done
PRS_DONE=0

# Per-script git config:
# - disable fsck (some target repos have legacy zero-padded filemodes that trip transfer.fsckObjects=true)
# - rewrite SSH GitHub URLs to HTTPS so we don't need yubikey taps mid-script
# - use `gh auth git-credential` to provide the GitHub token for HTTPS pushes
export GIT_CONFIG_PARAMETERS="'fetch.fsckObjects=false' 'receive.fsckObjects=false' 'transfer.fsckObjects=false' 'url.https://github.com/.insteadOf=git@github.com:' 'credential.helper=' 'credential.https://github.com.helper=!gh auth git-credential'"

FORK_DIR="/mnt/Dev/@williamdes/@forked-repos"
USER_LOGIN="williamdes"
BRANCH="williamdes/gitattributes-export-ignore"

PR_TITLE="Update .gitattributes to exclude dev files from composer dist"

# Schema: REPO|DEFAULT_BRANCH|CREATE_IF_MISSING|LAST_GITATTR_REF|LINES_WITH_REFS
# Each line under LINES is "<entry>|<intro/last commit ref>".
TARGETS=$(cat <<'EOF'
GoogleCloudPlatform/grpc-gcp-php|master|0|b0cf139 (2026-03-13, #59)
/doc/ export-ignore|2d903e2 2018-07-09 (initial)
/.github/ export-ignore|1049c0c 2026-03-12 (#58)
/.php_cs.dist export-ignore|2d903e2 2018-07-09 (initial)
phar-io/manifest|master|0|3d6d988 (2024-03-12, #36)
/.php-cs-fixer.dist.php export-ignore|a9fa919 2022-02-17
Intervention/validation|develop|0|e103bb1 (2025-06-15)
/phpunit.xml export-ignore|2252882 2024-02-28 (#77 PHPUnit 10)
mockery/mockery|1.6.x|0|daad681 (2026-05-05, #1483)
/CONTRIBUTING.md export-ignore|5ee21d6 2023-12-10
shuchkin/simplexlsx|master|1|none
/examples/ export-ignore|ce6559b 2022-02-21
/.gitattributes export-ignore|new
/.gitignore export-ignore|new
swaggest/php-json-schema|master|0|0ee6870 (2022-07-27, #144)
/benchmarks/ export-ignore|07e8698 2019-09-09
HubSpot/hubspot-api-php|master|1|none
/tests/ export-ignore|6319326 2026-05-11
/.github/ export-ignore|32e8839 2026-02-20
/.php-cs-fixer.php export-ignore|5750de5 2022-02-15
/.gitattributes export-ignore|new
/.gitignore export-ignore|new
getsentry/sentry-laravel|master|0|2aa81ac (2022-10-19, #601)
/phpstan.neon export-ignore|1e01c7b 2023-08-03 (#740)
/phpstan-baseline.neon export-ignore|1d1a873 2026-03-12 (#1103)
/.php-cs-fixer.php export-ignore|5a3f81b 2024-11-24 (#954)
/CONTRIBUTING.md export-ignore|fbcd9bd 2024-01-23 (#839)
slevomat/coding-standard|master|0|2d8b243 (2020-04-16)
/doc/ export-ignore|4c90045 2026-05-06
/.editorconfig export-ignore|614af5a 2021-02-03
schmittjoh/serializer|master|0|cd24e3c (2023-01-06)
/doc/ export-ignore|e5baafe 2025-07-17 (#1604)
/phpstan.neon.dist export-ignore|3d937ad 2024-11-25
/CONTRIBUTING.md export-ignore|f674fba 2026-03-26
bdelespierre/laravel-blade-linter|master|0|2203ff7 (2024-05-14)
/tests/ export-ignore|2203ff7 2024-05-14
/phpunit.xml.dist export-ignore|28082d0 2020-08-18 (initial)
EOF
)
# NOTE: namshi/jose dropped - repo last pushed 2021-06, dead.
# NOTE: PHPCSStandards/PHP_CodeSniffer dropped - they ship tests/ intentionally for downstream sniffs.
# NOTE: php-fig/http-message dropped - open PR #110 already does this.
# NOTE: MarkBaker/PHPComplex + PHPMatrix dropped - handled by review comments on open PR #28 (ziegenberg).
# NOTE: Mangopay/mangopay4-php-sdk dropped - maintainers don't want further PRs (PR #495 was the last).
# NOTE: maennchen/ZipStream-PHP dropped - owner repeatedly closed export-ignore PRs (#206, #285, #339,
#       #360, #422). Reasoning: they want GitHub Source Download to include tests, treat Composer as
#       the only supported distribution, and reject anything that strips tests/. Do not re-propose
#       until upstream comments out the intent in their .gitattributes or composer ships .composerignore.

say() { echo ">>> $*"; }

# Parse the multi-line block: each entry starts with "OWNER/REPO|".
parse_targets() {
    local current_meta=""
    local current_lines=""
    while IFS= read -r line; do
        if [[ "$line" =~ ^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+\| ]]; then
            if [ -n "$current_meta" ]; then
                printf '%s\x1f%s\x1e' "$current_meta" "$current_lines"
            fi
            current_meta="$line"
            current_lines=""
        else
            if [ -z "$current_lines" ]; then
                current_lines="$line"
            else
                current_lines+=$'\n'"$line"
            fi
        fi
    done <<< "$TARGETS"
    if [ -n "$current_meta" ]; then
        printf '%s\x1f%s\x1e' "$current_meta" "$current_lines"
    fi
}

entries=$(parse_targets)
while IFS=$'\x1e' read -r -d $'\x1e' entry; do
    [ -z "$entry" ] && continue
    meta="${entry%%$'\x1f'*}"
    lines="${entry#*$'\x1f'}"
    repo="${meta%%|*}"
    rest="${meta#*|}"
    branch="${rest%%|*}"; rest="${rest#*|}"
    create="${rest%%|*}"; rest="${rest#*|}"
    last_ga_ref="${rest}"

    say "Repo: $repo  (default=$branch, create=$create, last_gitattr=$last_ga_ref)"

    # Rejection-history check: skip the repo entirely if any recent (last 5y)
    # PR that modified .gitattributes was closed by someone OTHER than the
    # author (= rejection signal). Self-withdrawn or ≥5y stale closures don't
    # count. Saves us from proposing things the maintainer has firmly said no
    # to (e.g. maennchen/ZipStream-PHP).
    rejection_now_year=$(date +%Y)
    rejection_found=""
    closed_prs=$(gh pr list --repo "$repo" --state closed \
        --search "gitattributes OR export-ignore" \
        --json number,author,closedAt,mergedAt,state \
        --jq '[.[] | select(.state == "CLOSED" and .mergedAt == null)]' 2>/dev/null || echo "[]")
    while read -r pr_json; do
        [ -z "$pr_json" ] || [ "$pr_json" = "null" ] && continue
        pr_num=$(echo "$pr_json" | jq -r '.number')
        pr_author=$(echo "$pr_json" | jq -r '.author.login')
        pr_closed_year=$(echo "$pr_json" | jq -r '.closedAt[:4]')
        age=$((rejection_now_year - pr_closed_year))
        [ "$age" -gt 5 ] && continue
        # must actually touch .gitattributes
        touches=$(gh pr view "$pr_num" --repo "$repo" --json files --jq '[.files[].path] | any(. == ".gitattributes")' 2>/dev/null)
        [ "$touches" != "true" ] && continue
        pr_closer=$(gh api "repos/${repo}/issues/${pr_num}/timeline" \
            --jq '[.[] | select(.event == "closed")] | last | .actor.login' 2>/dev/null)
        [ "$pr_closer" = "$pr_author" ] && continue   # self-withdrawn → not a rejection
        [ "$pr_closer" = "$USER_LOGIN" ] && continue  # we closed it ourselves
        rejection_found="#${pr_num} closed by ${pr_closer} on $(echo "$pr_json" | jq -r '.closedAt[:10]')"
        break
    done < <(echo "$closed_prs" | jq -c '.[]')

    if [ -n "$rejection_found" ]; then
        say "  ✗ Maintainer rejected a similar PR ($rejection_found) - skipping ${repo}."
        continue
    fi

    # Verify each proposed path still exists in upstream HEAD before we even
    # bother showing a diff or opening a PR. If a path no longer exists
    # (renamed/removed upstream), drop it - that's a stale vendor/ artefact
    # that resolves on dependency upgrade, no PR needed.
    # Entries flagged "new" (housekeeping files we create ourselves like
    # .gitattributes) skip this check.
    filtered_lines=""
    while IFS= read -r raw; do
        [ -z "$raw" ] && continue
        text="${raw%%|*}"
        ref="${raw#*|}"
        path=$(echo "$text" | awk '{print $1}' | sed -E 's|^/||; s|/$||')
        if [ "$ref" = "new" ]; then
            filtered_lines+="${raw}"$'\n'
            continue
        fi
        status_line=$(gh api -i "repos/${repo}/contents/${path}?ref=${branch}" 2>&1 | head -1)
        if echo "$status_line" | grep -q "404"; then
            say "  ! Dropping \`${text}\` - no longer exists in ${repo}@${branch} (renamed/removed upstream)."
            continue
        fi
        filtered_lines+="${raw}"$'\n'
    done <<< "$lines"

    if [ -z "$(printf '%s' "$filtered_lines" | tr -d '\n[:space:]')" ]; then
        say "Nothing to add for ${repo} after upstream check, skipping."
        continue
    fi
    lines="${filtered_lines%$'\n'}"

    # Build PR body with citations (using the filtered list)
    PR_BODY="This avoids shipping dev/tooling files in the composer dist that aren't needed at runtime.

Last \`.gitattributes\` update was ${last_ga_ref}. The files below have been added or modified since, and are still included in the composer dist:
"
    while IFS= read -r raw; do
        [ -z "$raw" ] && continue
        line_text="${raw%%|*}"
        line_ref="${raw#*|}"
        PR_BODY+="- \`${line_text}\` - last touched in ${line_ref}"$'\n'
    done <<< "$lines"
    PR_BODY+="
Background reading: https://blog.madewithlove.be/post/gitattributes/"

    fork_name=$(basename "$repo")
    local_dir="${FORK_DIR}/${fork_name}"

    if [ "$DRY_RUN" -eq 1 ]; then
        echo "    fork:        gh repo fork $repo --clone=false"
        echo "    clone:       git@github.com:${USER_LOGIN}/${fork_name}.git → $local_dir"
        echo "    branch:      $BRANCH (from upstream/$branch)"
        echo "    file action: $( [ "$create" = "1" ] && echo CREATE || echo APPEND ) .gitattributes"
        echo "    lines to add:"
        while IFS= read -r raw; do
            [ -z "$raw" ] && continue
            echo "        + ${raw%%|*}    [intro/last: ${raw#*|}]"
        done <<< "$lines"
        echo "    --- PR title ---"
        echo "    $PR_TITLE"
        echo "    --- PR body ---"
        printf '    %s\n' "$PR_BODY" | sed 's/^    /    /'
        echo ""
        continue
    fi

    # --- Real run ---
    # Idempotency: skip if PR already exists on our branch
    existing_pr=$(gh pr list --repo "$repo" --state all --json number,headRefName,headRepositoryOwner \
        --jq "[.[] | select(.headRefName == \"${BRANCH}\" and .headRepositoryOwner.login == \"${USER_LOGIN}\")] | length" 2>/dev/null || echo 0)
    if [ "${existing_pr:-0}" -ge 1 ]; then
        say "PR already exists on ${USER_LOGIN}:${BRANCH} for ${repo}, skipping."
        continue
    fi

    if ! gh repo view "${USER_LOGIN}/${fork_name}" >/dev/null 2>&1; then
        gh repo fork "$repo" --clone=false --default-branch-only
        sleep 3
    fi

    if [ ! -d "$local_dir" ]; then
        git clone "git@github.com:${USER_LOGIN}/${fork_name}.git" "$local_dir"
    fi

    pushd "$local_dir" >/dev/null

    if ! git remote get-url upstream >/dev/null 2>&1; then
        git remote add upstream "git@github.com:${repo}.git"
    fi
    git fetch upstream "$branch"
    git checkout -B "$BRANCH" "upstream/$branch"

    if [ "$create" = "1" ] || [ ! -f .gitattributes ]; then
        : > .gitattributes
        while IFS= read -r raw; do
            [ -z "$raw" ] && continue
            printf '%s\n' "${raw%%|*}" >> .gitattributes
        done <<< "$lines"
    else
        # ---- Style detection on the existing .gitattributes ----
        # 1. Are the export-ignore lines alphabetically sorted (by stripped path)?
        # 2. Is `export-ignore` aligned in a fixed column via padding spaces?
        existing_ei=$(grep -E "[[:space:]]+export-ignore[[:space:]]*$" .gitattributes 2>/dev/null || true)
        sorted_flag=0
        align_col=0
        if [ -n "$existing_ei" ]; then
            stripped=$(echo "$existing_ei" | awk '{print $1}' | sed -E 's|^/||; s|/$||')
            sorted_chk=$(echo "$stripped" | LC_ALL=C sort -c 2>&1 || true)
            [ -z "$sorted_chk" ] && sorted_flag=1
            # If at least 2 lines have the same column position of "export-ignore",
            # treat that as the alignment target.
            col_freq=$(echo "$existing_ei" | awk '{ i = index($0, "export-ignore"); print i }' | sort | uniq -c | sort -rn | head -1)
            col_count=$(echo "$col_freq" | awk '{print $1}')
            col_pos=$(echo "$col_freq" | awk '{print $2}')
            if [ "${col_count:-0}" -ge 2 ]; then
                # is the same column hit by multiple ENTRIES with different path lengths?
                # (if all lines have the same path length, padding wouldn't be needed)
                distinct_lens=$(echo "$existing_ei" | awk '{print length($1)}' | sort -u | wc -l)
                if [ "$distinct_lens" -ge 2 ]; then
                    align_col=${col_pos:-0}
                fi
            fi
        fi
        say "  style: sorted=${sorted_flag}  align_col=${align_col}"

        # ---- Append our new entries (dedup'd), then re-sort if needed ----
        added_any=0
        while IFS= read -r raw; do
            [ -z "$raw" ] && continue
            text="${raw%%|*}"
            key=$(echo "$text" | awk '{print $1}')
            normkey=$(echo "$key" | sed -E 's|^/||; s|/$||')
            esc=$(printf '%s' "$normkey" | sed 's/[.[\*^$()+?{|]/\\&/g')
            if grep -E "^/?${esc}/?[[:space:]]+export-ignore" .gitattributes >/dev/null 2>&1; then
                continue
            fi
            # Match alignment if detected: pad path with spaces so "export-ignore"
            # lands at align_col.
            if [ "$align_col" -gt 0 ]; then
                pad=$((align_col - ${#key} - 1))
                [ "$pad" -lt 1 ] && pad=1
                printf -v spaces '%*s' "$pad" ''
                printf '%s%s%s\n' "$key" "$spaces" "export-ignore" >> .gitattributes
            else
                printf '%s\n' "$text" >> .gitattributes
            fi
            added_any=1
        done <<< "$lines"

        # If the file was alpha-sorted before our additions, sort the export-ignore
        # block in place to preserve that.
        if [ "$sorted_flag" -eq 1 ] && [ "$added_any" -eq 1 ]; then
            python3 - <<'PY' .gitattributes
import sys, re
path = sys.argv[1]
with open(path) as f:
    lines = f.read().splitlines()
ei = []
other = []
for ln in lines:
    if re.search(r'\s+export-ignore\s*$', ln):
        ei.append(ln)
    else:
        other.append(ln)
key = lambda s: re.sub(r'^/|/$', '', s.split()[0])
ei.sort(key=key)
# Heuristic: keep "other" (non-export-ignore) lines at the top in their order,
# then the sorted export-ignore block, then any trailing other lines.
# Simpler: emit non-ei lines first in original order, then ei sorted.
out = other + ei
with open(path, 'w') as f:
    f.write('\n'.join(out) + '\n')
PY
        fi
    fi

    if git diff --quiet .gitattributes 2>/dev/null && [ "$create" != "1" ]; then
        say "No changes after dedup for $repo, skipping."
        popd >/dev/null
        continue
    fi
    git add .gitattributes
    # Google-org repos require CLA - drop Co-Authored-By trailer (anthropic.com
    # email isn't CLA-signed). Use a plain-text attribution in the body instead.
    case "$repo" in
        GoogleCloudPlatform/*|googleapis/*|google/*)
            commit_body="${PR_BODY}

Drafted with assistance from Claude Code (https://claude.com/claude-code)."
            ;;
        *)
            commit_body="${PR_BODY}

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
            ;;
    esac
    git commit -m "$PR_TITLE

${commit_body}"
    git push -u origin "$BRANCH"

    gh pr create \
        --repo "$repo" \
        --base "$branch" \
        --head "${USER_LOGIN}:${BRANCH}" \
        --title "$PR_TITLE" \
        --body "$PR_BODY"

    popd >/dev/null

    PRS_DONE=$((PRS_DONE + 1))
    if [ "$MAX_PRS" -gt 0 ] && [ "$PRS_DONE" -ge "$MAX_PRS" ]; then
        say "Hit --limit=${MAX_PRS}, stopping."
        break
    fi

    wait=$((RANDOM % 11 + 10))
    say "Sleeping ${wait}s before next PR..."
    sleep "$wait"
done <<< "$entries"

say "Done."
