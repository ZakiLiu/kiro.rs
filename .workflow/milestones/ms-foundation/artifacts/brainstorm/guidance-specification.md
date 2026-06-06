# Guidance Specification: kiro.rs vs kiro-rs-dev-source Codebase Comparison

## §1. Project Positioning & Goals

**Context**: 对比两个 Anthropic Claude API 兼容代理服务实现（kiro.rs v1.1.31 vs kiro-rs-dev-source v2026.3.1），分析差异并制定融合策略。

**Primary Goal**: 识别两个项目的技术优劣势，确定从 dev-source 引入当前项目的最优路径，保留当前项目的核心竞争优势同时补齐短板。

**Success Criteria**:
- 每个分析角色产出可操作的建议
- 功能引入计划有明确优先级和实施路径
- 技术决策有充分的对比依据支撑

## §2. Concepts & Terminology

| 术语 | 定义 | 别名 | 类别 |
|------|------|------|------|
| Prefix Cache | 请求前缀缓存，复用已计算的 token 以减少重复处理成本 | Prompt Cache | technical |
| Prompt Preset | 预设系统提示模板，支持运行时通过 Admin UI 切换行为模式 | System Prompt Library | business |
| Input Compression | 多层压缩管道（空白→thinking→tool_result→tool_input→历史截断），处理超过上游 5MB 限制的请求 | Compression Pipeline | technical |
| Anti-Detection | 指纹生成、亲和性绑定、速率限制等机制，模拟真实用户行为以避免上游封禁 | Stealth Mechanisms | technical |
| Credential Failover | 凭据故障转移机制，自动切换到可用凭据以保证服务连续性 | Token Failover | core |
| Event Stream | AWS Event Stream 二进制协议，Kiro 上游使用的响应格式（header + payload + CRC32C） | AWS Event Stream | core |
| Protocol Converter | Anthropic ↔ Kiro 双向协议转换层，包括模型映射、Schema 规范化、工具占位符生成 | — | core |
| Prompt Filter | 运行时剥离系统提示中的限制性安全指令（14+ 模式） | Restriction Stripping | business |
| Web Portal | app.kiro.dev 的 CBOR-over-HTTP API 接口，用于查询用户用量和余额 | Kiro Web Portal | technical |
| CredentialIdentity | 共享 trait 抽象，统一 fingerprint 在 anti-detection 和 cache key 两个场景的使用 | — | technical |

## §3. Non-Goals (Out of Scope)

| 排除项 | 理由 |
|--------|------|
| 部署流程对比 | Docker/CI/CD 差异不在核心分析范围，两者部署模式相似 |
| 代码风格/格式差异 | 命名规范、缩进等表面差异不影响架构决策 |
| 历史演进路径 | 不追溯两个项目的 git 历史和分叉点，聚焦当前状态对比 |

## §4. System Architect Decisions

| ID | Decision | Choice | RFC 2119 | Source |
|----|----------|--------|----------|--------|
| SA-01 | 模块化策略 | SHOULD 逐步拆分，优先拆最大的模块（converter.rs, stream.rs, token_manager.rs） | SHOULD | user |
| SA-02 | 跨请求缓存优先级 | MUST 实现跨请求 conversation_id 复用，基于现有 fingerprint 体系 | MUST | user |
| SA-03 | 错误处理统一 | MUST 引入统一 error_map 模块，参考 dev-source 设计重新实现 | MUST | user |
| SA-04 | 缓存与 anti-detection 依赖关系 | MUST 抽取 CredentialIdentity 共享 trait，cache 和 affinity 各自实现 | MUST | user |
| SA-05 | 凭据管理架构 | MUST 保持当前拆分方式（token_manager + cooldown + rate_limiter 分离） | MUST | user |
| SA-06 | 当前压缩管道 | MUST NOT 移除输入压缩能力，这是 dev-source 不具备的核心优势 | MUST NOT | user |
| SA-11 | CLI 端点保留 | MUST 保留 kiro/endpoint/cli.rs — dev-source 缺失此模块，它模拟 kiro-cli 2.3.0 请求签名，是 anti-detection 关键组件 | MUST | user |

## §5. Product Manager Decisions

| ID | Decision | Choice | RFC 2119 | Source |
|----|----------|--------|----------|--------|
| PM-01 | 功能引入范围 | MUST 引入 Metrics、Cross-request Cache、Error Mapping、PDF 支持 | MUST | user |
| PM-02 | 核心优势保留 | MUST NOT 弱化压缩管道、Anti-Detection、图片处理、Web Portal 集成 | MUST NOT | user |
| PM-03 | 产品定位 | SHOULD 兼顾可靠工具与专业平台两种定位 | SHOULD | user |
| PM-04 | 引入优先级 | MUST P0: Cache + Metrics → P1: Error Map + Converter → P2: PDF + Prompt | MUST | user |
| PM-05 | Prompt 策略 | SHOULD 部分引入（presets 有价值，filter 需要独立评估安全影响） | SHOULD | user |

