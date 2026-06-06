# Scan Wisdom — Issues (SCAN-001)

## 金矿发现：抽象做好了但没接上（pattern）
本次 fusion 引入两个精心设计的抽象，集成时却没真正接通——这是测试绿但行为错的典型：

- **SEC-001 (critical)**：CrossRequestCache key 设计为 (credential_id, fp) 实现凭据隔离，identity.rs 还做了 cache_identity() 域分离。但 handlers.rs:979/1132 两处 lookup/insert 全写死 credential_id=0 → 跨凭据 conversation_id 串话，隔离形同虚设。单测用 1/2/3 测隔离，生产全传 0，测试与真实路径脱节。
- **MNT-002**：cache_identity() 同源——生产路径从未调用，仅单测引用。

> 教训：单元测试覆盖了组件 API，但没有集成测试覆盖「真实调用方传什么参数」。审 review 时必须追到调用点核对参数，不能只看组件自身测试。

## 字符串匹配做错误分类是脆弱耦合（COR-002）
error_map::classify 靠匹配 token_manager 的中文错误文案分类。文案一改分类静默退化为 Unknown(500)，把 4xx 误报 5xx 诱发客户端重试。无编译期保护。建议结构化错误类型。

## 静默吞错误（COR-001）
TokenUsageEvent::from_frame: Err(_) => Ok(default())。解析失败静默退化全 0，计费数据丢失且无日志。审 Rust 代码时盯 `Err(_) =>` 和 `unwrap_or_default()` 在数据路径上的使用。

## 大小检查顺序（SEC-002）
pdf.rs 先 base64 decode 整块再查 10MB 上限 → 内存放大 DoS。审「解码/解压再校验」模式时永远检查：校验是否在分配之前。

---
# Review Wisdom — Deep Analysis (REV-001)

## SEC-001 利用面边界（reviewer 追加）
critical 定级正确，但实际串话**只在 conv_id 走非内容确定性分支时发生**：
- conv_id 派生有 3 路（converter/mod.rs:106-119）：(a) metadata.user_id 的 session UUID、(b) history 前缀指纹、(c) 空 history 走 Uuid::v4()。
- **分支 (b)**：两请求同内容 → 派生相同 conv_id → 写死 0 命中后注入的值与自身一致 → **无害**。
- **分支 (a)/(c)**：非内容确定性 → credential A 缓存的 conv_id 被 credential B（同 content_fingerprint）命中注入 → **真串话**。
> 审「隔离失效」类 bug 时，不能只确认 key 维度坍缩，还要追下游：被污染的值是否本就确定性。确定性值的"串话"可能无害；只有非确定性/含会话标识的值才构成真实隔离破坏。

## 修复时序错配（SEC-001 的隐藏难点）
lookup 发生在选凭据**之前**（凭据在 provider.call_api 内部按优先级/故障转移选定）。所以"传真实 credential_id"非一行替换——lookup 时凭据未知。须把 key 改为入口即可确定的 (cache_identity, fp)，或重定义缓存语义。审"写死常量"bug 时要查清正确值在该时序点是否可得。

## 同源/同文件聚类（fix 排期参考）
- cache-isolation-broken: SEC-001(主) + MNT-002(症状) + PRF-001(同文件) → 一次重写 CrossRequestCache 闭环。
- error-handling-coupling: COR-002 + SEC-003 同在 error_map.rs → 引入结构化错误类型一并收敛分类与对外文案。
