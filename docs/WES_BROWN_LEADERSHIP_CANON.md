# Wes Brown's Leadership Canon — Engineering & Agent Operating Principles in XERJ

This document defines how Wes Brown's engineering leadership practices are integrated into **XERJ** (`unsigned-gg/xerj`), its domain validation model (`lsc_core::leadership`), its retrieval/memory surfaces (`xerj autoindex`, `xerj code`, `/_memory/{ns}`, `/_graph/{brain}`), and multi-agent engineering workflows.

Canonical corpus records in Cygnus XERJ (`http://xerj.tail769bd2.ts.net:9200/xc-iopsych-corpus`):
- `iopsych-wes-brown-leadership-canon`
- `iopsych-wes-brown-neurodivergent-scaffolding`
- `iopsych-wes-brown-management-practices`

---

## 1. Primary Mandate: The Manager and Harness as Force Multiplier

The sole responsibility of an engineering lead or coordinator agent is **removing obstacles so technical contributors can do their best work**.

- **Measure outcomes, never process theater:** Evaluate engineering effectiveness by impactful work shipped, technical capability growth, and operational sustainability—never by sprint velocity points, Jira ticket transitions, or story-point burndowns.
- **Anti-Jira / Artifact-Derived Status:** Do not maintain parallel status state in a ticketing system. Generate upward status reports directly from shipped, verifiable artifacts: merged pull requests, commit bodies with before/after measurements, `lsc_core::ValidationReceipt` BLAKE3 proofs, and live node telemetry.

---

## 2. The Top 3 Engineering Principles

Teach and enforce three core engineering principles through code review, design discussions, and post-mortems (`lsc_core::TopThreePrinciplesEval`):

1. **Determine the actual root problem being solved that has the most impact.**
   - Do not patch surface symptoms or build speculative abstractions. Identify the root bottleneck or failure mechanism before writing code.
2. **Be cognizant of the cost of an approach or solution in terms of time, complexity, and maintenance burden.**
   - Every new abstraction, background task, configuration knob, or dependency imposes a permanent cognitive and operational tax.
3. **Determine the "good enough" point (the 80% solution to 80% of the problem) and recognize that perfection is the enemy of good.**
   - Ship the concrete, measured 80/20 fix that resolves the primary bottleneck today rather than stalling on a theoretical 100% architecture (`EngineeringPrincipleVerdict::ShipEightyTwenty` vs. `PerfectionismTrap`).

---

## 3. Neurodivergent Executive Scaffolding & Team Rhythms

High-capability engineers and autonomous agent lanes frequently exhibit high variance in executive function, hyperfocus, poor interoception, and sensitivity to ambiguity or political noise. Adapt the operating system to the contributors rather than forcing contributors into rigid process molds:

### 3.1 Daily 30-Minute Sync ("Coffee & Tea Time")
- **Randomized speaker order:** Avoid fixed round-robin order so participants stay engaged rather than rehearsing a script and checking out.
- **Bimodal communication:** Pair synchronous voice with an active text chat stream (mirrored in XERJ CLI by pairing human-readable `xerj-bar` display lines with structured `xerj-progress` records).
- **Immediate tactical use:** Use surfaced blockers immediately during the sync to reallocate resources, pair contributors, or drop low-priority scope on the spot.

### 3.2 Weekly Unstructured Architectural Sync ("Fireside Chat") & The "Never Recorded" Rule
- **60-90 minutes, no fixed agenda:** Dedicated to architectural exploration, conceptual teaching, and cross-pollinating ideas.
- **Never Recorded:** Synchronous exploratory meetings and Fireside Chats are **never recorded**. Eliminating permanent-record surveillance removes performance masking and allows engineers to ask basic questions and test raw hypotheses safely.

### 3.3 Public Channel by Default (`#ml-dev` <-> Shared XERJ Indices)
- **100% of technical work lives in the public channel:** Design debates, PR reviews, debugging traces, and decisions belong in shared, searchable surfaces (`#ml-dev`, `#reverie`, `xerj autoindex` corpora, `/_memory/{ns}`, and `/_graph/{brain}`) so the entire team benefits from passive transactive memory.
- **DMs are strictly for personal matters:** Reserve 1:1 direct messages exclusively for personal, health, or HR topics. Redirect any technical question asked in a DM back to the public channel.

---

## 4. Self-Driven Deadlines & Explicit Trade-Off Firewalling

- **"When can YOU do it?":** Never impose arbitrary top-down deadlines. Ask the engineer or builder lane for their own estimate (`xerj autoindex --dry-run` / `xerj_index_budget`), negotiate scope or dependencies to calibrate estimation skill, and hold them accountable to their own commitment through daily touchpoints.
- **Explicit Trade-Off Firewall:** Absorb organizational politics so contributors can focus on deep work (protecting 2-4 hour uninterrupted blocks). Never give a bare "no" upward; state explicit trade-offs: *"Yes, we can deliver X by Friday, which means pausing Y; choose which priority wins."*

---

## 5. Conflict & Stall Resolution: Debug the State, Not the Argument

When two engineers clash or an agent lane enters a retry spiral, treat the friction first as a **dysregulated physiological or runtime state** (`lsc_core::InteroceptiveStateEval`), not a technical disagreement:

- **Poor interoception & hyperfocus depletion:** Humans in hyperfocus routinely go 6+ hours without food, water, or a break; agent lanes hit context-window saturation or node RSS watermarks (`rss >= watermark`).
- **Mandatory timeout protocol:** When `continuous_minutes >= 240`, `resource_saturation_ratio >= 0.85`, or `consecutive_friction_turns >= 3` (`StateDiagnosis::InteroceptiveTimeoutRequired`), call an immediate timeout. Checkpoint state, step away or compact context, and resume technical debate only after state is regulated.

---

## 6. Individualized Communication (8 Archetypes) & The 5-6 Engineer Scale Ceiling

- **Adapt to the 8 communication archetypes (`lsc_core::CommunicationArchetype`):**
  1. `HyperverbalProcessor` (thinks out loud through dialogue)
  2. `WrittenProcessor` (needs written specs and reading time before responding)
  3. `DirectInstruction` (*"just tell me what to do"* during high-ambiguity crunch)
  4. `AutonomousExplorer` (*"let me figure it out"* given goal and constraints)
  5. `ValidationSeeker` (high capability + imposter syndrome; affirm sound direction first)
  6. `JustTheFacts` (<=15-line evidence-backed verdict, zero fluff)
  7. `BluntDirect` (unvarnished technical critique)
  8. `ContextSensitive` (frame critique around shared system goals)
- **Hard Scale Ceiling (`WES_BROWN_MAX_SPAN_OF_CONTROL = 6`):** Deep technical management breaks past **5-6 direct engineers** (and <=3-6 active subagent lanes per coordinator wave). Beyond 6, split the team or delegate a subdomain lead rather than diluting technical context.
