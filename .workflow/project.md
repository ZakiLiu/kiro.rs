# Project: kiro-rs

## What This Is

Anthropic Claude API 兼容的 Kiro 代理服务，用 Rust 编写。将 Anthropic API 请求转换为 Kiro API 请求，支持多凭据管理、自动故障转移、流式响应和 Web 管理界面。面向需要通过 Anthropic 兼容接口访问 Kiro 服务的开发者。

## Core Value

**请求代理的可靠性**——无论凭据状态如何变化，用户的 API 请求必须被正确转发并返回响应。故障转移、重试、协议转换必须透明且稳定。

## Requirements

### Validated

<!-- 已上线并确认有价值的功能 -->

- [x] Anthropic Messages API 兼容代理（POST /v1/messages）
- [x] 多凭据管理与优先级故障转移
- [x] AWS Event Stream → Anthropic SSE 流式转换
- [x] Token 自动刷新（Social + IdC 认证）
- [x] Web 管理界面（React + rust-embed 嵌入）
- [x] 输入压缩管道（空白/thinking/tool_result/历史截断）
- [x] 图片处理（缩放、GIF 抽帧）
- [x] WebSearch 工具路由
- [x] Admin API（凭据 CRUD、状态监控、余额查询）
- [x] 凭据级代理支持（HTTP/SOCKS5）

### Active

<!-- 当前正在构建的需求 -->

- [ ] 持续维护与 bug 修复
- [ ] 上游 API 变更适配

### Out of Scope

- 自建 LLM 推理引擎 — 本项目仅做协议转换和代理
- 多租户 SaaS 化 — 定位为单实例自部署工具

## Context

项目已进入稳定维护阶段（v1.1.31），核心功能完备。主要工作集中在跟进上游 API 变更、修复边缘 case、优化性能。Docker 镜像发布到 Docker Hub（myuan6/kiro-rs）。

## Constraints

- **兼容性**: 必须保持 Anthropic API 格式兼容 — 下游客户端（Claude Code、Cursor 等）依赖标准格式
- **性能**: 流式响应不能引入明显延迟 — 用户体验直接受影响
- **安全**: API Key 常量时间比较、敏感日志默认关闭 — 防止时序攻击和信息泄露

## Tech Stack

- **Language**: Rust (Edition 2024)
- **Backend Framework**: Axum 0.8 + Tokio
- **Frontend**: React 18 + TypeScript + Tailwind CSS + Vite
- **HTTP Client**: reqwest 0.12 (rustls-tls, socks)
- **Serialization**: serde + serde_json (preserve_order)
- **Image Processing**: image 0.25
- **Static Embedding**: rust-embed 8

## Key Decisions

| Decision | Rationale | Outcome |
|----------|-----------|---------|
| Axum 0.8 作为 HTTP 框架 | 类型安全、性能好、Tokio 生态原生 | 稳定运行 |
| serde_json preserve_order | Kiro CLI 依赖字段顺序的字节对齐 | 避免协议兼容问题 |
| rust-embed 嵌入前端 | 单二进制部署，无需额外静态文件服务 | 简化部署 |
| 多层输入压缩 | 避免上游 5MB 限制导致请求失败 | 减少 400 错误 |
| GIF 抽帧为 JPEG 序列 | 降低请求体大小，提升内容识别效果 | 兼顾质量和性能 |
| subtle 常量时间比较 | 防止 API Key 时序攻击 | 安全加固 |

## Stakeholders

- 项目维护者（haoyue）
- 下游用户（通过 Anthropic 兼容接口使用 Kiro 服务的开发者）

---
*Last updated: 2026-05-24 after initialization*
