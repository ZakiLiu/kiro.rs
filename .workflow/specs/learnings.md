---
title: Learnings
readMode: optional
priority: medium
category: learning
keywords:
  - bug
  - lesson
  - gotcha
  - learning
related:
  - knowhow-decompose-src-2026-05-24
  - knowhow-follow-provider-2026-05-24
  - knowhow-periodic-recovery-2026-05-25
---



# Learnings

Bugs, gotchas, and lessons learned during development.
Add entries with: `/spec-add learning <description>`

## Entries

<spec-entry source="wiki-digest" category="technique" date="2026-05-24">
Wiki 初始化后知识图谱呈「全 spec 零经验」状态（29 条 spec，0 条 knowhow/note/issue）。结构性规范完备但缺乏实操沉淀。优先动作：从源码提取隐含决策（/learn-decompose）、修复 orphan 连接（/wiki-connect --fix）、日常开发随手记录 gotcha。
</spec-entry>

<spec-entry source="wiki-connect" category="technique" date="2026-05-24">
wiki-connect --fix 将健康分从 93 提升到 100/100，消除全部 10 个 orphan。关键策略：(1) knowhow↔spec type bridge 是最高价值连接（让经验层和规范层互通）；(2) coding-conventions 自然成为 hub（in-degree 6），符合其「总纲」定位；(3) project.md 只读限制导致 project-project 无法被充分连接，需通过其他条目的 related 间接引用。
</spec-entry>

<spec-entry source="learn-decompose" category="technique" date="2026-05-24">
src/ 全量模式分解：63 文件 → 40 raw patterns → 24 unique（6 已记录、5 跨维度合并）。核心发现：(1) 韧性是主导架构主题（retry/failover/circuit-breaker/cooldown/compression/decoder-recovery 全服务于「不丢请求」）；(2) 协议隔离严格（Anthropic 类型不泄漏到 Kiro 类型）；(3) 自适应优于固定值（TTL/压缩阈值/cooldown/采样率均动态调整）；(4) 字符串错误分类是已知技术债；(5) 三个 cooldown 变体已定义但未接线。
</spec-entry>

<spec-entry source="learn-follow" category="technique" date="2026-05-24">
provider.rs 韧性链路深度阅读：(1) 错误分类决定重试策略——400 bail、401/403 failover、402 disable+failover、429 cooldown、5xx retry、network retry-without-failover（Round 11 决议）；(2) client_cache 无驱逐策略是潜在内存泄漏；(3) MAX_TOTAL_RETRIES=3 硬上限在大凭据池场景可能不足；(4) Round 注释系统记录了迭代决策历史，是重要的考古线索；(5) is_rate_limit_response 是 Round 8 后的 dead code，保留但未接入。
</spec-entry>

<spec-entry source="wiki-digest" category="technique" date="2026-05-25">
二轮 digest（91 条，99/100 健康分）：知识图谱从「全 spec」演进到 spec:35 + knowhow:55 的双层结构，但仍缺 note 和 issue 类型。关键发现：(1) 前端和测试主题仅有规范零实操（health 70-75）；(2) 新增的周期性凭据恢复（start_periodic_recovery）和后台 Token 刷新（start_background_token_refresh）尚未入 wiki；(3) learnings 中 3 条技术债（cooldown 未接线、dead code、MAX_TOTAL_RETRIES 不足）应升级为 issue 追踪。
</spec-entry>

<spec-entry source="wiki-connect" category="technique" date="2026-05-25">
wiki-connect --fix 将健康分从 97 提升到 100/100，消除全部 3 个 orphan（127 条目）。关键策略：(1) 新增 knowhow-periodic-recovery 首日即成为 top hub（in-degree 5），证明高质量 knowhow 自然吸引连接；(2) digest 条目间建立时间线链（digest-2024→digest-2025），支持知识演进追溯；(3) spec:project:periodic-recovery-mechanism 是 maestro 自动镜像的 spec 副本，需手动补 related 才能消除 orphan。
</spec-entry>


<spec-entry category="learning" keywords="cooldown, suspended, 冷却, 凭据禁用, 大小写不敏感" date="2026-06-12" source="milestone-complete">

