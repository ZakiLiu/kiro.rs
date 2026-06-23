---
title: Periodic Credential Recovery & Background Token Refresh
category: resilience
createdBy: wiki-digest
related:
  - knowhow-knw-periodic-recovery-2026-05-25
  - "spec:project:coding-conventions"
  - knowhow-knw-follow-provider-2026-05-24
---

# Periodic Credential Recovery & Background Token Refresh

## 概述

v1.1.31+ 新增两个后台任务，补全凭据生命周期管理的闭环：

1. **周期性凭据恢复** (`start_periodic_recovery`) — 自动恢复被错误禁用的凭据
2. **后台 Token 刷新** (`start_background_token_refresh`) — 防止空闲期 Token 过期

## 周期性凭据恢复

### 设计动机

凭据被自动禁用后（余额不足、认证失败、连续错误），可能因外部条件变化而恢复可用（月初余额重置、OAuth 服务恢复）。没有自动恢复机制时，这些凭据会永久处于禁用状态，需要人工干预。

### 恢复策略

| 禁用原因 | 恢复方式 | 判定条件 |
|----------|---------|---------|
| InsufficientBalance | 重新查询余额 | remaining >= 1.0 |
| QuotaExceeded | 重新查询余额 | remaining >= 1.0 |
| AuthenticationFailed | 尝试刷新 Token | 刷新成功 |
| RefreshFailureLimit | 尝试刷新 Token | 刷新成功 |
| FailureLimit | 尝试刷新 Token | 刷新成功 |
| Manual | **不恢复** | — |
| AccountSuspended | **不恢复** | — |
| ModelUnavailable | **不恢复**（有独立机制） | — |

### 指数退避

```
backoff = min(5min × 2^attempts, 120min)
```

- 基础间隔：5 分钟
- 每次恢复失败：attempts + 1，退避翻倍
- 上限：2 小时（attempts.min(5) 防止溢出）
- 恢复成功：重置 attempts = 0

### 关键实现细节

- `get_recovery_candidates()` 持有 entries 锁做时间计算（凭据数 <50 时可接受）
- `increment_recovery_attempts()` 每次调用持久化到磁盘，防止重启后退避丢失
- 恢复循环中每个凭据间隔 500ms，避免突发压力
- `force_refresh_token_for()` 排除 Manual 和 AccountSuspended，防止意外恢复

### 启动方式

```rust
// main.rs — 5 分钟检查间隔
kiro_provider.start_periodic_recovery(300);
```

## 后台 Token 刷新

### 设计动机

长时间无请求时，OAuth Token 可能过期。当请求到来时触发刷新，如果刷新失败（网络抖动、OAuth 服务暂时不可用），凭据会被标记为 RefreshFailureLimit 并禁用。后台预刷新避免了这个窗口。

### 配置参数

| 参数 | 默认值 | 说明 |
|------|--------|------|
| check_interval_secs | 60s | 检查间隔 |
| batch_size | 50 | 每批处理凭据数 |
| concurrency | 10 | 并发刷新数 |
| refresh_before_expiry_mins | 15min | 过期前多久开始刷新 |

### 存活机制

```rust
// 用 pending future 保持 Arc<BackgroundRefresher> 存活
tokio::spawn(async move {
    let _keep_alive = refresher;
    std::future::pending::<()>().await;
});
```

BackgroundRefresher 通过 Arc 引用计数控制生命周期，drop 时自动停止后台任务。

## 与现有机制的关系

```
请求路径错误 → 凭据禁用（Circuit Breaker）
                    ↓
        周期性恢复（本机制）← 指数退避
                    ↓
        恢复成功 → 重新加入轮转池
        恢复失败 → 增加退避，等待下次

空闲期 → Token 过期风险
            ↓
    后台预刷新（本机制）← 15min 提前量
            ↓
    刷新成功 → Token 保持有效
    刷新失败 → 下次重试（不禁用）
```

## 决策记录

- **为什么 increment_recovery_attempts 要持久化？** 防止进程重启后退避计数丢失，导致所有失败凭据立即重试造成突发压力。
- **为什么 force_refresh_token_for 要排除 AccountSuspended？** 账户暂停是外部决定，Token 刷新成功不代表账户恢复，需要人工确认。
- **为什么用 pending future 而不是 JoinHandle？** BackgroundRefresher 内部已有 shutdown 机制（AtomicBool + Notify），只需保持引用存活即可。
