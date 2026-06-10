# Review Report — REV-001

- **Review date**: 2026-06-06
- **Source**: `scan/scan-results.json` (SCAN-001, 91 files / 32,991 lines)
- **Verification**: All findings independently traced to source by the reviewer (CLI delegate `codex` was broken in env). **3 scan descriptions corrected.**

## Executive Summary

10 findings: **2 HIGH, 3 MEDIUM, 5 LOW**. Codebase quality is high overall (clippy clean except cosmetic warnings; extensively tested provider/compressor). Two HIGH findings are genuine, exploitable/impactful, and **both fix-low**:

- **SEC-001** — empty proxy `apiKey` ⇒ full auth bypass on `/v1/messages`. Confirmed, and the bypass works via **both** `x-api-key` and `Authorization: Bearer` (scan only noted Bearer).
- **COR-001** — EventStream `DecodeIter` halts forever after the first recoverable frame error, silently dropping all later valid frames.

Recommended fix scope: **SEC-001, COR-001, COR-002, PRF-001** (all minimal/low). Opportunistic: **MNT-001/002/003**. **Skip/document**: COR-003 (intentional design), SEC-002, SEC-003.

## Metrics — Dimension × Severity

| Dimension | HIGH | MEDIUM | LOW | Total |
|-----------|:----:|:------:|:---:|:-----:|
| Security | 1 | 0 | 2 | 3 |
| Correctness | 1 | 2 | 0 | 3 |
| Performance | 0 | 1 | 0 | 1 |
| Maintainability | 0 | 0 | 3 | 3 |
| **Total** | **2** | **3** | **5** | **10** |

- Fixable: 8 · Auto-fixable: 2 (MNT-002/003) · Skip-recommended: 3 (COR-003, SEC-002, SEC-003)

## HIGH / MEDIUM Findings

| ID | Sev | File:Line | Title | Verified | Fix | Notes |
|----|-----|-----------|-------|:--------:|-----|-------|
| SEC-001 | HIGH | main.rs:128 | Empty proxy api_key ⇒ auth bypass | ✅ | minimal/low | Also via empty `x-api-key`. Admin & kiro keys ARE guarded — proxy is the lone gap. |
| COR-001 | HIGH | decoder.rs:393 | DecodeIter halts on first recoverable error | ✅ | minimal/low | `Recovering` treated as terminal; recovery is dead in iterator path. |
| COR-002 | MED | compressor.rs:259 | short-input branch lacks hard-cap | ✅ | minimal/low | Overflow is bounded = marker length (~25-30 ch), not unbounded. |
| COR-003 | MED | schema.rs:18 | normalize non-recursive | ✅ | **skip** | **Intentional** (Round 6, kiro-cli wire alignment). Document, don't fix. |
| PRF-001 | MED | compressor.rs:460 | history char-count loop O(n²) | ✅ | minimal/low | Re-sums whole history each removal. |

## Scan Corrections (reviewer)

1. **SEC-001** — bypass works via empty `x-api-key` header too, not only `Authorization: Bearer `. `extract_api_key` (auth.rs:16-22) returns `Some("")` for an empty header (no empty filter).
2. **SEC-002** — `subtle` 2.6.1 `ct_eq` does **not** "short-circuit early-return". It is constant-time over content; the length-equality check is plain, so only **length** leaks. Direction right, mechanism phrasing wrong.
3. **COR-003** — not a bug. File header documents the deliberate non-recursion (Round 6, 2026-05-13) for kiro-cli 2.3.0 byte alignment + prefix-cache stability. Recommend **skip + comment + monitor**, not a code fix.

## Critical Files

- **`src/anthropic/compressor.rs`** (2 findings: COR-002, PRF-001) — both in the synchronous compression hot path; fix together.
- **`src/main.rs`** (SEC-001 startup half; auth half in middleware.rs / auth.rs).

## Root-Cause Groups

- **Empty-string auth collapse** (primary SEC-001; +SEC-002) — empty/short keys vs `constant_time_eq`. **Hunt result: no other empty-string auth comparison in `src/`.** Asymmetry is isolated; admin (main.rs:254/260) and kiro (main.rs:65) keys already guard `!trim().is_empty()` — only the proxy api_key (main.rs:128) lacks it.
- **Compressor budget/perf** (primary PRF-001; +COR-002) — co-located, both fire on large payloads.

## Optimization Suggestions (priority-ordered)

| # | For | Action | Priority |
|---|-----|--------|:--------:|
| OPT-1 | SEC-001 | Two-layer: startup `trim().is_empty()` exit + middleware reject empty key. Add 401-on-empty test. | P0 |
| OPT-2 | COR-001 | `Recovering → Ready` in `decode()`; add `[valid][corrupt][valid]` regression test. | P0 |
| OPT-3 | COR-002 | Extract shared post-format hard-cap, apply to short-input branch. | P1 |
| OPT-4 | PRF-001 | Compute `total_chars` once, subtract per removal. O(n²)→O(n). | P1 |
| OPT-5 | MNT-001 | Reuse CJK-aware estimator for WebSearch `output_tokens` (websearch.rs:582, :747). | P2 |
| OPT-6 | MNT-002/003 | Batch `cargo clippy --fix`. | P3 |
| OPT-7 | COR-003 | Inline "intentional" comment + debug log on nested schema. No behavior change. | P3 |

## Recommended Fix Scope for FIX-001

**Must fix (P0–P1):** SEC-001, COR-001, COR-002, PRF-001 — all minimal/low complexity, no inter-dependencies.
**Opportunistic (P2–P3):** MNT-001, MNT-002, MNT-003.
**Do not code-change:** COR-003 (document only), SEC-002, SEC-003 (documented low-risk).