## §6. Subject Matter Expert Decisions

| ID | Decision | Choice | RFC 2119 | Source |
|----|----------|--------|----------|--------|
| SME-01 | 缓存融合策略 | MUST 复用现有 fingerprint 作为 cache key，通过 CredentialIdentity trait 统一 | MUST | user |
| SME-02 | 凭据管理组织 | MUST 保持当前拆分方式，职责分离更清晰 | MUST | user |
| SME-03 | 转换层增强 | MUST 引入 tool name shortening 和 forced conversation_id 两个特性 | MUST | user |
| SME-04 | Anti-Detection 评价 | 当前 anti-detection 机制效果很好，是核心竞争力，MUST 保留并强化 | MUST | user |

## §7. Test Strategist Decisions

| ID | Decision | Choice | RFC 2119 | Source |
|----|----------|--------|----------|--------|
| TS-01 | 测试策略 | MUST 关键路径必须有测试覆盖（协议转换、缓存、错误映射） | MUST | user |
| TS-02 | 引入方式 | SHOULD 参考 dev-source 设计重新实现，不直接移植代码 | SHOULD | user |

## §8. Cross-Role Integration

### Anti-Detection × Cache
- Anti-Detection 的 fingerprint 体系 MUST 通过 CredentialIdentity trait 共享给 Cache 模块
- Cache 的 conversation_id 复用 MUST NOT 影响 anti-detection 的指纹多样性
- 两个系统 SHOULD 独立演进，通过共享 trait 解耦

### Compression × Error Mapping
- 输入压缩后如果仍触发上游错误，error_map SHOULD 能识别压缩相关的错误类型
- Error mapping MUST 在压缩管道之后运行（处理压缩后的响应错误）

### Metrics × All Features
- Metrics 系统 MUST 覆盖所有关键路径：请求延迟、TTFB、缓存命中率、凭据分布、错误分类
- 新引入的功能（Cache、Error Map、PDF）MUST 与 Metrics 集成

## §9. Risks & Constraints

| 风险 | 影响 | 缓解 |
|------|------|------|
| 重新实现工作量大（6+ 模块） | 开发周期长，可能引入新 bug | 分 P0/P1/P2 三批推进，每批独立验证 |
| fingerprint 复用可能泄露关联性 | cache 和 anti-detection 共用标识可能被上游关联 | trait 设计允许为 cache 生成独立衍生标识 |
| 模块拆分可能破坏现有行为 | 回归风险 | 拆分前补齐关键路径测试 |
| 跨请求缓存的内存消耗 | LRU 缓存占用内存 | 设置合理的 max_entries 上限和 TTL |

## §10. Feature Decomposition

| ID | Slug | 标题 | 描述 | 优先级 | 相关角色 |
|----|------|------|------|--------|----------|
| F-001 | cross-request-cache | 跨请求前缀缓存 | 参考 dev-source 的 prompt_cache 设计，实现 (credential, fingerprint) → conversation_id 映射，LRU 淘汰，TTL 分级（5m/1h），复用现有 fingerprint 体系 | MUST (P0) | SA, SME |
| F-002 | request-metrics | 请求指标与可观测性 | 实现 ring buffer 请求指标收集（延迟、TTFB、凭据分布、错误分类），Admin API 聚合查询，时间窗口统计 | MUST (P0) | SA, TS |
| F-003 | error-mapping | 统一错误映射 | 将上游 Kiro 错误统一映射为 Anthropic 错误格式，注入 Retry-After 头，区分可重试/不可重试错误 | MUST (P1) | SA, SME |
| F-004 | converter-enhance | 转换层增强 | 引入 tool name shortening（超长工具名自动缩写+映射表）和 forced_conversation_id（强制会话复用以提升 cache 命中） | MUST (P1) | SME |
| F-005 | prompt-presets | 系统提示预设 | 实现可配置的系统提示预设库，支持 Admin UI 切换，filter 功能独立评估后决定是否引入 | SHOULD (P2) | PM, SME |
| F-006 | pdf-support | PDF 文档支持 | 支持 document 块中的 PDF base64 解码和文本提取，参考 dev-source 的 lopdf 实现 | SHOULD (P2) | PM |
| F-007 | module-refactor | 核心模块拆分 | 逐步将 converter.rs / stream.rs 拆分为子模块（参考 dev-source 的拆分维度但保持当前项目风格） | SHOULD (跨版本) | SA, TS |
| F-008 | shared-identity | CredentialIdentity 共享 trait | 抽取 CredentialIdentity trait 统一 fingerprint 在 affinity 和 cache 中的使用，为 F-001 的前置依赖 | MUST (P0) | SA, SME |

