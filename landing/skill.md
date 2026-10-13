---
name: xerj
description: Search local codebases, mixed-format folders, long-term agent memory, and reference-coding corpora with XERJ — via CLI (`xerj search`, `xerj def`, `xerj autoindex`, `xerj code`) or MCP (`xerj_search`, `xerj_map`, `xerj_code_search`, `xerj_memory_*`).
---

# XERJ — Agent Skill

XERJ is an AI-native search engine in a single Rust binary. `xerj autoindex <folder>` turns mixed-format files (code, PDF, DOCX, SQLite, CSV, logs, mbox) into typed, self-describing indices with zero configuration; agents query them over CLI, MCP (`xerj mcp`), or an Elasticsearch-8.x-compatible HTTP API.

Canonical index: [https://xerj.org/llms.txt](https://xerj.org/llms.txt) · Step-by-step install playbook: [https://xerj.org/llms-install.md](https://xerj.org/llms-install.md) · Full reference: [https://xerj.org/llms-full.txt](https://xerj.org/llms-full.txt).

---

## 1. Anti-priors (check before acting or describing results)

1. **Default embedding mode is lexical, not neural.** Without `--embed-mode neural` (or `proxy`) at server start, `semantic` queries and `_memory` recall use a deterministic 384-d feature-hashing embedder (word / sub-word overlap, not paraphrase/meaning). `--embed-mode` applies at **ingest**; switching later requires re-indexing.
2. **Unknown fields on `ax-*` indices return HTTP 400.** Autoindex indices have explicit schemas and reject unmapped fields instead of silently returning 0 hits. Call `xerj_map` or `xerj autoindex map --json` first to inspect field names.
3. **Default `_search` returns whole documents.** On HTTP `_search`, project `_source` (e.g. `["ax_path","title"]`) and pass `"fields":["_passage"]` to return only the matching snippet (`34x` smaller response). Note: omit `"fields":["_passage"]` on `hybrid` or multi-kNN fusion queries (returns HTTP 400).
4. **Unfiltered kNN is approximate (HNSW + exact rescore); filtered kNN is exact brute-force.** Unfiltered cosine kNN on `>= 1,024` docs searches HNSW (`num_candidates` floored at 800; measured 100-probe recall@10 mean `0.976`). Filtered kNN, nested kNN, SQ8, `< 1,024` docs, and `semantic`/`_memory` recall run an exact scan.
5. **Hybrid fusion is `"rrf"` or `"linear"` only.** `"fusion":"learned"` returns HTTP 400.
6. **`rerank` sends hit text off the node.** Adding `"rerank":{...}` sends candidate text to an external judge when configured by the operator. Never add `rerank` on data that must stay local without user approval.

---

## 2. CLI quickstart (when you have a shell)

```sh
# One-command Searches (symbol definitions ranked first):
xerj search "<query in plain words>"
xerj def "<SymbolName>"                       # exact symbol definition: file:line + signature

# Inspect indexed datasets, fields, and sample queries:
xerj autoindex map --json

# Index a folder (always --dry-run first and report estimate to the user):
xerj autoindex <folder> --dry-run
xerj autoindex <folder>                       # exit 0 or 3 = success; exit 4 = ask user & pass --approve

# Reference coding against pinned peer corpora (Corpus Hub):
curl -fsSLO https://raw.githubusercontent.com/xerj-org/xerj/corpus-hub/tools/xerj-code/hub/<slug>.json
xerj corpus add --from ./<slug>.json
xerj corpus index <slug>
xerj code <slug> "<mechanism or symbol>"
```

- If `xerj` is not on `PATH`, run `$HOME/.local/bin/xerj` or `export PATH="$HOME/.local/bin:$PATH"`.
- Against an auth-enabled node, set `XERJ_API_KEY="$(cat "$HOME/xerj-data/admin.key")"`.

---

## 3. MCP tools (`xerj mcp` — 13 tools)

Published schemas: [https://xerj.org/docs/agents/schemas/mcp-tools.json](https://xerj.org/docs/agents/schemas/mcp-tools.json)

| Tool | Purpose & required arguments |
|---|---|
| `xerj_search` | Search `index` (`"ax-*"` or specific index) with a plain string `query` (definition-first ranking) or an ES query-DSL object. |
| `xerj_map` | Read `autoindex-catalog` (`prefix` optional) to discover datasets, field names/types, and example queries. |
| `xerj_code_search` | Run the reference-coding pipeline on `corpus` (`xc-<corpus>-*`) for `query`, with staleness checks and licence warnings. |
| `xerj_plan` | Deterministic rule-based query planner (`POST /_ask`): turns `{index, prompt}` into a validated ES query DSL object. |
| `xerj_semantic_search` | Query a `semantic_text` field (`index`, `field`, `query`, `k`). Lexical unless the node runs `--embed-mode neural`. |
| `xerj_vector_search` | kNN vector search over a `dense_vector` field (`index`, `field`, `query_vector`, `k`, optional `num_candidates`, `filter`). |
| `xerj_hybrid_search` | Combine multiple query clauses (`index`, `queries`, `fusion`: `"rrf"` or `"linear"`). |
| `xerj_memory_store` | Store a memory (`namespace`, `text`, optional `metadata`, `id`, `dedup`, `dedup_threshold`). |
| `xerj_memory_recall` | Recall memories (`namespace`, `query`, optional `semantic`, `hybrid`, `fusion`, `k`, `filter`, `recency_weight`). |
| `xerj_brain_overview` | Summarize entities, hubs, and communities in the deterministic link index (`prefix` optional). |
| `xerj_brain_ego` | 1-hop neighborhood around `node` in the link index. |
| `xerj_brain_link` | Assert a manual edge (`source`, `target`, optional `relation`, `note`) in the link index. |
| `xerj_brain_unlink` | Remove a manual edge (`source`, `target`, optional `relation`) from the link index. |

### MCP registration

```json
{
  "mcpServers": {
    "xerj": {
      "command": "/ABSOLUTE/PATH/TO/xerj",
      "args": ["mcp"],
      "env": { "XERJ_URL": "http://localhost:9200", "XERJ_AUTH": "ApiKey <key>" }
    }
  }
}
```

Full per-client commands (Claude Code, Codex, Gemini CLI, Cursor, Windsurf, VS Code, OpenCode): [https://xerj.org/llms-install.md](https://xerj.org/llms-install.md#6-register-the-mcp-server).

---

## 4. Reference coding before writing non-trivial systems code

Before writing unfamiliar engine, storage, parser, or protocol code from scratch, retrieve how a tested open-source peer solved it (`xerj code <corpus> "<query>"` or `xerj_code_search`).

1. **Retrieved code is evidence, not authority.** Understand the mechanism and adapt it; cite `repo@sha:path:line` in your commit or PR.
2. **Respect licences.** Permissive (`MIT`, `Apache-2.0`, `BSD`, `0BSD`, `Zlib`, `ISC`): adapt with attribution. Copyleft (`GPL`, `LGPL`, `AGPL`, `SSPL`, `MPL-2.0`, `BSL`): study the approach only; never copy code into an incompatible project.
3. **515 live corpora on the Corpus Hub** ([https://hub.xerj.org](https://hub.xerj.org) · [https://hub.xerj.org/llms.txt](https://hub.xerj.org/llms.txt) · [https://xerj.org/docs/corpus-hub](https://xerj.org/docs/corpus-hub)). If retrieval returns nothing relevant after 2 queries, stop and write the code normally.
