#!/usr/bin/env bash
# scripts/self-improve/cut-rc.sh — Cut a fork RC tag or prepare/verify an upstream RC branch.
#
# Modes:
#   1. Fork pre-release tag on unsigned-gg/xerj:
#      bash scripts/self-improve/cut-rc.sh --mode fork [--rc 1.0.0-rc.96] [--seq 1] [--push]
#      Creates signed tag `v<RC>-unsigned.<SEQ>` on HEAD (or unsigned/main) and optionally pushes to `unsigned`.
#
#   2. Upstream RC branch preparation & gate verification:
#      bash scripts/self-improve/cut-rc.sh --mode upstream --rc 1.0.0-rc.96 [--date YYYY-MM-DD] [--verify-only]
#      Stamps engine/Cargo.toml, engine/Cargo.lock, landing/ footers + data-latest-tag + sitemap.xml,
#      and verifies all upstream release gates:
#        - .github/scripts/release-notes-gate.sh origin/main
#        - .github/scripts/landing-constants-guard.sh
#        - .github/scripts/workflow-timeout-guard.py
#        - cargo test -j 2 -p xerj-engine --test docs_capability_lists --test docs_do_not_promise_the_next_release
set -euo pipefail

REPO_ROOT="$(git rev-parse --show-toplevel 2>/dev/null || (cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd))"
cd "$REPO_ROOT"

MODE=""
RC_VERSION=""
SEQ=""
PUSH_TAG=0
VERIFY_ONLY=0
CUT_DATE="$(date -u +%Y-%m-%d)"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --mode)
      MODE="$2"
      shift 2
      ;;
    --rc)
      RC_VERSION="${2#v}"
      shift 2
      ;;
    --seq)
      SEQ="$2"
      shift 2
      ;;
    --date)
      CUT_DATE="$2"
      shift 2
      ;;
    --push)
      PUSH_TAG=1
      shift
      ;;
    --verify-only)
      VERIFY_ONLY=1
      shift
      ;;
    -h|--help)
      sed -n '2,16p' "${BASH_SOURCE[0]}"
      exit 0
      ;;
    *)
      echo "unknown argument: $1" >&2
      exit 2
      ;;
  esac
done

if [[ -z "$MODE" ]]; then
  echo "error: --mode fork|upstream is required" >&2
  exit 2
fi

latest_upstream_rc() {
  git tag -l 'v[0-9]*.[0-9]*.[0-9]*-rc.[0-9]*' |
    grep -E '^v[0-9]+\.[0-9]+\.[0-9]+-rc\.[0-9]+$' |
    awk -F'.' '{print $NF, $0}' | sort -n | tail -1 | awk '{print $2}'
}

if [[ -z "$RC_VERSION" ]]; then
  prev_tag="$(latest_upstream_rc)"
  prev_num="${prev_tag##*.}"
  next_num=$((prev_num + 1))
  RC_VERSION="1.0.0-rc.${next_num}"
fi

if [[ "$MODE" == "fork" ]]; then
  if [[ -z "$SEQ" ]]; then
    existing="$(git tag -l "v${RC_VERSION}-unsigned.*" | awk -F'.' '{print $NF}' | sort -n | tail -1)"
    SEQ=$(( ${existing:-0} + 1 ))
  fi
  TAG_NAME="v${RC_VERSION}-unsigned.${SEQ}"
  echo "=== Cutting Fork Pre-Release Tag: ${TAG_NAME} ==="
  python3 .github/scripts/workflow-timeout-guard.py
  if git rev-parse -q --verify "refs/tags/${TAG_NAME}" >/dev/null; then
    echo "Tag ${TAG_NAME} already exists locally ($(git rev-parse --short "refs/tags/${TAG_NAME}"))."
  else
    git tag -s "$TAG_NAME" -m "XERJ ${TAG_NAME} (unsigned-gg fork pre-release)"
    echo "Created signed tag ${TAG_NAME} at $(git rev-parse --short HEAD)."
  fi
  if [[ "$PUSH_TAG" -eq 1 ]]; then
    git push unsigned "refs/tags/${TAG_NAME}"
    echo "Pushed ${TAG_NAME} to remote 'unsigned'."
  fi
  exit 0
