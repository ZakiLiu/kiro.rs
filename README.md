# kiro-rs

一个用 Rust 编写的 Anthropic Claude API 兼容代理服务，将 Anthropic API 请求转换为 Kiro API 请求。

> **本仓库基于 [ZyphrZero/kiro.rs](https://github.com/ZyphrZero/kiro.rs) 开发**，在其基础上做了多项增强和修复。

---

## 免责声明

本项目仅供研究使用, Use at your own risk, 使用本项目所导致的任何后果由使用人承担, 与本项目无关。
本项目与 AWS/KIRO/Anthropic/Claude 等官方无关, 本项目不代表官方立场。

## 功能特性

- **Anthropic API 兼容**: 完整支持 Anthropic Claude API 格式（/v1/messages）
- **OpenAI 兼容**: 支持 /v1/chat/completions 和 /v1/responses 格式
- **Gemini 兼容**: 支持 /v1beta/models/*:generateContent 格式
- **流式响应**: 支持 SSE (Server-Sent Events) 流式输出
- **Token 自动刷新**: 自动管理和刷新 OAuth Token（支持 Social 和 IdC 两种认证）
- **多凭据管理**: 支持配置多个凭据，按优先级自动故障转移
- **智能重试**: 单凭据最多重试 2 次，单请求最多重试 3 次
- **凭据回写**: 多凭据格式下自动回写刷新后的 Token
- **Thinking 模式**: 支持 Claude 的 extended thinking 功能
- **工具调用**: 完整支持 function calling / tool use
- **WebSearch**: 内置 WebSearch 工具转换逻辑
- **多模型支持**: 支持 Sonnet、Opus、Haiku 系列模型
- **Admin 管理界面**: Web 管理界面 + 60+ Admin API 端点
- **代理池**: 支持批量代理导入、健康检查、轮询分配
- **客户端 Key 管理**: 多 API Key 发放和用量追踪
- **账号分组**: 凭据和 Key 的逻辑分组管理
- **输入压缩**: 请求体接近上游限制时自动多层压缩（空白→thinking→tool_result→tool_use→历史截断）
- **图片处理**: GIF 抽帧、大图缩放、token 计算
- **Prompt 预设**: 可配置系统提示词注入和安全限制剥离
- **请求追踪**: SQLite 存储的请求链路追踪
- **用量统计**: 按模型、按凭据的用量时序统计
- **在线更新**: 通过 GitHub Releases 自动检查/下载/替换二进制，支持回滚和定时自动更新
- **多级 Region 配置**: 支持全局和凭据级别的 Auth Region / API Region 配置
- **凭据级代理**: 支持为每个凭据单独配置 HTTP/SOCKS5 代理

---

- [开始](#开始)
  - [方式一：下载预编译二进制（推荐）](#方式一下载预编译二进制推荐)
  - [方式二：Docker 部署](#方式二docker-部署)
  - [方式三：从源码编译](#方式三从源码编译)
- [配置详解](#配置详解)
- [API 端点](#api-端点)
- [模型映射](#模型映射)
- [Admin 管理](#admin-管理)
- [在线更新](#在线更新)
- [项目结构](#项目结构)
- [技术栈](#技术栈)
- [License](#license)

## 开始

### 方式一：下载预编译二进制（推荐）

从 [GitHub Releases](https://github.com/ZakiLiu/kiro.rs/releases) 下载对应平台的二进制：

| 平台 | 文件 |
|------|------|
| Linux x64 (静态链接) | `kiro-rs-*-Linux-musl-x64.tar.gz` |
| Linux arm64 (静态链接) | `kiro-rs-*-Linux-musl-arm64.tar.gz` |
| Linux x64 (GNU) | `kiro-rs-*-Linux-x64.tar.gz` |
| Linux arm64 (GNU) | `kiro-rs-*-Linux-arm64.tar.gz` |
| macOS x64 | `kiro-rs-*-macOS-x64.tar.gz` |
| macOS arm64 (Apple Silicon) | `kiro-rs-*-macOS-arm64.tar.gz` |
| Windows x64 | `kiro-rs-*-Windows-x64.zip` |

```bash
# 以 Linux x64 为例
curl -sL https://github.com/ZakiLiu/kiro.rs/releases/latest/download/kiro-rs-2.0.0-Linux-musl-x64.tar.gz | tar xz
chmod +x kiro-rs-*/kiro-rs
./kiro-rs-*/kiro-rs -c config.json --credentials credentials.json
```

**systemd 部署示例**（推荐用于生产环境）：

```ini
# /etc/systemd/system/kiro-rs.service
[Unit]
Description=Kiro.rs Anthropic API Proxy
After=network-online.target

[Service]
Type=simple
WorkingDirectory=/opt/kiro-rs
ExecStart=/opt/kiro-rs/kiro-rs -c /opt/kiro-rs/config/config.json --credentials /opt/kiro-rs/config/credentials.json
Restart=on-failure
RestartSec=3
LimitNOFILE=65535

[Install]
WantedBy=multi-user.target
```

```bash
systemctl enable --now kiro-rs
journalctl -u kiro-rs -f  # 查看日志
```

### 方式二：Docker 部署

Docker 镜像每次发布 Release 时由 GitHub Actions 自动构建，支持 `linux/amd64` 和 `linux/arm64`。

```yaml
# docker-compose.yml
services:
  kiro-rs:
    image: <your-dockerhub-namespace>/kiro-rs:latest
    container_name: kiro-rs
    ports:
      - "8990:8990"
    volumes:
      - ./config/:/app/config/
    restart: unless-stopped
```

```bash
docker compose up -d
```

### 方式三：从源码编译

```bash
# 1. 构建前端
cd admin-ui && pnpm install && pnpm build && cd ..

# 2. 编译后端
cargo build --release

# 3. 运行
./target/release/kiro-rs -c config.json --credentials credentials.json
```

### 最小配置

创建 `config.json`：

```json
{
   "host": "0.0.0.0",
   "port": 8990,
   "apiKey": "sk-kiro-rs-your-secret-key",
   "region": "us-east-1",
   "adminApiKey": "sk-admin-your-secret-key"
}
```

创建 `credentials.json`（Social 认证示例）：

```json
{
   "refreshToken": "你的刷新token",
   "expiresAt": "2025-12-31T02:32:45.144Z",
   "authMethod": "social"
}
```

> 也可以通过 Admin Web UI 在线添加凭据，无需手动编写 credentials.json。

### 验证

```bash
curl http://127.0.0.1:8990/v1/messages \
  -H "Content-Type: application/json" \
  -H "x-api-key: sk-kiro-rs-your-secret-key" \
  -d '{
    "model": "claude-sonnet-4-20250514",
    "max_tokens": 1024,
    "stream": true,
    "messages": [{"role": "user", "content": "Hello!"}]
  }'
