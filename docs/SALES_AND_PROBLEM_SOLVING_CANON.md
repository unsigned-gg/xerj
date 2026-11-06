# The 7-Principle Sales & Problem-Solving Canon in XERJ

This document defines how the **7-Principle Sales & Problem-Solving Canon** is integrated into **XERJ** (`unsigned-gg/xerj`), its pure domain validation model (`lsc_core::leadership`), its retrieval/memory surfaces (`xc-iopsych-corpus`, `/_memory/iopsych`), and how XERJ positions and evaluates commercial and technical value.

Canonical corpus records in Cygnus XERJ (`http://xerj.tail769bd2.ts.net:9200`):
- Corpus document: `xc-iopsych-corpus` -> `iopsych-sales-problem-solving-canon`
- Durable memory: `/_memory/iopsych` -> `sales-problem-solving-canon-v1`

---

## 1. The 7 Canonical Principles

1. **Walk Up and Hit the Ball (Direct Execution Over Complexity):**
   - *"Sales is a lot like golf. You can make it so complicated as to be impossible or you can simply walk up and hit the ball."*
   - In XERJ: Do not force users through schema ceremonies, cluster provisioning, or ingestion pipeline code before they get value. `curl -fsSL https://xerj.org/get | sh` followed by `xerj autoindex <folder>` walks up and hits the ball.
2. **People and Problem Solving Over Lines of Code:**
   - *"Sales is about people and it's about problem solving. It is not about solutions or technology or chemicals or lines of code or artichokes."*
   - In XERJ: Never pitch BM25 block-max WAND, Tantivy segment internals, or Rust line counts as the headline. Lead with the human problem solved: finding exact ground-truth answers across messy folders and repositories without writing an ingestion pipeline.
3. **The 4 Buyer Drivers (`BuyerDriver`):**
   - *"People buy 4 things and 4 things only. Ever. Those 4 things are time, money, sex, and approval/peace of mind. If you try selling something other than those 4 things you will fail."*
   - In XERJ:
     - **Time (`BuyerDriver::Time`):** Zero ingestion pipeline authoring; `1.5x–2.7x` fewer agent output tokens and turns than grep loops.
     - **Money (`BuyerDriver::Money`):** Single static binary operating inside a deterministic memory cap (`XERJ_MAX_PROCESS_MEMORY_MB`), replacing multi-node JVM clusters.
     - **Approval / Peace of Mind (`BuyerDriver::ApprovalPeaceOfMind`):** Every search hit carries a verifiable `ax_path:match_line` citation, honest `llms.txt` caveats, and explicit circuit-breaker refusal rather than silent corruption.
     - **Status / Prestige (`BuyerDriver::Sex`):** Shipping faster, cleaner, citation-backed engineering work that stands up to adversarial review.
4. **Sell Aspirin, Never Vitamins (`PainClass::Aspirin` vs. `PainClass::Vitamin`):**
   - *"People buy aspirin always. They buy vitamins only occasionally and at unpredictable times. Sell aspirin."*
   - In XERJ: Acute bleeding pain (`Aspirin`) is an agent burning hundreds of dollars grepping blind, a search node OOM-killing in production, or a team unable to query a 30 GB compliance dump today. Nice-to-have cosmetic toggles (`Vitamin`) never preempt acute pain relief.
5. **Make Everything Else Equal, Then Go Make a Lot of Friends:**
   - *"All things being equal people buy from their friends. So make everything else equal then go make a lot of friends."*
   - In XERJ: "Make everything else equal" means closed-loop, cache-off, correctness-checked benchmarks (`demo/playbooks/`), zero-I/O domain invariants (`lsc-core`), and rock-solid wire compatibility. Once technical parity/superiority is unimpeachable, human trust and genuine friendship win adoption.
6. **Unconditional Value-First Generosity:**
   - *"Being valuable and useful is all you ever need to do to sell things... Introduce people who would benefit from knowing each other then get out of the way, expecting nothing in return."*
   - In XERJ: Ship reproducible playbooks, honest capability disclosures (`llms.txt`), free Corpus Hub packs (`xerj ask`), and uncompensated help without transactional gating.
7. **Zero Internal-Metric Leakage:**
   - *"No one cares about your quota, your payroll, your opex, your burn rate, etc. No one. They care about the problem you are solving for them."*
   - In XERJ: Never justify product limitations, pricing, or roadmaps by internal burn rate, infrastructure opex, or engineering effort.

---

## 2. Domain Model (`lsc_core::leadership::SalesCanonEval`)

`engine/crates/lsc-core/src/leadership.rs` provides pure, zero-I/O evaluators for auditing any product surface, GTM pitch, or roadmap proposal against all 7 principles:

- `BuyerDriver`: `Time`, `Money`, `Sex`, `ApprovalPeaceOfMind`
- `PainClass`: `Aspirin` (acute, always bought) vs. `Vitamin` (occasional, unpredictable)
- `SalesCanonVerdict`:
  - `HitTheBallAspirin` — All 7 principles satisfied: direct execution, human problem focus, anchored in the 4 buyer drivers, solves acute Aspirin pain, technical parity + relationship warmth, value-first generosity, and zero internal-metric leakage.
  - `InternalMetricLeakage` — Violates Principle 7 (exposes quota, payroll, opex, or burn rate).
  - `TechJargonOverProblem` — Violates Principle 2 (leads with lines of code or tech jargon instead of the human problem).
  - `MissingBuyerDriver` — Violates Principle 3 (fails to anchor in Time, Money, Sex, or Approval/Peace of Mind).
  - `VitaminPitch` — Violates Principle 4 (pitches an optional Vitamin instead of acute Aspirin pain relief).
  - `OvercomplicatedFunnel` — Violates Principle 1 (overcomplicates execution instead of walking up and hitting the ball).
  - `TransactionalOrLowParity` — Violates Principle 5 or 6 (insufficient product parity or transactional gating instead of value-first generosity).
