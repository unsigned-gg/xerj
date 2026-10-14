# Fork Self-Improvement & Upstream RC Pipeline (`scripts/self-improve/`)

This directory implements the local CLI + scheduled agent self-improvement loop for the `unsigned-gg/xerj` fork and generates verified Release Candidates for both `unsigned-gg/xerj` and upstream `xerj-org/xerj`.

By keeping this tooling under `scripts/self-improve/` and running the recurring automation from `ceres` instead of adding fork-only workflows under `.github/workflows/`, upstream PR branches never carry workflow drift.

## 1. Self-Improvement Audit (`pipeline.sh`)

```bash
# Fast pass (remotes, ROADMAP shortlist audit, PR/issue triage, static guards, Cygnus smoke probe):
bash scripts/self-improve/pipeline.sh --fast

# Full pass (adds Rust doc-drift test suite with -j 2):
bash scripts/self-improve/pipeline.sh --full
```

What `pipeline.sh` checks on each run:
1. **Git Remotes & Release Horizon:** Fetches `origin` (`xerj-org/xerj`) and `unsigned` (`unsigned-gg/xerj`), reports ahead/behind counts, the latest upstream `v1.0.0-rc.<N>` tag, the latest fork `v1.0.0-rc.<N>-unsigned.<M>` tag, and first-parent merges on `origin/main` since the last upstream release.
2. **`ROADMAP.md` Open-Defects Shortlist Audit:** Parses every `issues/<N>` link in the `**Open defects` section of `ROADMAP.md` and verifies against the live GitHub API (`repos/xerj-org/xerj/issues/<N>`) that none have closed since the last release cut.
3. **Upstream PRs & Staged Fork Branches:** Lists open PRs by `todie` on `xerj-org/xerj` alongside staged `fix/*`, `perf/*`, and `release/*` branches on `unsigned-gg/xerj`.
4. **Static Conformance & Release Guards:** Runs `.github/scripts/workflow-timeout-guard.py` and `.github/scripts/landing-constants-guard.sh`.
5. **Live Dogfood Smoke Probe:** Queries `/_cluster/health` against `XERJ_URL` (default `http://xerj.tail769bd2.ts.net:9200` on Cygnus).
6. **Structured Report:** Writes `.dagr/self-improve-latest.json` for scheduled agent inspection.

## 2. Cutting Release Candidates (`cut-rc.sh`)

### A. Fork Pre-Release Tags (`v1.0.0-rc.<N>-unsigned.<M>`)

To cut and push a signed pre-release tag on `unsigned-gg/xerj` (including fork fixes ahead of `origin/main`):

```bash
bash scripts/self-improve/cut-rc.sh --mode fork --rc 1.0.0-rc.96 --push
```

### B. Upstream Release Candidate Branch (`release/rc<N>`)

To prepare and verify a clean upstream `release/rc<N>` branch on top of `origin/main`:

```bash
git checkout -B release/rc96 origin/main
# 1. Populate CHANGELOG.md ## [1.0.0-rc.96] and ROADMAP.md rc.96 roll
# 2. Stamp version, Cargo.lock, landing footers, data-latest-tag, and sitemap.xml, then run all 4 gates:
bash scripts/self-improve/cut-rc.sh --mode upstream --rc 1.0.0-rc.96
```

`cut-rc.sh --mode upstream` enforces all four upstream release merge gates before a branch is pushed:
- `.github/scripts/release-notes-gate.sh origin/main` (every `ROADMAP.md` open defect is open on GitHub, every `CHANGELOG.md` `### Fixed` issue is closed or marked partial, and every merged PR since `v1.0.0-rc.<N-1>` is cited or exempted)
- `.github/scripts/landing-constants-guard.sh` (footer version stamps, `data-latest-tag`, and `sitemap.xml` freshness)
- `.github/scripts/workflow-timeout-guard.py`
- `cargo test -j 2 -p xerj-engine --test docs_capability_lists --test docs_do_not_promise_the_next_release`
