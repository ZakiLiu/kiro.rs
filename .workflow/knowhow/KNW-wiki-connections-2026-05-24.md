# Wiki Connections Report
**Generated:** 2026-05-24 | **Mode:** --fix

## Health Delta

| Metric | Before | After | Delta |
|--------|--------|-------|-------|
| Health Score | 93/100 | 100/100 | **+7** |
| Orphans | 10 | 0 | **-10** |
| Broken links | 0 | 0 | — |
| Hub nodes | 0 | 5 | **+5** |

## Applied Connections (8 updates, 1 skipped)

| # | Score | Source → Target | Reason | Status |
|---|-------|----------------|--------|--------|
| 1 | 0.90 | project-project → arch-constraints, coding-conventions | project↔spec bridge | ❌ FORBIDDEN (project.md read-only) |
| 2 | 0.85 | knowhow-decompose → arch-constraints, coding-conventions | knowhow↔spec bridge | ✅ Applied |
| 3 | 0.80 | knowhow-follow-provider → coding-conventions, decompose, arch | deep dive cross-ref | ✅ Applied |
| 4 | 0.70 | knowhow-digest → project-project | digest↔project bridge | ✅ Applied |
| 5 | 0.65 | coding-conventions → arch-constraints, decompose, follow | bidirectional | ✅ Applied |
| 6 | 0.65 | arch-constraints → coding-conventions, decompose, follow | bidirectional | ✅ Applied |
| 7 | 0.60 | test-conventions → coding-conventions | test references coding | ✅ Applied |
| 8 | 0.55 | quality-rules → coding-conventions | quality enforces coding | ✅ Applied |
| 9 | 0.50 | ui-conventions → coding-conventions | UI references coding | ✅ Applied |
| 10 | 0.50 | learnings → decompose, follow-provider | learnings source | ✅ Applied |

## New Hub Structure

```
spec:project:coding-conventions (in-degree: 6) ← 核心 hub
  ↑ test-conventions, quality-rules, ui-conventions, decompose, follow-provider, arch-constraints

knowhow-decompose-src-2026-05-24 (in-degree: 4)
  ↑ coding-conventions, arch-constraints, follow-provider, learnings

knowhow-follow-provider-2026-05-24 (in-degree: 3)
  ↑ coding-conventions, arch-constraints, learnings

spec:project:architecture-constraints (in-degree: 3)
  ↑ coding-conventions, decompose, follow-provider

project-project (in-degree: 1)
  ↑ digest
```

## Graph Structure Observations

1. **coding-conventions 成为自然 hub** — 所有其他 spec 和 knowhow 都引用它，符合其「编码规范总纲」定位
2. **knowhow 层成功桥接到 spec 层** — decompose 和 follow-provider 都双向链接到 arch + coding specs
3. **project-project 仍然相对孤立** — 因为 project.md 是只读的，无法添加 related 链接。只有 digest 指向它
4. **子条目通过 parent 链接自动连通** — 74 个子条目都通过 parent→root 链接参与图谱

## Recommendations

- project.md 的只读限制导致 project-project 无法被充分连接。如需改善，可在其他条目中添加指向它的 related 链接
- 未来新增 knowhow 条目时，应主动添加 related 链接到相关 spec，保持 type bridge 模式
