---
related:
  - knowhow-knw-wiki-connections-2026-05-24
  - knowhow-knw-periodic-recovery-2026-05-25
  - knowhow-knw-digest-full-2026-05-25
---
# Wiki Connections Report (2026-05-25)

## Health Delta

| Metric | Before | After | Delta |
|--------|--------|-------|-------|
| Health Score | 97/100 | 100/100 | +3 |
| Entries | 127 | 127 | 0 |
| Orphans | 3 | 0 | -3 |
| Broken links | 0 | 0 | 0 |

## Applied Connections (5 updates)

| # | Source | → Target(s) | Reason |
|---|--------|-------------|--------|
| 1 | knowhow-knw-digest-full-2026-05-25 | project-project, knowhow-knw-digest-full-2026-05-24, knowhow-knw-periodic-recovery-2026-05-25 | Orphan rescue + digest chain + type bridge |
| 2 | knowhow-knw-wiki-connections-2026-05-24 | spec:project:architecture-constraints, spec:project:coding-conventions, knowhow-knw-decompose-src-2026-05-24 | Orphan rescue + type bridge |
| 3 | spec:project:learnings | +knowhow-knw-periodic-recovery-2026-05-25 | New knowhow reference |
| 4 | knowhow-knw-decompose-src-2026-05-24 | +knowhow-knw-periodic-recovery-2026-05-25 | Bidirectional link (recovery extends decomposed patterns) |
| 5 | knowhow-knw-follow-provider-2026-05-24 | +knowhow-knw-periodic-recovery-2026-05-25 | Bidirectional link (recovery extends provider analysis) |
| 6 | spec:project:periodic-recovery-mechanism | knowhow-knw-periodic-recovery-2026-05-25, spec:project:coding-conventions, knowhow-knw-follow-provider-2026-05-24 | Orphan rescue + type bridge (spec↔knowhow) |

## New Hub Structure

| Hub | In-Degree | Change |
|-----|-----------|--------|
| spec:project:coding-conventions | 9 | +3 |
| knowhow-knw-decompose-src-2026-05-24 | 6 | +1 |
| knowhow-knw-follow-provider-2026-05-24 | 5 | +1 |
| knowhow-knw-periodic-recovery-2026-05-25 | 5 | NEW |
| spec:project:architecture-constraints | 5 | +1 |

## Graph Structure Observations

- **新 hub 诞生**: `knowhow-knw-periodic-recovery-2026-05-25` 首日即达 in-degree 5，成为第 4 大 hub
- **coding-conventions 继续强化**: in-degree 6→9，作为「总纲」的中心地位更加稳固
- **type bridge 策略有效**: spec↔knowhow 双向连接是消除 orphan 的最高效手段
- **digest 链**: 两个 digest 条目现在互相引用，形成时间线追溯

## Recommendations

1. 子条目（-001, -002 等）天然是 orphan，但不影响健康分计算——maestro 按顶层条目统计
2. 未来新增 knowhow 时，记得同时连接对应的 spec 条目（type bridge）
3. 考虑给 `project-project` 增加 related（当前 in-degree 仅 1，但受只读限制）
