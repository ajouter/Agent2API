# OpenCode 内置提供商 — 设计规格

日期：2026-10-24
状态：已批准（用户确认：新增内置提供商 / 粘贴 API Key / 固定官方地址 + 可选覆盖）

## 1. 背景与目标

把 [opencode-go-proxy-for-trae](https://github.com/LIMTCYT/opencode-go-proxy-for-trae)（Python 单文件本地代理）验证过的 OpenCode Go / Zen API 接入方式，做成 Agent2API 的一个**内置提供商**：

- 用户在「账号」页粘贴自己的 OpenCode API Key（opencode.ai 控制台获取）即可添加账号；
- 网关对每个上游请求自动注入 OpenCode 要求的会话头，支持多账号轮转、429 冷却换号、请求报表，与现有 17 家提供商同一体验；
- 按请求模型**自动选择上游路由**（`/zen/go/v1` 与 `/inference/go/v1`），免除参考项目要求用户按模型手填不同 Base URL 的负担。

上游协议：OpenAI 兼容 `POST /{route}/chat/completions`（route ∈ {`zen/go/v1`, `inference/go/v1`}），凭 `Authorization: Bearer <API Key>` 鉴权。

## 2. 方案决策记录

| 方案 | 结论 | 理由 |
| --- | --- | --- |
| A. 原生内置提供商（stateless 适配器，克隆 Zcode/OfficeAce 形态） | **采纳** | 与现有提供商模式一致，功能完整且代码量最小 |
| B. 自定义提供商预设（静态头） | 否决 | `x-opencode-session` 须按会话稳定、`x-opencode-request` 须会话内递增，静态头做不到 |
| C. 内嵌旁路代理（搬运参考 Python 实现） | 否决 | 绕开编排层轮转/冷却/报表，双份日志，重复造轮子 |

参考项目的心跳保活 / 竞速重试 / 429 指数退避**不移植**：编排层（`provider_loop`）已具备重试、账号轮转与冷却机制，多账号场景下「换号重试」优于「同号死等」。模型别名 `proxy-` 前缀改写同样不移植（网关有自己的模型映射体系）。

## 3. 上游协议规格（源自参考实现逐行核对）

### 3.1 端点

- 默认 host：`https://opencode.ai`（账号可配 `baseUrl` 覆盖，须为合法 http(s) URL）。
- **路由（2026-10-24 实测修正）**：参考项目注释里「老模型只在 `/zen/go/v1`、新模型只在 `/inference/go/v1`」的说法**已过时**——两条路由的 `GET /models` 均无需鉴权且返回**完全相同的 37 个模型**。因此固定走推荐的 `/zen/go/v1`（参考实现口径：新路由间歇性 429），不做按模型选路。
- `GET {base}/zen/go/v1/models` 无需鉴权（实测），远端目录刷新因此总能拿到清单；仍随请求带上鉴权头以兼容上游将来收紧。

### 3.2 请求头（缺 `x-opencode-session` 上游 400：`Request is missing x-opencode-session and cannot be routed efficiently`）

| 头 | 取值 |
| --- | --- |
| `Authorization` | `Bearer {api_key}` |
| `Content-Type` | `application/json` |
| `Accept` | `*/*` |
| `User-Agent` | `opencode/1.18.29 cli`（常量） |
| `x-opencode-session` | 会话 ID（见 3.3） |
| `x-opencode-request` | `msg_{n}`，同一会话键内递增 |
| `x-opencode-client` | `cli` |
| `x-opencode-project` | `global` |

### 3.3 会话解析（优先级从高到低）

1. **入站头**：`x-opencode-session` / `x-session-id` / `x-conversation-id` / `x-thread-id`（第一个合法者）；
2. **请求体字段**：`session_id`/`sessionId`/`conversation_id`/`conversationId`/`thread_id`/`threadId`，含 `metadata.*` 同名键；
3. **内容哈希**：`system`（数组则逐项换行拼接）+ 第一条 `role=="user"` 消息的 `content`（非字符串则 JSON 序列化），两段以 `\n` 相接做 SHA-256；两段皆空则跳过此档；
4. **随机**：`ses_` + 26 位十六进制。

ID 合法性（`normalize`）：非空、≤128 字符、仅 `[A-Za-z0-9_.:-]`，不合法视为未提供。

会话表：全局 `Mutex<HashMap<key, {session_id, req_num}>>`，上限 256 条，超限按插入序裁最旧。键：显式档用 `"explicit:{id}"`，哈希档用 `"k_{sha256}"`；随机会话不入表（`req_num` 恒 1）。会话键与账号无关——**多账号轮转不影响上游提示词缓存路由**。

### 3.4 错误语义

- 401/403：Key 无效/被拒；
- 429（含错误体 `Endpoint is unavailable` 的「伪 429」端点瞬时故障；新路由该故障报 503，错误体相同）：可轮转重试；
- 缺会话头的 400：不应发生（网关恒注入），原样透传。

## 4. 架构与改动清单

路径缩写：`$P` = `desktop-tauri/src-tauri/server/src/server/core/providers`，`$S` = `.../server/src/server`，`$UI` = `desktop-tauri`。

### Rust（必须）

| # | 文件 | 改动 |
| --- | --- | --- |
| 1 | `$P/mod.rs` | `pub mod opencode;`；`ProviderKind::OpenCode`（追加末尾，:172-472）；`PROVIDERS` 加 `{ id: "opencode", label: "OpenCode" }`（:495-568）；`kind_from_id`/`kind_id` 各加分支（:623/:668） |
| 2 | `$P/opencode/`（新建） | `mod.rs`（常量：`PROVIDER_ID="opencode"`、`DEFAULT_BASE_URL`、`CHAT_ROUTES`、`CLIENT_UA`）；`credentials.rs`（`Credential{api_key, base_url:Option<String>}`，`from_payload` 容错认 `apiKey`/`api_key`/`key`）；`session.rs`（§3.3 全量移植 + 单测）；`models.rs`（静态种子表带 route + 远端刷新）；`adapter.rs`（`OPENCODE_ADAPTER`） |
| 3 | `$P/adapter.rs` | `adapter_for` 分支（:1096-1156）+ `implemented_kinds()`（:1188-1254） |
| 4 | `$P/catalog.rs` | `refresh_meta` 加 OpenCode 分支（:83-194） |
| 5 | `$P/catalog_cache.rs` | `SCOPE_OPENCODE` 常量 + all-scopes 列表（:67-123） |
| 6 | `$S/core/account_store/opencode_accounts.rs`（新建）+ `mod.rs` 声明 + `store_view.rs` 公开视图（key 脱敏掩码，按 apiKey 去重） | 账号存取 |
| 6a | `$S/core/account_store/store.rs` | `session_from_record`（:489）加 OpenCode 附加键块（仿 ZCode/Antigravity 先例）：记录的 `baseUrl` 非空时注入会话顶层 `baseUrl`，适配器经会话读到账号级覆盖 |
| 7 | `$S/api/accounts.rs` | `add_account` 加 OpenCode 分支（:619+，仿 Command Code）；`refresh_account` 的提示分支仿 Command Code（:1524 一带）加「无法续期，请重新粘贴」文案（无续期，不进刷新分派表） |

### 适配器行为（仿 Zcode / Command Code）

- `build_chat_request(account, body, client_headers)`：headers 见 §3.2；body 克隆保留（`body.model` 在编排层 `send_body` 已解析为上游名，`provider_loop.rs:998` 调用点已核实），补 `stream_options.include_usage`（不覆盖客户端已有值）；URL = `{baseUrl}/zen/go/v1/chat/completions`；会话解析在此处调用（同步、零网络）；
- `classify_error`：429 → `QuotaLimited{reset_at: None}`；503 且错误体含 `Endpoint is unavailable`（端点瞬时故障）→ 同样按 `QuotaLimited`（下游 429）；401/403 → `TokenExpired`；其余走 `content_block::classify_or_fatal` 与各家同口径；
- `refresh_access_token` 如实报错（仿 Zcode 文案）；`supports_refresh/supports_web_login/supports_usage` 均 false；
- `sse_model_rewrite` 为 true（与 raccoon / autoclaw 同一处境：账号可配模型别名，SSE 帧里的上游名要改回客户端请求的名字）；
- `refresh_models`：`GET {base}/zen/go/v1/models`（目录公开，仍带鉴权头），OpenAI 风格 `{data:[{id}]}` 解析，成功写 `catalog_cache`，失败静默回落静态表；

### 前端 / 文档

| # | 文件 | 改动 |
| --- | --- | --- |
| 8 | `$UI/ui-islands/src/islands/add-account-configs.ts` | `OPENCODE` 配置（apiKey 必填密码框 + baseUrl 选填高级项）+ `BUILTIN_CONFIGS` 追加 |
| 9 | `$UI/ui-islands/src/islands/add-provider-pick.tsx` | `PROVIDER_ICONS` 加 `opencode` + 图标 PNG 资产 |
| 10 | `$UI/ui/i18n/{en,ja,ko,pt-BR,zh-Hant}/add-account.js` + `$UI/ui-islands/src/i18n.ts` | 文案五语 |
| 11 | `README.md` + `docs/README.{en,zh-Hant,ja,ko,pt-BR}.md` | 支持表格加一行：LLM ✓ / 续期 — / 模型列表 ✓ / 余额 — / 签到 —；「17 家」→「18 家」 |

### 明确不做

签到（不进 `CHECKIN_PROVIDERS`）、余额、Token 续期、网页登录、心跳/竞速重试、`proxy-` 别名、`account_bootstrap` 迁移、桌面登录窗口改动。

## 5. 错误处理与边界

- 缺 key：添加时校验必填；转发时 401「OpenCode 账号缺少 API Key」；
- `baseUrl` 添加时校验 scheme（http/https）；
- 会话表并发：`Mutex` 锁内仅短操作（查/插/裁剪/自增）；
- 上游 400 缺会话头：Fatal 原样透传（不应发生）；
- 远端模型刷新失败：静默保留静态表 + 缓存。

## 6. 测试计划

- 单测（Rust）：session 四档优先级、`normalize` 拒绝非法 ID、计数器递增与 256 裁剪、内容哈希稳定性（system 数组/字符串、content 对象）、`from_payload` 各键形与非法输入、`classify_error` 各档、models 静态表 route 归属与远端解析；
- `cargo test` 全绿；
- 手动冒烟：添加账号 → `GET /v1/models` → `curl /v1/chat/completions` 流式与非流式各一，核对注入头日志与报表计量。

## 7. 实现时核对项（已全部解决）

1. ✅ `provider_loop` 调用 `build_chat_request` 时 `body.model` 已是上游名（`send_body` 先解析 wire model，`provider_loop.rs:998-1003`）；
2. ✅ SSE 帧内模型名回写：开 `sse_model_rewrite()`（与 raccoon / autoclaw 同口径）；
3. ✅ 无续期提供商的刷新按钮：仿 Command Code 的提示分支（`accounts.rs:1524` 一带）；
4. ✅ 模型清单：`/zen/go/v1/models` 公开可拉（无需鉴权），2026-10-24 实测 37 个模型；两条路由清单相同，固定 `/zen/go/v1`，不做按模型选路。

**注册表顺序的既知影响（如实告知用户）**：OpenCode 排在 `PROVIDERS` 末尾，其模型名（`glm-5.3` / `kimi-k3` / `deepseek-v4-pro` 等）与 codearts / catpaw / zcode / officeace / orcaterm 大面积同名——目录合并时同名模型先归已有各家，OpenCode 的同名模型在统一名单里会被「让位」。用户要用 OpenCode 的同名模型，可停用其它家的账号、或用模型映射/自定义模型名绑定到 opencode。这与 OrcaTerm 接入时的既定处置一致。

> 注：本目录当前不是 git 仓库，规格文档不做 commit。