### 临时性上游信号应接冷却而非永久禁用；字符串信号匹配须大小写不敏感

上游 TEMPORARILY_SUSPENDED（临时暂停）原被按永久禁用处理（mark_account_suspended），需人工 Admin API 捞回。修复改接 CooldownReason::AccountSuspended 24h 冷却（report_account_suspended 包装：set_credential_cooldown_with_duration(id, reason, None) + affinity 解绑），到期 check_cooldown 返回 None 自动回池。关键认知：cooldown.rs 的 is_auto_recoverable()==false 仅决定时长计算走 long_cooldown_secs（24h），不阻止到期回池——语义勿望文生义。检测面：裸 body.contains("suspended") 大小写敏感漏判变体，统一为 is_suspended_signal（to_ascii_lowercase）；误伤风险因处置降为可自愈冷却而可接受。耦合警示：error_map 503 归类依赖 provider 错误文案「账户暂停」关键词，改文案须同步。custom_duration 传 None 让配置即真相，禁止硬编码 86400。
Milestone: M-adhoc-20260612-220748

</spec-entry>

<spec-entry category="learning" keywords="keepalive, 保活, 空闲探测, balance循环, 节流" date="2026-06-13" source="milestone-complete">

### 周期性副作用尽量挂既有循环；空闲探测必须三道闸防自噬

凭据保活探测（空闲 2h 强制 usage-limits 探测）的最小实现 = balance 刷新循环门控加一个 OR 条件——因为探测失败处置全长在 get_usage_limits_for 内部（invalid_grant→AuthenticationFailed 软永久、suspended→24h 冷却、网络错误仅传播），新增独立 tokio 任务只会复制 ticker 还要倒贴探测去重。三道闸缺一不可：① 冷却闸（all_enabled_credential_ids 不滤冷却，不查 check_cooldown 则 suspended 凭据每 tick 被续 24h 冷却成永动刑期，且 set_credential_cooldown_with_duration 会覆写 last_used_at）；② last_probed_at 内存节流（探测设计上不写 last_used_at 防自我续命，但过阈值后每 tick 重探，成败一律记防网络抖动风暴）；③ 配置下限钳制（MIN 600s，无下限则极小阈值令 idle 与节流同时失效，每 tick 全量轰炸上游）。「不可用永久禁用」是伪需求：池健康在 disabled 过滤瞬间达成，AuthenticationFailed 软永久（真死=recovery 全败=事实永禁，误判 5min 自愈）严格优于硬永久与 n_strikes。测试陷阱：Instant 零点为系统启动时刻，checked_sub 大时长在低 uptime 机器下溢 panic，必须兜底。
Milestone: M-adhoc-20260613-001800

</spec-entry>

<spec-entry category="learning" keywords="超额, 复活, quota, 恢复条件, 探测信号" date="2026-06-13" source="milestone-complete">

### 可用性信号与余额数字脱钩：探测成功即复活，402 才是真不可用

额度禁用凭据（QuotaExceeded/InsufficientBalance）的复活条件从『余额恢复 remaining>=1.0』改为『探测成功即复活』（recovery 循环 Ok 分支拍平：recover_credential_inner + update_balance_cache，零/负余额如实写入）。依据：上游允许超额使用，get_usage_limits_for 成功 = token 链路 + 上游 API 双重存活证明；余额仅供 LB 评分排序（select_best_candidate_id max-balance retain）与 TTL 参考，不做可用性过滤。402 信号链完整保留（report_quota_exhausted 再禁用），flapping 预期场景不发生（超额调用会成功），最坏 >=5min/轮且 failover 兜底。关键架构事实：禁用凭据对 balance/keepalive 循环不可见（all_enabled_credential_ids 滤 !disabled），recovery 循环是禁用凭据唯一探测口。测试陷阱：裸 KiroCredentials::default() 过不了 acquire_context（access_token=None → try_ensure_token 因缺 refreshToken 快速失败），acquire 成功路径测试必须显式 access_token + 未来 expires_at。
Milestone: M-adhoc-20260613-012100

</spec-entry>