## §11. Appendix: Decision Tracking

| # | Decision | Choice | Source |
|---|----------|--------|--------|
| 1 | Anti-Detection 效果评估 | 核心竞争力，效果很好，保留强化 | user |
| 2 | Prompt 策略引入范围 | 部分引入（presets 有价值，filter 待评估） | user |
| 3 | 可观测性重要性 | 很重要，是当前项目短板 | user |
| 4 | 模块化策略 | 逐步拆分，优先最大模块 | user |
| 5 | 跨请求缓存优先级 | 高优先级，可显著降本 | user |
| 6 | 错误处理统一 | 很需要，参考 dev-source 重新实现 | user |
| 7 | 引入功能范围 | Metrics + Cache + Error Map + PDF（全部引入） | user |
| 8 | 核心优势保留 | 压缩 + Anti-Detection + 图片 + Web Portal（全部保留） | user |
| 9 | 产品定位 | 两者兼顾（可靠工具 + 专业平台） | user |
| 10 | 缓存融合策略 | 复用现有 fingerprint 作为 cache key，抽取共享 trait | user |
| 11 | 凭据管理架构 | 当前拆分方式更好（职责分离清晰） | user |
| 12 | 转换层特性 | tool name shortening + forced conversation_id 都引入 | user |
| 13 | 测试策略 | 关键路径必须有测试覆盖 | user |
| 14 | 引入方式 | 参考设计，重新实现 | user |
| 15 | 功能优先级排序 | P0: Cache + Metrics → P1: Error Map + Converter → P2: PDF + Prompt | user |
| 16 | fingerprint 复用架构 | 抽取 CredentialIdentity 共享 trait，cache 和 affinity 各自实现 | user |

## §12. Cross-Role Resolutions

### Cross-Role Resolutions (added 2026-06-05)

| ID | Type | Source(s) | Resolution | Applied to |
|---|---|---|---|---|
| C-001 | conflict | system-architect/analysis-F-008-shared-identity.md "## Interface Contract" / subject-matter-expert/analysis-F-008-shared-identity.md "## Architecture" | Adopt SME's three-method signature (detection_identity, cache_identity, credential_id) for proper domain separation | SA file: strikeout original two-method trait; SME file: annotated as agreed contract |
| C-002 | conflict | system-architect/analysis-F-003-error-mapping.md "## Interface Contract" / subject-matter-expert/analysis-F-003-error-mapping.md "## Interface Contract" / test-strategist/analysis-F-003-error-mapping.md "## Interface Contract" | SA's dual-function pattern (classify + to_anthropic_response) + RequestContext struct carrying was_compressed (SME) and upstream_headers (TS) | All three files: strikeout original signatures |
| C-003 | conflict | system-architect/analysis.md "### Interfaces" / subject-matter-expert/analysis.md "### Interfaces" | ErrorMapper consumers = union: handlers.rs + stream.rs + provider.rs | Both files annotated |
| G-001 | gap | subject-matter-expert/analysis-F-003-error-mapping.md "## Interface Contract" | "anthropic/metrics.rs" does not exist — error classification counts recorded via MetricsCollector::record() from handlers.rs/provider.rs | SME file annotated; SA analysis.md interfaces table extended |
| G-002 | gap | test-strategist/analysis.md "### Interfaces" / system-architect/analysis-F-007-module-refactor.md "## Constraints" | SA must acknowledge TS coverage gate — module splits maintain ≥80% line coverage | TS annotated; SA F-007 constraints extended |
| G-003 | gap | system-architect/analysis.md "### Interfaces" / subject-matter-expert/analysis.md "### Interfaces" | CrossRequestCache insertion point: resolve whether stream.rs (SA) or provider.rs (SME) is authoritative | Both files annotated for design-time resolution |
| S-001 | synergy | system-architect/analysis-F-003-error-mapping.md "## Architecture" / subject-matter-expert/analysis.md "### Pitfall Taxonomy" | Compression-induced 400s detected via RequestContext.was_compressed in unified error_map | Both files annotated |
| S-002 | synergy | test-strategist/analysis.md "### Risk-Based Prioritization" / subject-matter-expert/analysis.md "### Pitfall Taxonomy" | SME pitfall severity ratings drive TS integration test priority ordering | Both files annotated |
| S-003 | synergy | product-manager/analysis.md "### Interfaces" / system-architect/analysis.md "### Configuration" | PM Feature Toggle Strategy maps to SA per-feature enabled:bool config fields | Both files annotated |