```

## 配置详解

### config.json

| 字段 | 类型 | 默认值 | 描述 |
|------|------|--------|------|
| `host` | string | `127.0.0.1` | 服务监听地址 |
| `port` | number | `8080` | 服务监听端口 |
| `apiKey` | string | - | 自定义 API Key（客户端认证，必配） |
| `region` | string | `us-east-1` | AWS 区域 |
| `apiRegion` | string | - | API Region，未配置时回退到 region |
| `kiroVersion` | string | `0.10.0` | Kiro 版本号 |
| `machineId` | string | - | 自定义机器码（64位十六进制） |
| `tlsBackend` | string | `rustls` | TLS 后端：`rustls` 或 `native-tls` |
| `proxyUrl` | string | - | HTTP/SOCKS5 代理地址 |
| `proxyUsername` | string | - | 代理用户名 |
| `proxyPassword` | string | - | 代理密码 |
| `adminApiKey` | string | - | Admin API 密钥，启用管理功能 |
| `credentialRpm` | number | - | 单凭据目标 RPM；`0` 禁用本地节流 |
| `credentialDailyMaxRequests` | number | - | 单凭据每日最大请求数；`0` 禁用 |
| `keepaliveIdleThresholdSeconds` | number | `7200` | 凭据保活探测空闲阈值（秒） |
| `promptCacheTtlSeconds` | number | `300` | 本地 Prompt Cache TTL（秒），支持 `300` / `3600` / `7200` / `18000`；对客户端协议仍最多按 1 小时上报 |
| `promptCacheAccountingEnabled` | bool | `true` | 是否启用 cache usage 记账 |
| `loadBalancingMode` | string | `priority` | `priority` 或 `balanced` |
| `traceEnabled` | bool | `false` | 请求追踪（SQLite） |
| `traceRetentionDays` | number | `7` | 追踪日志保留天数 |
| `metricsEnabled` | bool | `true` | 请求指标收集 |
| `stripSystemRestrictions` | bool | `false` | 剥离 system prompt 安全限制 |
| `systemPromptEnabled` | bool | `false` | 系统提示词注入开关 |
| `updateAutoApply` | bool | `false` | 无人值守自动更新 |
| `updateAutoApplyTime` | string | `03:00` | 自动更新触发时间（HH:MM，本地时区） |
| `githubToken` | string | - | GitHub PAT（提升更新检查 API 限额） |

### credentials.json

支持单凭据（对象）和多凭据（数组）格式。

| 字段 | 类型 | 描述 |
|------|------|------|
| `refreshToken` | string | OAuth 刷新令牌 |
| `expiresAt` | string | Token 过期时间 (RFC3339) |
| `authMethod` | string | `social` 或 `idc` |
| `clientId` | string | IdC 认证必填 |
| `clientSecret` | string | IdC 认证必填 |
| `priority` | number | 优先级（数字越小越优先，默认 0） |
| `region` / `authRegion` / `apiRegion` | string | 凭据级 Region |
| `proxyUrl` | string | 凭据级代理（`direct` = 显式直连） |
| `email` | string | 用户邮箱（可选） |

多凭据示例：

```json
[
   {"refreshToken": "token1", "authMethod": "social", "priority": 0},
   {"refreshToken": "token2", "authMethod": "idc", "clientId": "xxx", "clientSecret": "xxx", "priority": 1}
]
```

### 代理优先级

`凭据.proxyUrl` > `config.proxyUrl` > 无代理

| 凭据 proxyUrl | 行为 |
|---|---|
| URL | 使用凭据代理 |
| `direct` | 显式不使用代理 |
| 未配置 | 回退到全局代理 |

## API 端点

### 代理端点

| 方法 | 端点 | 描述 |
|------|------|------|
| GET | `/v1/models` | 获取可用模型列表 |
| POST | `/v1/messages` | 创建消息（Anthropic 格式） |
| POST | `/v1/messages/count_tokens` | Token 计数 |
| POST | `/v1/chat/completions` | 创建消息（OpenAI 格式） |
| POST | `/v1/responses` | 创建消息（OpenAI Responses 格式） |
| POST | `/v1beta/models/*:generateContent` | 创建消息（Gemini 格式） |
| GET | `/health` | 健康检查 |

### 认证方式

```
x-api-key: sk-your-api-key
# 或
Authorization: Bearer sk-your-api-key
```

## 模型映射

| Anthropic 模型 | Kiro 模型 |
|----------------|-----------|
| `*sonnet*`（含 4-6/4.6） | `claude-sonnet-4.6` |
| `*sonnet*`（其他） | `claude-sonnet-4.5` |
| `*opus*`（含 4-5/4.5） | `claude-opus-4.5` |
| `*opus*`（其他） | `claude-opus-4.6` |
| `*haiku*` | `claude-haiku-4.5` |

## Admin 管理

配置 `adminApiKey` 后启用 Admin API 和 Web UI。

- **Web UI**: `http://<host>:<port>/admin`
- **API**: `http://<host>:<port>/api/admin/...`（需 `x-api-key` 认证）

### 主要 Admin API

| 类别 | 端点 | 描述 |
|------|------|------|
| 凭据 | `GET/POST /api/admin/credentials` | 列表 / 添加凭据 |
| 凭据 | `DELETE/PUT /api/admin/credentials/{id}` | 删除 / 更新 |
| 凭据 | `POST /api/admin/credentials/{id}/disabled` | 启用/禁用 |
| 凭据 | `GET /api/admin/credentials/{id}/balance` | 查询余额 |
| 凭据 | `POST /api/admin/credentials/{id}/proxy` | 分配代理 |
| 代理池 | `GET/POST /api/admin/proxy-pool` | 列表 / 添加代理 |
| 代理池 | `POST /api/admin/proxy-pool/check-all` | 批量健康检查 |
| 配置 | `GET/PUT /api/admin/config/global` | 全局配置 |
| 配置 | `GET/PUT /api/admin/config/update` | 更新配置 |
| 客户端 Key | `GET/POST /api/admin/client-keys` | Key 管理 |
| 分组 | `GET/POST /api/admin/groups` | 分组管理 |
| 统计 | `GET /api/admin/stats/overview` | 用量概览 |
| 追踪 | `GET /api/admin/traces` | 请求追踪 |
| 预设 | `GET/POST /api/admin/presets` | Prompt 预设 |
| 更新 | `GET /api/admin/system/update/check` | 检查新版本 |
| 更新 | `POST /api/admin/system/update/apply` | 下载并应用更新 |
| 更新 | `POST /api/admin/system/update/rollback` | 回滚到上一版本 |
| 登录 | `POST /api/admin/auth/social/start` | Social 登录 |
| 登录 | `POST /api/admin/auth/idc/start` | IdC 设备码登录 |

## 在线更新

服务内置了基于 GitHub Releases 的自动更新系统，无需手动下载替换。

### 手动更新

通过 Admin API 触发：

```bash
# 1. 检查是否有新版本
curl -H "x-api-key: $ADMIN_KEY" http://localhost:8990/api/admin/system/update/check

# 2. 下载并应用（服务会自动重启）
curl -X POST -H "x-api-key: $ADMIN_KEY" http://localhost:8990/api/admin/system/update/apply

# 3. 出问题？回滚
curl -X POST -H "x-api-key: $ADMIN_KEY" http://localhost:8990/api/admin/system/update/rollback
```

### 自动更新

在 `config.json` 中配置：

```json
{
  "updateAutoApply": true,
  "updateAutoApplyTime": "03:00"
}
```

或通过 Admin API 动态修改：

```bash
curl -X PUT -H "x-api-key: $ADMIN_KEY" -H "Content-Type: application/json" \
  http://localhost:8990/api/admin/config/update \
  -d '{"autoApply": true, "autoApplyTime": "03:00"}'
```

### GitHub Token（可选）

GitHub API 匿名限额 60 次/小时，配置 PAT 可提升到 5000 次/小时：

```bash
curl -X PUT -H "x-api-key: $ADMIN_KEY" -H "Content-Type: application/json" \
  http://localhost:8990/api/admin/config/update \
  -d '{"githubToken": "ghp_xxxxxxxxxxxx"}'
```

## 注意事项

1. **凭证安全**: 请妥善保管 `credentials.json` 文件，不要提交到版本控制
2. **Token 刷新**: 服务会自动刷新过期的 Token，无需手动干预
3. **TLS 后端**: 默认使用 rustls；遇到代理或证书问题时可切换为 `native-tls`
4. **在线更新**: 更新时旧二进制会备份为 `<exe>.backup`，可随时回滚
5. **输入压缩**: 请求体接近 5MB 上游限制时自动执行多层压缩

## 项目结构

```
kiro-rs/
├── src/
│   ├── main.rs                  # 程序入口
│   ├── model/                   # 配置和参数模型
│   │   ├── config.rs            # 应用配置
│   │   └── arg.rs               # 命令行参数
│   ├── anthropic/               # Anthropic API 兼容层
│   │   ├── router.rs            # 路由配置
│   │   ├── handlers.rs          # 请求处理器
│   │   ├── middleware.rs        # 认证中间件
│   │   ├── converter/           # 协议转换器
│   │   ├── stream.rs            # 流式响应处理
│   │   ├── compressor.rs        # 输入压缩管道
│   │   └── websearch.rs         # WebSearch 处理
│   ├── openai/                  # OpenAI API 兼容层
│   ├── gemini/                  # Gemini API 兼容层
│   ├── kiro/                    # Kiro API 客户端
│   │   ├── provider.rs          # API 提供者（重试 + 故障转移）
│   │   ├── token_manager/       # Token 管理（多凭据）
│   │   ├── auth/                # 认证（Social OAuth + IdC 设备码）
│   │   └── parser/              # AWS Event Stream 解析器
│   ├── admin/                   # Admin API 模块
│   │   ├── router.rs            # 路由配置（60+ 端点）
│   │   ├── handlers.rs          # 请求处理器
│   │   ├── service.rs           # 业务逻辑
│   │   ├── binary_update.rs     # 在线更新（下载/校验/安装/回滚）
│   │   ├── proxy_pool.rs        # 代理池管理
│   │   ├── client_keys.rs       # 客户端 Key 管理
│   │   ├── groups.rs            # 账号分组
│   │   ├── trace_db.rs          # 请求追踪（SQLite）
│   │   └── usage_stats.rs       # 用量统计
│   └── admin_ui/                # Admin UI 静态文件嵌入
├── admin-ui/                    # Admin UI 前端（React + TypeScript）
├── .github/workflows/
│   └── release.yaml             # CI：7 平台构建 + Docker + GitHub Release
├── Dockerfile                   # Docker 构建（从源码）
├── Dockerfile.release           # Docker 构建（预编译二进制）
├── Cargo.toml
└── config.example.json
```

## 技术栈

- **语言**: Rust (Edition 2024)
- **Web 框架**: [Axum](https://github.com/tokio-rs/axum) 0.8
- **异步运行时**: [Tokio](https://tokio.rs/)
- **HTTP 客户端**: [Reqwest](https://github.com/seanmonstar/reqwest) (rustls-tls)
- **序列化**: [Serde](https://serde.rs/) + serde_json (preserve_order)
- **数据库**: [rusqlite](https://github.com/rusqlite/rusqlite) (SQLite, 请求追踪)
- **前端**: React 18 + TypeScript + Tailwind CSS

## License

MIT

## 致谢

本项目基于 [ZyphrZero/kiro.rs](https://github.com/ZyphrZero/kiro.rs) 开发。

原项目的实现也离不开前辈的努力:
 - [kiro2api](https://github.com/caidaoli/kiro2api)
 - [proxycast](https://github.com/aiclientproxy/proxycast)

部分逻辑参考了以上项目, 由衷的感谢!
