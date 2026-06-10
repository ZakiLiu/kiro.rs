# Scan Issues — RV-fullscan-2026-06-06

## High severity
- **SEC-001** Empty proxy api_key -> full auth bypass. main.rs:128 only guards None, not "". Admin path HAS the guard (main.rs:259-261); the public proxy path does NOT. Asymmetry is the smell.
- **COR-001** EventStream DecodeIter stops forever on first recoverable error (decoder.rs:393 returns None on Recovering state). Fault-tolerant recovery is dead in iterator path.

## Medium
- **COR-002** compressor smart_truncate_by_lines short-input branch (line 259) missing the hard-cap fallback that the main branch (line 288) has.
- **COR-003** normalize_json_schema (schema.rs) is non-recursive — nested MCP schemas not normalized. Possibly intentional (kiro-cli alignment) — needs reviewer call.
- **PRF-001** compress_history_pass char loop O(n^2) (compressor.rs:460), re-sums whole history each removal.

## Low
- SEC-002 constant_time_eq leaks key length (subtle ct_eq short-circuits on length mismatch).
- SEC-003 wide-open CORS on a server that also hosts Admin API (header-auth limits CSRF).
- MNT-001 clippy unused_imports x10 (test modules). MNT-003 len_zero + bool_assert_comparison.
- MNT-002 WebSearch output_tokens = byte_len/4, ~3x off for CJK; CJK-aware estimator exists at provider.rs:1313.

## Scan notes / hard-won context
- Codebase quality is HIGH: clippy clean (0 errors, 12 cosmetic warnings), provider.rs retry/failover logic extensively tested and documented across multiple fix rounds, compressor has exhaustive tool-pairing tests.
- The recurring threat pattern here is the EMPTY-STRING edge case: empty api_key (SEC-001) and empty Bearer both collapse to constant_time_eq("","")==true. Reviewer/fixer should hunt for other empty-string auth comparisons.
- codex delegate is broken in this env (reasoning effort config set to invalid `xhigh`). Solo manual scan used instead.