fi

if [[ "$MODE" != "upstream" ]]; then
  echo "error: unsupported --mode '$MODE' (expected 'fork' or 'upstream')" >&2
  exit 2
fi

RC_NUM="${RC_VERSION##*rc.}"
PREV_NUM=$((RC_NUM - 1))
PREV_RC="1.0.0-rc.${PREV_NUM}"

if [[ "$VERIFY_ONLY" -eq 0 ]]; then
  echo "=== Stamping Upstream Release Candidate v${RC_VERSION} (from v${PREV_RC}) ==="
  # 1. Stamp engine/Cargo.toml workspace version
  python3 - "$RC_VERSION" <<'PY'
import pathlib, re, sys
ver = sys.argv[1]
p = pathlib.Path("engine/Cargo.toml")
s = p.read_text()
s2, n = re.subn(r'(\[workspace\.package\]\s*\nversion\s*=\s*)"[^"]+"', rf'\1"{ver}"', s, count=1)
if n != 1:
    raise SystemExit("failed to update [workspace.package] version in engine/Cargo.toml")
p.write_text(s2)
PY

  # 2. Refresh workspace member versions in engine/Cargo.lock
  (cd engine && cargo update -w --offline --quiet 2>/dev/null || cargo metadata --format-version 1 >/dev/null)

  # 3. Sweep landing/index.html data-latest-tag and all landing/**/*.html footer stamps
  python3 - "$RC_VERSION" "$RC_NUM" "$PREV_NUM" <<'PY'
import pathlib, re, sys
rc_ver, rc_num, prev_num = sys.argv[1], sys.argv[2], sys.argv[3]
idx = pathlib.Path("landing/index.html")
idx_text = idx.read_text()
idx_text = re.sub(
    r'(<span data-latest-tag>)v1\.0\.0-rc\.\d+(</span>)',
    rf'\g<1>v{rc_ver}\2',
    idx_text,
)
idx.write_text(idx_text)

swept = 0
for html in pathlib.Path("landing").rglob("*.html"):
    t = html.read_text()
    t2 = t.replace(f"V1.0.0-RC.{prev_num}", f"V1.0.0-RC.{rc_num}")
    if t2 != t:
        html.write_text(t2)
        swept += 1
print(f"Swept footer stamps in {swept} HTML files under landing/ to V1.0.0-RC.{rc_num}")
PY

  # 4. Update ROADMAP.md review line date and version
  python3 - "$RC_VERSION" "$CUT_DATE" <<'PY'
import pathlib, re, sys
rc_ver, cut_date = sys.argv[1], sys.argv[2]
p = pathlib.Path("ROADMAP.md")
s = p.read_text()
s2, n = re.subn(
    r'Last reviewed: \d{4}-\d{2}-\d{2} \(against `v1\.0\.0-rc\.\d+` and `main`\)\.',
    f'Last reviewed: {cut_date} (against `v{rc_ver}` and `main`).',
    s,
    count=1,
)
if n != 1:
    raise SystemExit("failed to update Last reviewed line in ROADMAP.md")
p.write_text(s2)
PY

  # 5. Regenerate landing/sitemap.xml so lastmod matches modified landing pages
  python3 scripts/seo/gen_sitemap.py --write
fi

echo ""
echo "=== Running Upstream Release Verification Gates for v${RC_VERSION} ==="
echo "  [1/4] .github/scripts/release-notes-gate.sh origin/main"
REPO="${UPSTREAM_REPO:-xerj-org/xerj}" bash .github/scripts/release-notes-gate.sh origin/main

echo "  [2/4] .github/scripts/landing-constants-guard.sh"
bash .github/scripts/landing-constants-guard.sh

echo "  [3/4] .github/scripts/workflow-timeout-guard.py"
python3 .github/scripts/workflow-timeout-guard.py

echo "  [4/4] Rust doc-drift gates (docs_capability_lists + docs_do_not_promise_the_next_release)"
(cd engine && cargo test -j 2 -p xerj-engine --test docs_capability_lists --test docs_do_not_promise_the_next_release)

echo ""
echo "All upstream release gates PASSED for v${RC_VERSION}."
