# Reviewer Decisions — RV-fullscan-2026-06-06

## Fix scope verdicts (REV-001)
- **Must fix (P0-P1):** SEC-001, COR-001, COR-002, PRF-001 — all minimal/low complexity, no inter-dependencies.
- **Opportunistic (P2-P3):** MNT-001 (CJK token est), MNT-002/003 (clippy auto-fix).
- **Skip / document only:**
  - **COR-003** — non-recursive normalize_json_schema is INTENTIONAL (schema.rs:9-17 Round 6, 2026-05-13: kiro-cli 2.3.0 wire alignment, prefix-cache stability). No observed nested-malformed 400. Add inline comment + debug log; do NOT recurse.
  - **SEC-002** — length leak via subtle ct_eq; documented low. Fix only if hardening (sha256-to-fixed-width).
  - **SEC-003** — wide CORS intentional for public proxy; header auth (no cookies) limits CSRF. Confirm Admin stays header-auth.

## Scan corrections issued
1. SEC-001 bypass also via empty `x-api-key` header (not only Bearer). extract_api_key returns Some("") for empty header.
2. SEC-002 — subtle 2.6.1 ct_eq does NOT short-circuit; constant-time over content, only length leaks (plain len check).
3. COR-003 — intentional design, not a bug.
