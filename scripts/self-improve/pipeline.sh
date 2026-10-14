#!/usr/bin/env bash
# scripts/self-improve/pipeline.sh — Fork self-improvement & upstream RC readiness pipeline.
#
# Runs non-destructively against the local checkout and GitHub API:
#   1. Syncs `origin` (xerj-org/xerj) and `unsigned` (unsigned-gg/xerj) refs
#   2. Audits ROADMAP.md `**Open defects` shortlist against live GitHub issue state
#   3. Audits open upstream issues, our open upstream PRs, and staged fork branches
#   4. Runs fast static guards (workflow-timeout-guard, landing-constants-guard)
#   5. Probes live dogfood node health (XERJ_URL / Cygnus) if reachable
#   6. Optionally (--full) runs Rust doc-drift test gates with `-j 2`
#
# Usage:
#   bash scripts/self-improve/pipeline.sh [--fast|--full] [--no-fetch]
set -euo pipefail

REPO_ROOT="$(git rev-parse --show-toplevel 2>/dev/null || (cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd))"
cd "$REPO_ROOT"

MODE="fast"
DO_FETCH=1
for arg in "$@"; do
  case "$arg" in
    --full) MODE="full" ;;
    --fast) MODE="fast" ;;
    --no-fetch) DO_FETCH=0 ;;
    -h|--help)
      sed -n '2,14p' "${BASH_SOURCE[0]}"
      exit 0
      ;;
    *)
      echo "unknown argument: $arg" >&2
      exit 2
      ;;
  esac
done

UPSTREAM_REPO="${UPSTREAM_REPO:-xerj-org/xerj}"
FORK_REPO="${FORK_REPO:-unsigned-gg/xerj}"
XERJ_PROBE_URL="${XERJ_URL:-http://xerj.tail769bd2.ts.net:9200}"

echo "=== [1/6] Git Remotes & Release Horizon ==="
if [[ "$DO_FETCH" -eq 1 ]]; then
  git fetch --quiet origin --tags
  git fetch --quiet unsigned --tags
fi

ORIGIN_SHA="$(git rev-parse --short origin/main)"
UNSIGNED_SHA="$(git rev-parse --short unsigned/main 2>/dev/null || echo "none")"
AHEAD_BEHIND="$(git rev-list --left-right --count origin/main...unsigned/main 2>/dev/null || echo "0	0")"
BEHIND_COUNT="$(awk '{print $1}' <<<"$AHEAD_BEHIND")"
AHEAD_COUNT="$(awk '{print $2}' <<<"$AHEAD_BEHIND")"

LATEST_UPSTREAM_TAG="$(
  git tag -l 'v[0-9]*.[0-9]*.[0-9]*-rc.[0-9]*' |
    grep -E '^v[0-9]+\.[0-9]+\.[0-9]+-rc\.[0-9]+$' |
    awk -F'.' '{print $NF, $0}' | sort -n | tail -1 | awk '{print $2}'
)"
LATEST_FORK_TAG="$(
  git tag -l 'v[0-9]*.[0-9]*.[0-9]*-rc.*-unsigned.*' |
    sort -V | tail -1
)"
UNRELEASED_MERGES="$(git log --first-parent --oneline "${LATEST_UPSTREAM_TAG}..origin/main" | wc -l | tr -d ' ')"

echo "  origin/main:         ${ORIGIN_SHA}"
echo "  unsigned/main:       ${UNSIGNED_SHA} (${AHEAD_COUNT} ahead, ${BEHIND_COUNT} behind origin/main)"
echo "  latest upstream tag: ${LATEST_UPSTREAM_TAG} (${UNRELEASED_MERGES} first-parent commits on origin/main since tag)"
echo "  latest fork RC tag:  ${LATEST_FORK_TAG:-<none>}"

