---
title: Debug Notes
readMode: optional
priority: medium
category: debug
keywords:
  - debug
  - issue
  - workaround
  - root-cause
  - gotcha
related:
  - "spec:project:coding-conventions"
  - "spec:project:quality-rules-003"
---


# Debug Notes

## Entries

<spec-entry category="debug" keywords="429,backoff,thundering herd,重试,凭据轮转" date="2026-06-23" source="harvest:analyze-kiro-429-500-root-cause">

### 429 路径缺少 backoff 导致 Thundering Herd

凭据轮转重试循环中 429 处理路径用 `continue` 直接跳到下一个凭据，未调用 `sleep(retry_delay())`（与 5xx 路径不同）。后果：上游限流时所有凭据被无延迟连续轮转，形成 thundering herd 雪崩。修复：在 MCP 和 streaming 两处 429 分支的 `continue` 前插入 `sleep(Self::retry_delay(attempt)).await`，与 5xx 处理保持一致。参考来源：.workflow/scratch/20260609-analyze-kiro-429-500-root-cause/。

</spec-entry>

<spec-entry category="debug" keywords="500,do request failed,历史代码,sub2api,来源排查" date="2026-06-23" source="harvest:analyze-kiro-429-500-root-cause">

### "do request failed" 500 非 kiro.rs 来源

排查发现 "do request failed" 500 错误消息在 kiro.rs 任何源码中都搜不到，判断为历史遗留或其他项目（疑似 sub2api）的错误，非当前代码库问题。真正的上游 500（MODEL_TEMPORARILY_UNAVAILABLE）已正确重试并故障转移。参考来源：.workflow/scratch/20260609-analyze-kiro-429-500-root-cause/。

</spec-entry>

<spec-entry category="debug" keywords="wiki,knowhow,id,knw,断链,related,frontmatter,maestro" date="2026-06-23" source="harvest:wiki-connect-fix">

### maestro wiki knowhow ID 含 knw- 前缀，related 引用漏写导致全库断链

maestro wiki 给 knowhow 文件（`KNW-{slug}.md`）生成的 ID 是 `knowhow-knw-{slug}`——文件名 `KNW-` 前缀小写化后被带进 slug，而非直觉的 `knowhow-{slug}`。手写 frontmatter `related` 或 `/spec-add` 时若按逻辑名写 `knowhow-{slug}`，会因 ID 不匹配产生 broken link。

实测影响：2026-06-23 connect 排查发现全库 41 处 knowhow 互引全部漏写 `knw-` 前缀，造成 24 条 broken link、健康分跌至 45。修复后回到 95。

**规则**：引用 knowhow 条目必须用完整 ID `knowhow-knw-{slug}`（全小写，含 `knw-` 前缀）。不确定时先 `maestro wiki list --type knowhow` 核对实际 ID。

**ID 速查**：
- spec 文件 → `spec:project:{filename}`（无前缀问题）
- knowhow 文件 → `knowhow-knw-{slug}`（`KNW-` 文件名小写，**含 knw-**）
- issue → wiki **不索引** issue，related 里写 `ISS-*` 必断链，改在正文提及

</spec-entry>

