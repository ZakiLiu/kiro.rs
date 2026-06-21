 1. kiro cli 有debug模式吗？

 Kiro CLI 没有单一的"debug 模式"开关，但提供了几个用于调试和排查问题的工具，根据你的需求可以选择对应的方式：

  调试日志（最接近"debug 模式"）
  通过环境变量控制日志详细程度：

  KIRO_LOG_LEVEL=debug kiro-cli acp
  KIRO_CHAT_LOG_FILE=/path/to/custom.log kiro-cli acp

  日志默认位置：

  - macOS: $TMPDIR/kiro-log/kiro-chat.log
  - Linux: /tmp/kiro-log/logs/kiro-chat.log

  性能调试 /stats
  查看最近请求的 request ID、耗时、token 数等，用于排查响应慢的问题：

  /stats          # 查看全部
  /stats 5        # 最近 5 条
  /stats save debug-session.json   # 导出

  系统诊断 kiro-cli diagnostic
  生成系统信息报告（版本、系统、环境变量等），适合排障或反馈给支持团队：

  kiro-cli diagnostic
  kiro-cli diagnostic --format json-pretty

  查看 AI 推理过程 chat.showThinking
  想看 agent 的思考/推理过程来调试它的行为：

  kiro-cli settings chat.showThinking expanded

  LSP 代码智能日志 /code logs
  调试语言服务器相关问题：

  /code logs -l DEBUG -n 50

2. kiro cli 访问的端点是什么
文档里没有给出 Kiro CLI 默认访问的具体端点 URL。文档说明的是这些端点可以配置/覆盖，但默认值只标注为"uses default AWS
  endpoint"，没有列出实际地址。

  可以配置的几个服务端点设置如下（都在 kiro-cli settings 里管理）：

  api.codewhisperer.service — CodeWhisperer 服务端点

  kiro-cli settings api.codewhisperer.service '{"endpoint": "https://custom.endpoint.com", "region": "us-east-1"}'

  默认：使用 AWS 默认端点（文档未给出具体 URL）。作用域：仅全局。

  api.q.service — Q 服务端点

  kiro-cli settings api.q.service '{"endpoint": "https://custom.q.endpoint.com", "region": "us-east-1"}'

  默认：使用 AWS 默认端点。作用域：仅全局。

  api.kiroauth.service — Kiro 认证服务端点

  kiro-cli settings api.kiroauth.service "https://custom.auth.endpoint.com"

  默认：使用默认端点（文档未给出具体 URL）。作用域：仅全局。

  此外还有相关的超时设置 api.timeout（默认 300000 毫秒）和 OIDC 认证的 api.oidc.scopePrefix。

  如果你想知道当前实际连接的端点地址，可以试试查询当前设置值，例如：

  kiro-cli settings api.q.service

  不过如果没有自定义覆盖，它返回的是默认（空），具体的默认 URL 在文档中没有记录。

3. kiro cli 在本机有哪些帮助和调试文档吗？

我从两个角度来看：本机实际存在的文件，以及 CLI 内置的帮助/调试命令。先查一下本机的 Kiro 相关目录和日志。
● Glob ".kiro/**/*"
    ╰ path=C:\Users\HomePC1, limit=100
  19 files
    C:\Users\HomePC1\.kiro\argv.json, C:\Users\HomePC1\.kiro\extensions\extensions.json
    C:\Users\HomePC1\.kiro\logs\20260620T121202969\kiro.log +16 more
● Glob "**/kiro-log/**/*"
    ╰ path=C:\Users\HomePC1\AppData\Local\Temp, limit=50
  4 files
    C:\Users\HomePC1\AppData\Local\Temp\kiro-log\kiro-tui.log
    C:\Users\HomePC1\AppData\Local\Temp\kiro-log\logs\kiro-chat.log
    C:\Users\HomePC1\AppData\Local\Temp\kiro-log\logs\lsp.log +1 more

  本机确实有一批 Kiro 的配置、日志和会话文件，可用于排查问题。结合 CLI 自带的调试命令，整理如下：

  本机实际存在的文件

  调试/日志（最有用）

  - C:\Users\HomePC1\AppData\Local\Temp\kiro-log\logs\kiro-chat.log — 主聊天日志
  - ...\kiro-log\kiro-tui.log — TUI 界面日志
  - ...\kiro-log\logs\lsp.log — 代码智能 (LSP) 日志
  - ...\kiro-log\logs\mcp.log — MCP 服务器日志
  - C:\Users\HomePC1\.kiro\logs\20260620T121202969\ 下还有 kiro.log、mcp.log、powers.log

  配置文件

  - C:\Users\HomePC1\.kiro\settings\cli.json — CLI 设置
  - C:\Users\HomePC1\.kiro\argv.json
  - C:\Users\HomePC1\.kiro\extensions\extensions.json
  - C:\Users\HomePC1\.kiro\powers\registry.json

  会话历史（排查具体对话问题用）

  - C:\Users\HomePC1\.kiro\sessions\cli\*.jsonl / .json / .history

  CLI 内置的帮助与调试命令

  - kiro-cli diagnostic — 生成系统诊断报告（版本、系统、环境变量），排障首选
  - /stats — 查看请求 ID、耗时、token，用于调试响应慢
  - /code logs -l DEBUG -n 50 — 查看 LSP 日志
  - KIRO_LOG_LEVEL=debug 环境变量 — 提升日志详细度
  - 各命令的 --help / -h 参数

  需要我帮你做什么吗？比如读一下 kiro-chat.log 看最近有没有报错，或者看一眼 cli.json
  当前配置。我可以直接读这些本机文件。

4. kiro cli 的在线文档

Kiro CLI 的官方在线文档在 kiro.dev，主要入口如下：

  主文档站

  - 文档首页：https://kiro.dev/docs/ (https://kiro.dev/docs/)
  - CLI 文档区：https://kiro.dev/docs/cli/ (https://kiro.dev/docs/cli/)
  - CLI 产品页：https://kiro.dev/cli/ (https://kiro.dev/cli/)

  CLI 文档常用页面（基于官方导航结构）

  - 从 Amazon Q 迁移：https://kiro.dev/docs/cli/migrating-from-q/ (https://kiro.dev/docs/cli/migrating-from-q/)
  - 认证：https://kiro.dev/docs/cli/authentication (https://kiro.dev/docs/cli/authentication)
  - CLI 命令参考：https://kiro.dev/docs/cli/reference/cli-commands (https://kiro.dev/docs/cli/reference/cli-commands)
  - Chat 功能：https://kiro.dev/docs/cli/chat (https://kiro.dev/docs/cli/chat)
  - 自定义 Agent：https://kiro.dev/docs/cli/custom-agents (https://kiro.dev/docs/cli/custom-agents)
  - MCP 集成：https://kiro.dev/docs/cli/mcp (https://kiro.dev/docs/cli/mcp)
  - Agent Hooks：https://kiro.dev/docs/cli/hooks (https://kiro.dev/docs/cli/hooks)

  其他资源

  - 更新日志：https://kiro.dev/changelog/ (https://kiro.dev/changelog/)
  - FAQ：https://kiro.dev/faq/ (https://kiro.dev/faq/)
  - 报告 bug：https://github.com/kirodotdev/Kiro/issues/new/choose
   (https://github.com/kirodotdev/Kiro/issues/new/choose)