echo ""
echo "=== [2/6] ROADMAP.md Open-Defects Shortlist Audit ==="
ROADMAP_ISSUES="$(
  awk '
    /^\*\*Open defects/ { inside=1 }
    inside && /^## /    { exit }
    inside              { print }
  ' ROADMAP.md |
    grep -oE 'issues/[0-9]+' |
    awk -F/ '{print $2}' |
    sort -u -n
)"
STALE_ROADMAP=0
ROADMAP_COUNT=0
for num in $ROADMAP_ISSUES; do
  ROADMAP_COUNT=$((ROADMAP_COUNT + 1))
  state="$(gh api "repos/${UPSTREAM_REPO}/issues/${num}" --jq '.state' 2>/dev/null || echo "unknown")"
  if [[ "$state" != "open" ]]; then
    echo "  [DRIFT] ROADMAP.md **Open defects lists #${num}, but state on ${UPSTREAM_REPO} is '${state}'"
    STALE_ROADMAP=$((STALE_ROADMAP + 1))
  fi
done
echo "  ROADMAP.md shortlist: ${ROADMAP_COUNT} issues checked, ${STALE_ROADMAP} stale/closed."

echo ""
echo "=== [3/6] Upstream PRs (author: todie) & Ready Fork Branches ==="
gh pr list --repo "$UPSTREAM_REPO" --author todie --state open \
  --json number,title,headRefName,mergeStateStatus \
  --jq '.[] | "  PR #\(.number) [\(.headRefName)] (\(.mergeStateStatus)): \(.title)"' || true

echo "  Staged branches on unsigned (ahead of origin/main):"
while IFS= read -r ref; do
  [[ -z "$ref" ]] && continue
  branch="${ref#unsigned/}"
  [[ "$branch" == "main" || "$branch" == "HEAD" ]] && continue
  ahead="$(git rev-list --count "origin/main..${ref}" 2>/dev/null || echo 0)"
  if [[ "$ahead" -gt 0 ]]; then
    subj="$(git log -1 --format='%h %s' "$ref")"
    echo "    - ${branch} (+${ahead}): ${subj}"
  fi
done < <(git for-each-ref --format='%(refname:short)' refs/remotes/unsigned/)

echo ""
echo "=== [4/6] New Upstream Issues Since ${LATEST_UPSTREAM_TAG} ==="
gh issue list --repo "$UPSTREAM_REPO" --state open --limit 10 \
  --json number,title,createdAt \
  --jq '.[] | "  #\(.number) [\(.createdAt[0:10])] \(.title)"' || true

echo ""
echo "=== [5/6] Static Guards ==="
python3 .github/scripts/workflow-timeout-guard.py
bash .github/scripts/landing-constants-guard.sh

if [[ "$MODE" == "full" ]]; then
  echo "  Running Rust doc-drift gates (-j 2)..."
  (cd engine && cargo test -j 2 -p xerj-engine --test docs_capability_lists --test docs_do_not_promise_the_next_release --quiet)
  echo "  Rust doc-drift gates: PASS"
fi

echo ""
echo "=== [6/6] Live Dogfood Smoke Probe (${XERJ_PROBE_URL}) ==="
if health_json="$(curl -fsS --max-time 3 "${XERJ_PROBE_URL}/_cluster/health" 2>/dev/null)"; then
  status="$(python3 -c 'import json,sys; d=json.load(sys.stdin); print("status=%s nodes=%s shards=%s" % (d.get("status"), d.get("number_of_nodes"), d.get("active_primary_shards")))' <<<"$health_json" 2>/dev/null || echo "reachable")"
  echo "  Cygnus node: ONLINE (${status})"
else
  echo "  Cygnus node: SKIPPED (not reachable at ${XERJ_PROBE_URL})"
fi

mkdir -p .dagr
cat > .dagr/self-improve-latest.json <<EOF
{
  "timestamp": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "origin_main": "${ORIGIN_SHA}",
  "unsigned_main": "${UNSIGNED_SHA}",
  "ahead_of_origin": ${AHEAD_COUNT},
  "behind_origin": ${BEHIND_COUNT},
  "latest_upstream_tag": "${LATEST_UPSTREAM_TAG}",
  "latest_fork_tag": "${LATEST_FORK_TAG:-}",
  "unreleased_upstream_merges": ${UNRELEASED_MERGES},
  "roadmap_open_defects_checked": ${ROADMAP_COUNT},
  "roadmap_stale_defects": ${STALE_ROADMAP}
}
EOF
echo ""
echo "Self-improvement pipeline check complete. Report written to .dagr/self-improve-latest.json"
