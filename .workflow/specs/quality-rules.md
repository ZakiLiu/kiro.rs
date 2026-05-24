---
title: Quality Rules
readMode: required
priority: medium
category: review
keywords:
  - quality
  - lint
  - rule
  - enforcement
related:
  - "spec:project:coding-conventions"
---


# Quality Rules

## Tools
- Formatter: `cargo fmt` (default rustfmt settings, no custom config)
- Linter: `cargo clippy`
- Build check: `make check` (fmt + clippy + test)
- No CI pipeline in repo (Docker Hub automated build only)

## Build Order
- Frontend (`cd admin-ui && pnpm build`) MUST complete before backend (`cargo build`)
- `make release` handles correct ordering automatically

## Feature Flags
- `sensitive-logs`: opt-in at compile time, outputs token usage diagnostics (never enable in production)

## Entries

