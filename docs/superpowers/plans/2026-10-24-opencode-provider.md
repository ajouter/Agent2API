# OpenCode 内置提供商 实现计划

> **面向 AI 代理的工作者：** 必需子技能：使用 subagent-driven-development（推荐）或 executing-plans 逐任务实现此计划。步骤使用复选框（`- [ ]`）语法来跟踪进度。

**目标：** 把 opencode.ai（Zen API）做成 Agent2API 的第 18 家内置提供商：粘贴 API Key 添加账号，网关自动注入 `x-opencode-*` 会话头，支持多账号轮转与模型目录远端刷新。

**架构：** 克隆 Command Code（`providers/commandcode/`）的「无状态 + 粘贴 API Key」适配器形态，新增 `providers/opencode/` 模块（endpoints / session / credentials / login / models / adapter 六个子模块）；会话解析逻辑逐行移植参考实现 [LIMTCYT/opencode-go-proxy-for-trae](https://github.com/LIMTCYT/opencode-go-proxy-for-trae) 的 `resolve_session`。注册接线、账号存储、前端表单均仿 Command Code 的既有触点。

**技术栈：** Rust（axum + serde_json + sha2 + getrandom，均已在 `Cargo.toml` 依赖里）；前端 TypeScript（ui-islands）+ 五语 i18n 扁平字典。

**规格：** `docs/superpowers/specs/2026-10-24-opencode-provider-design.md`（已批准）

**约定（全文适用）：**
- 路径缩写：`$P` = `desktop-tauri/src-tauri/server/src/server/core/providers`，`$S` = `desktop-tauri/src-tauri/server/src/server`，`$UI` = `desktop-tauri`。
- **本目录不是 git 仓库，所有 commit 步骤省略**；每个任务以 `cargo test`/`cargo check`（工具链可用时）或逐文件静态自查（不可用时）收尾。
- release 是 `panic=abort`：新代码**零 unwrap/expect/panic**（互斥锁毒锁用 `unwrap_or_else(|poisoned| poisoned.into_inner())`）。
- 注释风格：中文模块头注释，写清「为什么」（参照 commandcode 各文件）。
- 测试命令（工具链可用时）：`cargo test --manifest-path desktop-tauri/src-tauri/server/Cargo.toml opencode`；全量检查：`cargo check --manifest-path desktop-tauri/src-tauri/server/Cargo.toml`。

---

## 文件结构

新建（Rust，全部在 `$P/opencode/`）：
- `mod.rs` — 模块声明与再导出
- `endpoints.rs` — 基址 / 路径 / UA / 会话头常量与构造
- `session.rs` — 会话解析与请求计数（全局会话表；含单测）
- `credentials.rs` — 账号记录 → 凭证快照；脱敏
- `login.rs` — 粘贴归一化 + baseUrl 校验
- `models.rs` — 静态兜底清单（37 项）+ 远端刷新（含单测）
- `adapter.rs` — `ProviderAdapter` 实现（含 classify_error 单测）

新建（账号存储）：
- `$S/core/account_store/opencode_accounts.rs` — 账号存取与公开形态

修改（Rust 接线）：
- `$P/mod.rs` — 模块声明 + `ProviderKind::OpenCode` + `PROVIDERS` + `kind_from_id` + `kind_id`
- `$P/adapter.rs` — `adapter_for` + `implemented_kinds`
- `$P/catalog.rs` — `refresh_meta` 分支
- `$P/catalog_cache.rs` — `SCOPE_OPENCODE` + `ALL_SCOPES`
- `$S/core/account_store/mod.rs` — `pub mod opencode_accounts` + `OPENCODE_PROVIDER_ID`
- `$S/core/account_store/store_view.rs` — 公开形态分支
- `$S/core/account_store/store.rs` — `session_from_record` 的 OpenCode 附加键（`baseUrl`）
- `$S/api/accounts.rs` — `add_account` 分支 + `refresh_account` 提示分支

修改（前端 / 文档）：
- `$UI/ui-islands/src/islands/add-account-configs.ts` — `OPENCODE` 配置 + `BUILTIN_CONFIGS` 追加
- `$UI/ui-islands/src/islands/add-provider-pick.tsx` — `PROVIDER_ICONS` 条目
- `$UI/ui/assets/providers/opencode.png` — 新图标资产（官方 apple-touch-icon）
- `$UI/ui/i18n/{en,ja,ko,pt-BR,zh-Hant}/add-account.js` — 新字符串译文
- `README.md` 与 `docs/README.{en,zh-Hant,ja,ko,pt-BR}.md` — 支持表格 + 「17 家」→「18 家」

---

### 任务 1：`opencode/session.rs`（会话解析，测试先行）

**文件：**
- 创建：`$P/opencode/session.rs`
- 创建：`$P/opencode/mod.rs`（先只挂 `pub mod session;`，后续任务追加）

- [ ] **步骤 1：写 `mod.rs` 骨架**

```rust
//! OpenCode（opencode.ai Zen API）适配实现：**无状态 OpenAI 兼容转发 + 粘贴式 API Key**。
//!
//! ── 上游长什么样（逆向来源：`LIMTCYT/opencode-go-proxy-for-trae` 的
//! 单文件 Python 代理 + opencode.ai 官方文档 <https://opencode.ai/v2/docs/console/go/>）
//! ────────────────────────────────────────────────────────────
//! OpenAI 兼容协议（`POST /zen/go/v1/chat/completions`），凭
//! `Authorization: Bearer <API Key>` 鉴权（opencode.ai 控制台获取，无 OAuth、
//! 无续期）。每请求必须带四个会话头（缺 `x-opencode-session` 上游 400）：
//! `x-opencode-session` / `x-opencode-request` / `x-opencode-client: cli` /
//! `x-opencode-project: global`，另注入 UA `opencode/1.18.29 cli`。
//! 模型目录 `GET /zen/go/v1/models`（公开、无需鉴权，2026-10-24 实测）。
//!
//! ── 与参考实现的差别（原生的改进，别当漏抄）────────────────────
//!   - 参考代理要用户按模型手填不同 Base URL（当时两条路由清单不同）；
//!     2026-10-24 实测两条路由的 `/models` 已完全一致，故固定走推荐的
//!     `/zen/go/v1`（新路由 `/inference/go/v1` 间歇性 429，参考实现同口径）；
//!   - 会话键与账号无关：网关多账号轮转时同一会话仍映射同一个 session id，
//!     上游提示词缓存路由不受换号影响（参考代理单 key，无此问题也无此能力）；
//!   - 参考代理的心跳保活 / 竞速重试 / 429 指数退避不移植：编排层
//!     （`upstream::provider_loop`）已有重试、轮转与冷却，多账号场景下
//!     「换号重试」优于「同号死等」；`proxy-` 模型别名同理不移植
//!     （网关有自己的模型映射体系）。
//!
//! ── 子模块分工 ──────────────────────────────────────────────
//! ```text
//!   endpoints.rs    基址 / 路径 / UA / 四个会话头的构造
//!   session.rs      会话解析与请求计数（x-opencode-session / x-opencode-request）
//!   credentials.rs  账号记录 → 凭证快照；脱敏
//!   login.rs        粘贴归一化 + baseUrl 校验
//!   models.rs       GET /zen/go/v1/models（5 分钟缓存 + 内置 37 项兜底）
//!   adapter.rs      ProviderAdapter 实现（装配层）
//! ```
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本目录零 unwrap/expect/panic。

pub mod session;
```

- [ ] **步骤 2：写 `session.rs` 的失败测试**

在同文件底部追加（先于实现写好）：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderMap;
    use serde_json::json;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// 每个用例独立的会话键（全局表跨用例共享，键不能撞）
    static CASE_SEQ: AtomicU64 = AtomicU64::new(1);
    fn unique(text: &str) -> String {
        format!("{text}-{}", CASE_SEQ.fetch_add(1, Ordering::Relaxed))
    }
    fn headers_with(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut headers = HeaderMap::new();
        for (name, value) in pairs {
            headers.insert(
                axum::http::HeaderName::from_bytes(name.as_bytes()).expect_static(),
                axum::http::HeaderValue::from_str(value).expect_static(),
            );
        }
        headers
    }
    /// 测试专用的免 panic 构造（axum 的 FromBytes 对合法名字不会失败）
    trait ExpectStatic: Sized {
        fn expect_static(self) -> Self;
    }
    impl ExpectStatic for axum::http::HeaderName {
        fn expect_static(self) -> Self { self }
    }
    impl ExpectStatic for axum::http::HeaderValue {
        fn expect_static(self) -> Self { self }
    }

    #[test]
    fn normalize_rejects_bad_ids() {
        assert_eq!(normalize_session_id(Some("ses-abc_1.0:x")), Some("ses-abc_1.0:x".to_string()));
        assert_eq!(normalize_session_id(Some("  spaced  ")), Some("spaced".to_string()));
        assert_eq!(normalize_session_id(Some("")), None);
        assert_eq!(normalize_session_id(Some("   ")), None);
        assert_eq!(normalize_session_id(Some("bad id!")), None);
        assert_eq!(normalize_session_id(None), None);
        let long = "a".repeat(MAX_ID_LENGTH + 1);
        assert_eq!(normalize_session_id(Some(&long)), None);
        let edge = "a".repeat(MAX_ID_LENGTH);
        assert_eq!(normalize_session_id(Some(&edge)), Some(edge));
    }

    #[test]
    fn header_session_wins_and_counts() {
        let sid = unique("hdr");
        let headers = headers_with(&[
            ("x-opencode-session", &sid),
            ("x-session-id", "should-not-be-used"),
        ]);
        let body = json!({"messages": [{"role": "user", "content": "hi"}]});
        let first = resolve(&body, &headers);
        let second = resolve(&body, &headers);
        assert_eq!(first.source, "header");
        assert_eq!(first.session_id, sid);
        assert_eq!(first.request_id, "msg_1");
        assert_eq!(second.request_id, "msg_2");
        assert_eq!(second.session_id, sid);
    }

    #[test]
    fn body_session_field_is_second_priority() {
        let sid = unique("body");
        let body = json!({"metadata": {"sessionId": sid}, "messages": []});
        let resolved = resolve(&body, &HeaderMap::new());
        assert_eq!(resolved.source, "body");
        assert_eq!(resolved.session_id, sid);
    }

    #[test]
    fn illegal_body_field_falls_through_to_hash() {
        let body = json!({
            "session_id": "not legal / id",
            "system": "you are a helper",
            "messages": [{"role": "user", "content": "hello"}],
        });
        let resolved = resolve(&body, &HeaderMap::new());
        assert_eq!(resolved.source, "content-hash");
        // 同一内容（哪怕后续消息变多）→ 同一 session id；首条 user 变了 → 不同
        let mut grown = body.clone();
        grown["messages"].as_array_mut().unwrap_or_else(|| panic_static()).push(json!({"role": "user", "content": "more"}));
        let again = resolve(&grown, &HeaderMap::new());
        assert_eq!(again.session_id, resolved.session_id);
        let changed = json!({
            "system": "you are a helper",
            "messages": [{"role": "user", "content": "different"}],
        });
        let other = resolve(&changed, &HeaderMap::new());
        assert_ne!(other.session_id, resolved.session_id);
    }

    #[test]
    fn empty_body_yields_random() {
        let resolved = resolve(&json!({}), &HeaderMap::new());
        assert_eq!(resolved.source, "random");
        assert_eq!(resolved.request_id, "msg_1");
        assert!(resolved.session_id.starts_with("ses_"));
        // 随机档不入会话表：再来一次还是 msg_1
        let again = resolve(&json!({}), &HeaderMap::new());
        assert_eq!(again.request_id, "msg_1");
    }

    #[test]
    fn conversation_key_ignores_trailing_messages() {
        let base = json!({
            "system": ["rule one", {"type": "text", "text": "rule two"}],
            "messages": [{"role": "user", "content": [{"type": "text", "text": "hi"}]}],
        });
        let key_a = conversation_key(&base).unwrap_or_else(|| panic_static());
        let mut later = base.clone();
        later["messages"].as_array_mut().unwrap_or_else(|| panic_static())
            .push(json!({"role": "assistant", "content": "reply"}));
        assert_eq!(conversation_key(&later), Some(key_a));
        assert_ne!(conversation_key(&json!({"messages": [{"role": "user", "content": "x"}]})), Some(key_a));
        // 全空 → None
        assert_eq!(conversation_key(&json!({"messages": []})), None);
    }

    #[test]
    fn table_trims_oldest_beyond_cap() {
        // 灌满 MAX_SESSIONS 条新会话，最早的会被裁掉（再命中会拿到新计数）
        let mut first_key = String::new();
        for index in 0..(MAX_SESSIONS + 8) {
            let sid = unique("trim");
            if index == 0 {
                first_key = sid.clone();
            }
            let headers = headers_with(&[("x-opencode-session", &sid)]);
            resolve(&json!({}), &headers);
        }
        let headers = headers_with(&[("x-opencode-session", first_key.as_str())]);
        let revived = resolve(&json!({}), &headers);
        // 被裁掉后重建，从 msg_1 重新计数
        assert_eq!(revived.request_id, "msg_1");
    }
}

fn panic_static() -> serde_json::Value { serde_json::Value::Null }
```

注意：`unwrap_or_else(|| panic_static())` 在数组断言处语义不对（拿 Null 会 panic 且信息差）——改为用 `if let` + `assert!` 的等价写法（实现步骤里照此调整，保持零 panic 原则）：

```rust
        let Some(messages) = grown.as_array_mut() else {
            panic_labeled("grown messages");
            return;
        };
```

其中 `fn panic_labeled(label: &str) -> ! { unreachable!("test setup broken: {label}") }` 放测试模块内（`#[cfg(test)]` 代码允许 panic，主代码不允许——该函数只存在于测试模块）。

- [ ] **步骤 3：运行确认失败**

```bash
cargo test --manifest-path desktop-tauri/src-tauri/server/Cargo.toml opencode::session
```
预期：编译错误（`session` 模块还不存在实现，或函数未定义）。工具链不可用时改为静态自查（步骤 5）。

- [ ] **步骤 4：实现 `session.rs`**

```rust
//! OpenCode 会话解析：`x-opencode-session` / `x-opencode-request` 的取值与递增。
//!
//! ── 上游为什么要这两个头（参考 `LIMTCYT/opencode-go-proxy-for-trae` 的
//! `resolve_session`，本文件是它的 Rust 移植）────────────────────
//! Zen API 按会话路由提示词缓存：`x-opencode-session` 在**同一会话内必须稳定**，
//! `x-opencode-request` 在会话内**递增**（msg_1、msg_2…）。缺前者上游直接 400：
//! `Request is missing x-opencode-session and cannot be routed efficiently`。
//!
//! ── 会话键的取值链（优先级从高到低，与参考实现逐档一致）────────
//!   1. 入站头：x-opencode-session / x-session-id / x-conversation-id / x-thread-id
//!   2. 请求体字段：session_id / sessionId / conversation_id / conversationId /
//!      thread_id / threadId（含 metadata.* 同名键）
//!   3. 内容哈希：system + 第一条 user 消息 —— 同一会话这两段相对稳定，
//!      因此同一会话命中同一个 session id（缓存才命中）
//!   4. 以上皆无 → 随机 `ses_` + 26 hex（只保证不 400，不入表、恒 msg_1）
//!
//! ── 与参考实现的两处小差别（别当漏抄）────────────────────────
//!   - 哈希对非字符串值用 `serde_json::to_string`（Python 版对 system 数组元素
//!     用 `str()`，序列化方言不同）。哈希只在本网关内部作键，不与参考代理
//!     互通，字节级一致无关紧要；
//!   - `source` 简化为四档静态标签（参考实现是 `header:{名字}` 动态串），
//!     日志粒度够用。
//!
//! ── 状态表 ──────────────────────────────────────────────────
//! 全局表上限 256 条（参考实现 `MAX_SESSIONS`，防泄漏），超限裁最旧
//! （HashMap 无序，插入序另用 VecDeque 记；重复命中不刷新次序，与 Python
//! dict 语义一致）。会话键与账号无关：换账号轮转不影响上游缓存路由。

use std::collections::{HashMap, VecDeque};
use std::sync::{Mutex, OnceLock};

use axum::http::HeaderMap;
use serde_json::Value;
use sha2::{Digest, Sha256};

/// 会话表上限（参考实现 `MAX_SESSIONS`）
const MAX_SESSIONS: usize = 256;
/// 会话 id 合法长度上限（参考实现 128，按**字符**数）
const MAX_ID_LENGTH: usize = 128;
/// 随机 id 的十六进制长度（`ses_` 之后 26 个 hex，参考实现 `token_hex(16)[:26]`）
const RANDOM_HEX_LENGTH: usize = 26;

/// 一条会话的共享状态：稳定的 session id + 会话内递增的请求号
struct SessionRecord {
    session_id: String,
    req_num: u64,
}

/// 全局会话表：`map` 存记录、`order` 记插入序（裁最旧用）。
struct SessionTable {
    map: HashMap<String, SessionRecord>,
    order: VecDeque<String>,
}

static TABLE: OnceLock<Mutex<SessionTable>> = OnceLock::new();

fn table() -> &'static Mutex<SessionTable> {
    TABLE.get_or_init(|| {
        Mutex::new(SessionTable {
            map: HashMap::new(),
            order: VecDeque::new(),
        })
    })
}

/// 一次请求解析出的会话头取值
pub struct ResolvedSession {
    /// `x-opencode-session` 的取值
    pub session_id: String,
    /// `x-opencode-request` 的取值（`msg_{n}`）
    pub request_id: String,
    /// 命中的档位（日志用）：header / body / content-hash / random
    pub source: &'static str,
}

/// 入站会话 id 的合法性（参考 `normalize_session_id`）：
/// 非空、≤128 字符、仅 `[A-Za-z0-9_.:-]`；不合法视为未提供。
fn normalize_session_id(value: Option<&str>) -> Option<String> {
    let value = value?.trim();
    if value.is_empty() || value.chars().count() > MAX_ID_LENGTH {
        return None;
    }
    let legal = value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.' | ':' | '-'));
    legal.then(|| value.to_string())
}

/// 非字符串值的锚定文本（哈希键用；见模块头「小差别」第 1 条）
fn anchor_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => serde_json::to_string(other).unwrap_or_default(),
    }
}

/// 内容哈希会话键（档 3，参考 `conversation_key_from_body`）：
/// system（数组逐项换行拼接）+ 第一条 user 消息的 content，SHA-256。
pub(crate) fn conversation_key(body: &Value) -> Option<String> {
    let mut anchor = String::new();
    match body.get("system") {
        Some(Value::Array(items)) => {
            let joined = items
                .iter()
                .map(anchor_text)
                .collect::<Vec<_>>()
                .join("\n");
            anchor.push_str(&joined);
        }
        Some(other) if !other.is_null() => anchor.push_str(&anchor_text(other)),
        _ => {}
    }
    if let Some(messages) = body.get("messages").and_then(Value::as_array) {
        if let Some(first_user) = messages
            .iter()
            .find(|message| message.get("role").and_then(Value::as_str) == Some("user"))
        {
            anchor.push('\n');
            if let Some(content) = first_user.get("content") {
                anchor.push_str(&anchor_text(content));
            }
        }
    }
    if anchor.trim().is_empty() {
        return None;
    }
    let mut hasher = Sha256::new();
    hasher.update(anchor.as_bytes());
    Some(format!("k_{:x}", hasher.finalize()))
}

/// 显式会话探测（档 1 头 + 档 2 体字段，参考 `extract_explicit_session`）。
fn explicit_session(body: &Value, headers: &HeaderMap) -> Option<String> {
    for name in [
        "x-opencode-session",
        "x-session-id",
        "x-conversation-id",
        "x-thread-id",
    ] {
        let value = headers
            .get(name)
            .and_then(|header| header.to_str().ok());
        if let Some(found) = normalize_session_id(value) {
            return Some(found);
        }
    }
    if !body.is_object() {
        return None;
    }
    let mut probes: Vec<&Value> = Vec::new();
    for key in [
        "session_id",
        "sessionId",
        "conversation_id",
        "conversationId",
        "thread_id",
        "threadId",
    ] {
        probes.push(body.get(key).unwrap_or(&Value::Null));
    }
    if let Some(metadata) = body.get("metadata").filter(|value| value.is_object()) {
        for key in ["session_id", "sessionId", "conversation_id", "conversationId"] {
            probes.push(metadata.get(key).unwrap_or(&Value::Null));
        }
    }
    probes
        .iter()
        .filter_map(|value| value.as_str())
        .find_map(|value| normalize_session_id(Some(value)))
}

/// `ses_` + 26 个十六进制字符（参考 `random_id`）。
fn random_id(prefix: &str) -> String {
    let mut bytes = [0u8; 16];
    if getrandom::getrandom(&mut bytes).is_err() {
        // 熵源失败（极罕见）：退化为时间派生，只求「这个进程内不重复」——
        // 上游对 session id 的要求就是「同一会话内稳定」，随机档每次都新，
        // 两个请求撞 id 的后果只是共享一次缓存路由，不是正确性问题。
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        let mut hasher = Sha256::new();
        hasher.update(nanos.to_le_bytes());
        hasher.update(format!("{:p}", prefix).as_bytes());
        let digest = hasher.finalize();
        bytes.copy_from_slice(&digest[..16]);
    }
    let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    format!("{prefix}_{}", &hex[..RANDOM_HEX_LENGTH])
}

/// 命中会话表的一档：取记录（没有则建），计数 +1，超限裁最旧。
fn bump(key: &str, new_session_id: impl FnOnce() -> String, source: &'static str) -> ResolvedSession {
    let mut guard = table()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if !guard.map.contains_key(key) {
        guard.map.insert(
            key.to_string(),
            SessionRecord {
                session_id: new_session_id(),
                req_num: 0,
            },
        );
        guard.order.push_back(key.to_string());
        while guard.map.len() > MAX_SESSIONS {
            let Some(oldest) = guard.order.pop_front() else {
                break;
            };
            guard.map.remove(&oldest);
        }
    }
    let Some(record) = guard.map.get_mut(key) else {
        // 理论不可达（上面刚插入过）；兜底成随机档，不给调用方 panic 的理由
        return ResolvedSession {
            session_id: random_id("ses"),
            request_id: "msg_1".to_string(),
            source: "random",
        };
    };
    record.req_num += 1;
    ResolvedSession {
        session_id: record.session_id.clone(),
        request_id: format!("msg_{}", record.req_num),
        source,
    }
}

/// 解析本次请求的会话（见模块头取值链）。
pub fn resolve(body: &Value, headers: &HeaderMap) -> ResolvedSession {
    if let Some(session_id) = explicit_session(body, headers) {
        let key = format!("explicit:{session_id}");
        return bump(&key, || session_id.clone(), "header");
    }
    if let Some(key) = conversation_key(body) {
        return bump(&key, || random_id("ses"), "content-hash");
    }
    ResolvedSession {
        session_id: random_id("ses"),
        request_id: "msg_1".to_string(),
        source: "random",
    }
}
```

（随后把步骤 2 的 `#[cfg(test)] mod tests` 追加到文件底部，并把其中的数组取值改为 `if let … else panic_labeled()` 写法。）

- [ ] **步骤 5：运行确认通过 / 静态自查**

```bash
cargo test --manifest-path desktop-tauri/src-tauri/server/Cargo.toml opencode::session
```
预期：全部 PASS。工具链不可用时：对照步骤 4 逐行自查（重点是取值链顺序、裁剪循环、零 panic），并在任务 9 编译时统一验证。

---

### 任务 2：`opencode/endpoints.rs` + `opencode/credentials.rs` + `opencode/login.rs`

**文件：**
- 创建：`$P/opencode/endpoints.rs`
- 创建：`$P/opencode/credentials.rs`
- 创建：`$P/opencode/login.rs`
- 修改：`$P/opencode/mod.rs`（挂三个 `pub mod`）

- [ ] **步骤 1：`endpoints.rs`（常量与头构造）**

```rust
//! OpenCode 上游地址与请求头常量。
//!
//! ── 路由的取舍（规格 §3.1，2026-10-24 实测）────────────────────
//! 参考实现注释说「老模型只在 /zen/go/v1、新模型只在 /inference/go/v1」，
//! 但实测两条路由的 `GET /models` 已返回完全相同的 37 个模型 —— 那段注释
//! 已过时。固定走参考实现推荐的 `/zen/go/v1`（新路由间歇性 429），
//! 不做按模型选路；模型与路由再分叉时把 `CHAT_PATH` 改成查表即可。
//!
//! ── UA 伪装 ────────────────────────────────────────────────
//! 上游按客户端形态识别流量：注入 `opencode/1.18.29 cli`（参考实现的默认值）。
//! 客户端自称 opencode（UA 含 "opencode"，大小写不敏感）时透传其 UA ——
//! 用 opencode CLI 直连网关的用户保持自己的版本号，比伪装成旧版更真实。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic。

use axum::http::HeaderMap;

/// 默认基址（账号可用 `baseUrl` 覆盖，见 `base_url`）
pub const DEFAULT_BASE_URL: &str = "https://opencode.ai";

/// 聊天生成路径（推荐老路由，见模块头）
pub const CHAT_PATH: &str = "/zen/go/v1/chat/completions";

/// 模型目录路径（公开端点，2026-10-24 实测无需鉴权）
pub const MODELS_PATH: &str = "/zen/go/v1/models";

/// 注入的 User-Agent 常量（参考实现 `OPENCODE_UA` 的默认值）
pub const CLIENT_USER_AGENT: &str = "opencode/1.18.29 cli";

/// 基址解析：账号级覆盖（去尾部斜杠）→ 默认。
pub fn base_url(account_override: Option<&str>) -> String {
    account_override
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.trim_end_matches('/').to_string())
        .unwrap_or_else(|| DEFAULT_BASE_URL.to_string())
}

/// 上游要求注入的四个会话/身份头（参考实现的注入表，一字不差）。
pub fn opencode_headers(session_id: &str, request_id: &str) -> Vec<(String, String)> {
    vec![
        ("x-opencode-session".to_string(), session_id.to_string()),
        ("x-opencode-request".to_string(), request_id.to_string()),
        ("x-opencode-client".to_string(), "cli".to_string()),
        ("x-opencode-project".to_string(), "global".to_string()),
    ]
}

/// UA 头：客户端自称 opencode 时透传，否则注入 CLI 常量（见模块头）。
pub fn user_agent_header(client_headers: &HeaderMap) -> (String, String) {
    let inbound = client_headers
        .get(axum::http::header::USER_AGENT)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty());
    match inbound.filter(|value| value.to_lowercase().contains("opencode")) {
        Some(value) => ("User-Agent".to_string(), value.to_string()),
        None => ("User-Agent".to_string(), CLIENT_USER_AGENT.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderMap;

    #[test]
    fn base_url_overrides_and_trims() {
        assert_eq!(base_url(None), DEFAULT_BASE_URL);
        assert_eq!(base_url(Some("")), DEFAULT_BASE_URL);
        assert_eq!(base_url(Some("  ")), DEFAULT_BASE_URL);
        assert_eq!(base_url(Some("http://127.0.0.1:9000/")), "http://127.0.0.1:9000");
        assert_eq!(base_url(Some(" https://mirror.example.com ")), "https://mirror.example.com");
    }

    #[test]
    fn user_agent_passthrough_needs_opencode() {
        let mut headers = HeaderMap::new();
        headers.insert("user-agent", "my-client/1.0".parse().unwrap_or(axum::http::HeaderValue::from_static("my-client/1.0")));
        assert_eq!(user_agent_header(&headers).1, CLIENT_USER_AGENT);
        let mut headers = HeaderMap::new();
        headers.insert("user-agent", "OpenCode/1.19 cli".parse().unwrap_or(axum::http::HeaderValue::from_static("opencode")));
        assert_eq!(user_agent_header(&headers).1, "OpenCode/1.19 cli");
        let empty = HeaderMap::new();
        assert_eq!(user_agent_header(&empty).1, CLIENT_USER_AGENT);
    }
}
```

注意测试里 `.parse().unwrap_or(...)` 的写法在 `HeaderValue` 上可用；实现时若觉得啰嗦可改用 `HeaderValue::from_static`（测试代码允许 unwrap，这里保持统一无 unwrap）。

- [ ] **步骤 2：`credentials.rs`（凭证快照与脱敏）**

```rust
//! OpenCode 凭证：单个粘贴的 API Key（无 OAuth、无续期）。
//!
//! ── 上游的凭证形态 ──────────────────────────────────────────
//! 一枚不透明字符串（opencode.ai 控制台获取）。上游没有公开的 key 形态约定
//! （参考实现原样透传 Authorization、不做前缀校验），本模块同样**不做形态
//! 检查**，只保证「非空、不超长」。上游没有 refresh 接口 → `supports_refresh
//! = false`。
//!
//! ── 账号级 `baseUrl` ────────────────────────────────────────
//! 记录上可选的 `baseUrl`（自建 / 镜像网关）。取值链兼容会话形态与会话顶层
//! 注入（`store.rs::session_from_record` 的 OpenCode 附加键）两种落点。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic。

use serde_json::Value;

use crate::server::errors::GatewayError;

use super::endpoints;

/// Key 长度上限（超过一定是粘错了东西；与别家同一量级）
pub const MAX_KEY_LENGTH: usize = 8192;

/// 展示用的脱敏形态（只露尾 4 位；日志与报错文案都用它）
pub fn masked(key: &str) -> String {
    let chars: Vec<char> = key.trim().chars().collect();
    if chars.len() <= 8 {
        return "…".to_string();
    }
    let tail: String = chars[chars.len() - 4..].iter().collect();
    format!("…{tail}")
}

/// 从一条取值链里挑第一个非空字符串（记录可能被手工编辑过，宽容读取）
fn pick(record: &Value, keys: &[&str]) -> String {
    for key in keys {
        let value = record
            .get(*key)
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or("");
        if !value.is_empty() {
            return value.to_string();
        }
    }
    String::new()
}

/// 一次转发 / 目录刷新用的凭证快照
#[derive(Clone, Debug)]
pub struct OpenCodeCredentials {
    /// 账号记录 id
    pub id: String,
    /// 上游 API Key（明文）
    pub api_key: String,
    /// 上游基址（账号级覆盖后的最终值，已去尾斜杠）
    pub base_url: String,
    /// 展示名（记录的备注名）
    pub name: String,
}

/// 从记录解析凭证（`record` 为 None 时报「没有可用账号」）。
///
/// 兼容两种形态：**原始记录**（`account_record`，key 在顶层 `accessToken`）
/// 与**会话**（`build_chat_request` 收到的形状，key 在 `auth.accessToken`）。
/// `baseUrl` 取值链：`baseUrl` → `apiBase` → `base_url`（会话形态下由
/// `store.rs` 的 OpenCode 附加键注入顶层）。
pub fn from_record(record: Option<&Value>) -> Result<OpenCodeCredentials, GatewayError> {
    let Some(record) = record else {
        return Err(GatewayError::with_status(
            401,
            "没有可用的 OpenCode 账号：请在账号页粘贴 API Key（opencode.ai 控制台获取）添加",
        ));
    };
    let api_key = pick(
        record,
        &["apiKey", "accessToken", "token", "key"],
    );
    if api_key.is_empty() {
        // 会话形态下顶层可能没有，再看 auth.accessToken
        let nested = record
            .get("auth")
            .map(|auth| pick(auth, &["accessToken", "apiKey", "token", "key"]))
            .unwrap_or_default();
        if nested.is_empty() {
            return Err(GatewayError::with_status(
                401,
                "OpenCode 账号缺少 API Key，请重新粘贴（opencode.ai 控制台获取）",
            ));
        }
        return finish(record, nested);
    }
    finish(record, api_key)
}

fn finish(record: &Value, api_key: String) -> Result<OpenCodeCredentials, GatewayError> {
    if api_key.chars().count() > MAX_KEY_LENGTH {
        return Err(GatewayError::with_status(
            400,
            format!("OpenCode API Key 过长（{}）：请检查是否粘错了内容", masked(&api_key)),
        ));
    }
    let override_value = pick(record, &["baseUrl", "apiBase", "base_url"]);
    let override_option = if override_value.is_empty() {
        None
    } else {
        Some(override_value.as_str())
    };
    Ok(OpenCodeCredentials {
        id: pick(record, &["id"]),
        api_key,
        base_url: endpoints::base_url(override_option),
        name: pick(record, &["name"]),
    })
}
```

等等——顶层 `apiKey` 链已包含 `accessToken`，而会话形态顶层**没有**这些键（在 `auth` 下），顶层链拿空串后走 `auth` 分支，正确。但原始记录形态下顶层 `accessToken` 有值、`auth` 不存在，走主链，正确。此取法两态兼容。

- [ ] **步骤 3：`login.rs`（粘贴归一化）**

```rust
//! OpenCode 粘贴归一化：剥引号 / `Authorization: Bearer` 前缀 / 空白 + baseUrl 校验。
//!
//! 上游对 key 没有可判的本地形态（见 `credentials` 的模块头），所以这里**不做
//! 前缀校验**：归一化后非空即通过，真实有效性由上游生成请求时的 401/403 回答
//! （`classify_error` 的 TokenExpired 档）。也没有 Command Code 那种添加探活 ——
//! 本家没有免费只读端点可探（`/models` 公开、不校验 key，探不出无效）。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic。

use serde_json::{json, Value};

use crate::server::errors::GatewayError;

use super::credentials;

/// 归一化一段粘贴内容：剥成对引号 / `Authorization: Bearer` 前缀 / 空白。
pub fn normalize_paste(raw: &str) -> String {
    let mut value = raw.trim();
    for (open, close) in [("\"", "\""), ("'", "'")] {
        if value.len() >= 2 && value.starts_with(open) && value.ends_with(close) {
            value = value[1..value.len() - 1].trim();
        }
    }
    // 整行 `Authorization: Bearer …` / 裸 `Bearer …`（大小写不敏感）
    let mut candidate = value;
    for prefix in ["authorization:", "bearer"] {
        let lowered = candidate.to_lowercase();
        if let Some(rest) = lowered.strip_prefix(prefix) {
            let skipped = candidate.len() - rest.len();
            candidate = candidate[skipped..].trim();
            break;
        }
    }
    // `Authorization:` 后若还有冒号残留的空白已由 trim 吃掉；`Bearer` 后同理。
    // 上面只剥一层：`Authorization: Bearer x` 的 `authorization:` 剥完剩
    // `Bearer x`，再剥一层才算干净 —— 所以循环一次不够，来第二轮：
    let mut candidate = candidate;
    let lowered = candidate.to_lowercase();
    if let Some(rest) = lowered.strip_prefix("bearer") {
        let skipped = candidate.len() - rest.len();
        candidate = candidate[skipped..].trim();
    }
    candidate.to_string()
}

/// 校验并归一化添加账号的载荷（`api/accounts.rs` 的 OpenCode 分支用）。
///
/// 返回 `{apiKey, baseUrl?}`（`add_opencode_account` 的 payload 形状）。
/// `base_url` 给了就必须是 http(s) URL —— 这是唯一的本地校验，
/// 拼错地址的症状是上游 404/连不上，添加时拦一次省得用户排查。
pub fn verify_payload(raw: &str, base_url: Option<&str>) -> Result<Value, GatewayError> {
    let api_key = normalize_paste(raw);
    if api_key.is_empty() {
        return Err(GatewayError::with_status(
            400,
            "缺少 API Key：请粘贴 opencode.ai 控制台获取的 API Key",
        ));
    }
    if api_key.chars().count() > credentials::MAX_KEY_LENGTH {
        return Err(GatewayError::with_status(400, "API Key 过长"));
    }
    let mut payload = json!({ "apiKey": api_key });
    if let Some(base) = base_url.map(str::trim).filter(|value| !value.is_empty()) {
        let trimmed = base.trim_end_matches('/');
        if !trimmed.starts_with("http://") && !trimmed.starts_with("https://") {
            return Err(GatewayError::with_status(
                400,
                "自定义 API 地址必须是 http(s):// 开头的完整地址（不要带 /v1 之类的路径后缀）",
            ));
        }
        payload["baseUrl"] = Value::String(trimmed.to_string());
    }
    Ok(payload)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_strips_wrapping_and_prefixes() {
        assert_eq!(normalize_paste("  sk-abc  "), "sk-abc");
        assert_eq!(normalize_paste("\"sk-abc\""), "sk-abc");
        assert_eq!(normalize_paste("'sk-abc'"), "sk-abc");
        assert_eq!(normalize_paste("Bearer sk-abc"), "sk-abc");
        assert_eq!(normalize_paste("bearer sk-abc"), "sk-abc");
        assert_eq!(normalize_paste("Authorization: Bearer sk-abc"), "sk-abc");
        assert_eq!(normalize_paste("authorization:bearer\tsk-abc"), "sk-abc");
        assert_eq!(normalize_paste(""), "");
    }

    #[test]
    fn verify_payload_validates_base_url() {
        let plain = verify_payload("sk-abc", None).unwrap_or_default();
        assert_eq!(plain["apiKey"], "sk-abc");
        assert!(plain.get("baseUrl").is_none());
        let with_base =
            verify_payload("sk-abc", Some("https://mirror.example.com/")).unwrap_or_default();
        assert_eq!(with_base["baseUrl"], "https://mirror.example.com");
        assert!(verify_payload("sk-abc", Some("ftp://x")).is_err());
        assert!(verify_payload("", None).is_err());
    }
}
```

注意 `normalize_paste` 里剥 `authorization:` 前缀的逻辑有个坑：`lowered.strip_prefix(prefix)` 得到的 `rest` 长度与 `candidate` 的字节偏移换算依赖「前缀本身是 ASCII」（大小写转换不改字节长度，成立）。两轮剥离（`authorization:` → `bearer`）已在实现里写明。

- [ ] **步骤 4：更新 `mod.rs` 挂载子模块**

在 `$P/opencode/mod.rs` 的 `pub mod session;` 处改为：

```rust
pub mod credentials;
pub mod endpoints;
pub mod login;
pub mod models;
pub mod session;
```

（`models` / `adapter` 在任务 3/4 创建，先注释占位会导致编译错——所以本步骤把 `pub mod models;` 与 `pub mod adapter;` **留到对应任务再加**，此处只挂已存在的四个。）

- [ ] **步骤 5：运行测试 / 静态自查**

```bash
cargo test --manifest-path desktop-tauri/src-tauri/server/Cargo.toml opencode
```
预期：endpoints 与 login 的测试 PASS。

---

### 任务 3：`opencode/models.rs`（静态兜底 + 远端刷新）

**文件：**
- 创建：`$P/opencode/models.rs`
- 修改：`$P/catalog_cache.rs`（`SCOPE_OPENCODE` + `ALL_SCOPES`，本任务先行，models 要用）
- 修改：`$P/opencode/mod.rs`（挂 `pub mod models;`）

- [ ] **步骤 1：`catalog_cache.rs` 加 scope**

在 `$P/catalog_cache.rs` 的 `SCOPE_ANTIGRAVITY` 之后追加：

```rust
/// OpenCode（`GET /zen/go/v1/models`；单一域名、无地区之分，单格即可）
pub const SCOPE_OPENCODE: &str = "opencode";
```

并把 `SCOPE_OPENCODE,` 追加进 `ALL_SCOPES` 数组末尾（`SCOPE_ANTIGRAVITY,` 之后）。

- [ ] **步骤 2：写 `models.rs`（结构克隆 `commandcode/models.rs`，静态表 37 项）**

```rust
//! OpenCode 模型目录：`GET /zen/go/v1/models` + 5 分钟缓存 + 内置 37 项兜底。
//!
//! ── 上游形态（2026-10-24 实测）────────────────────────────────
//! ```text
//!   { "object": "list", "data": [ { "id": "glm-5.3", "object": "model",
//!     "created": 1792819125, "owned_by": "opencode" }, … ] }
//! ```
//! 端点**公开、无需鉴权**（实测），刷新仍随请求带鉴权头（兼容上游将来收紧）。
//! 参考实现失败时回落内置清单 —— 本模块照做，绝不把目录清空（见
//! `catalog.rs` 的可用性判据：清单为空的家不进 `/v1/models`）。
//!
//! ── 能力位 ──────────────────────────────────────────────────
//! 上游目录不返回结构化能力位。工具调用与思考位照 commandcode 的口径给 true
//! （OpenAI 兼容上游的通用形态）；图片位没有目录依据，**全部 conservative
//! false** —— 报错方向只能选「少报」（models.dev 的元数据补全会按模型 id
//! 自动覆盖此处未声明的键，无需本文件操心）。
//!
//! ── 缓存 ────────────────────────────────────────────────────
//! 内存缓存 + `providers::catalog_cache` 落盘（scope `SCOPE_OPENCODE`）。
//! 自动路径 5 分钟 TTL；手动刷新 `force = true` 绕过。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic。

use std::sync::{Mutex, OnceLock};

use serde_json::{json, Value};

use crate::server::core::auth_http::send_raw;
use crate::server::core::providers::catalog_cache;

use super::endpoints;

/// 本家在 `catalog_cache` 里的 scope 名
pub const SCOPE: &str = catalog_cache::SCOPE_OPENCODE;

/// 自动路径的缓存 TTL（与 commandcode 同口径）
const TTL_MS: i64 = 5 * 60 * 1000;

/// 目录查询超时
const REQUEST_TIMEOUT_MS: u64 = 10_000;

/// 内置兜底清单（2026-10-24 实测 `GET /zen/go/v1/models` 的 37 项）。
/// id 逐字保留（改一个字母就等于换模型）；展示名是本仓给的易读形式。
const FALLBACK_MODELS: [(&str, &str); 37] = [
    ("deepseek-v4.1-flash", "DeepSeek V4.1 Flash"),
    ("deepseek-v4-pro", "DeepSeek V4 Pro"),
    ("glm-5.1", "GLM 5.1"),
    ("glm-5.2", "GLM 5.2"),
    ("glm-5.3", "GLM 5.3"),
    ("glm-5.3-flash", "GLM 5.3 Flash"),
    ("grok-4.6", "Grok 4.6"),
    ("grok-4.7", "Grok 4.7"),
    ("muse-spark-1.2-contributor", "Muse Spark 1.2 Contributor"),
    ("muse-spark-1.3-contributor", "Muse Spark 1.3 Contributor"),
    ("omen-alpha", "Omen Alpha"),
    ("gpt-5.6-luna", "GPT-5.6 Luna"),
    ("gpt-6-luna", "GPT-6 Luna"),
    ("claude-haiku-5-5", "Claude Haiku 5.5"),
    ("hy3", "HY3"),
    ("hy4-preview", "HY4 Preview"),
    ("kimi-k2.6", "Kimi K2.6"),
    ("kimi-k2.7-code", "Kimi K2.7 Code"),
    ("kimi-k3", "Kimi K3"),
    ("kimi-k3.1-pro", "Kimi K3.1 Pro"),
    ("mimo-v2.5", "MiMo V2.5"),
    ("mimo-v2.6-flash", "MiMo V2.6 Flash"),
    ("mimo-v2.5-pro", "MiMo V2.5 Pro"),
    ("mimo-v2.6-pro", "MiMo V2.6 Pro"),
    ("minimax-m2.5", "MiniMax M2.5"),
    ("minimax-m2.7", "MiniMax M2.7"),
    ("minimax-m3", "MiniMax M3"),
    ("minimax-m3.1-flash", "MiniMax M3.1 Flash"),
    ("gemini-4-argon-contributor", "Gemini 4 Argon Contributor"),
    ("longcat-2.0", "LongCat 2.0"),
    ("step-5-preview", "Step 5 Preview"),
    ("beam-free", "Beam Free"),
    ("qwen3.6-plus", "Qwen 3.6 Plus"),
    ("qwen3.7-max", "Qwen 3.7 Max"),
    ("qwen3.7-plus", "Qwen 3.7 Plus"),
    ("qwen3.8-max", "Qwen 3.8 Max"),
    ("qwen3.8-flash", "Qwen 3.8 Flash"),
];

/// 进程内缓存（首次访问时从落盘缓存读回）
struct CatalogState {
    models: Vec<Value>,
    fetched_at: i64,
    /// 这份清单是不是**远程拉到**的（落盘缓存装回来的也算）
    remote: bool,
}

static STATE: OnceLock<Mutex<Option<CatalogState>>> = OnceLock::new();

fn state() -> &'static Mutex<Option<CatalogState>> {
    STATE.get_or_init(|| Mutex::new(None))
}

fn with_state<R>(f: impl FnOnce(&mut Option<CatalogState>) -> R) -> R {
    let mutex = state();
    let mut guard = mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if guard.is_none() {
        if let Some(cached) = catalog_cache::load(SCOPE) {
            *guard = Some(CatalogState {
                models: cached.models,
                fetched_at: cached.fetched_at,
                remote: true,
            });
        }
    }
    f(&mut guard)
}

/// 把一个上游目录条目映射成聚合层认的形态（键名对照 `core/models/shape.rs`）。
fn map_entry(row: &Value) -> Option<Value> {
    let id = row
        .get("id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    let name = row
        .get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(id);
    Some(json!({
        "id": id,
        "name": name,
        "supportsToolCall": true,
        "supportsReasoning": true,
        "supportsImages": false,
    }))
}

fn fallback_models() -> Vec<Value> {
    FALLBACK_MODELS
        .iter()
        .map(|(id, name)| {
            json!({
                "id": id,
                "name": name,
                "supportsToolCall": true,
                "supportsReasoning": true,
                "supportsImages": false,
            })
        })
        .collect()
}

/// 解析 `/zen/go/v1/models` 的响应（`{data:[…]}` 或裸数组）
pub fn parse_models(payload: &Value) -> Vec<Value> {
    let rows = payload
        .get("data")
        .and_then(Value::as_array)
        .or_else(|| payload.as_array())
        .cloned()
        .unwrap_or_default();
    rows.iter().filter_map(map_entry).collect()
}

/// 当前模型清单（内存 → 落盘缓存 → 内置兜底；永远非空）
pub fn list() -> Vec<Value> {
    with_state(|slot| {
        slot.as_ref()
            .map(|state| state.models.clone())
            .filter(|models| !models.is_empty())
            .unwrap_or_else(fallback_models)
    })
}

/// 这份清单是否来自**远程拉取**（`catalog::refresh_meta` 的「来源」列判据）
pub fn remote_refreshed() -> bool {
    with_state(|slot| slot.as_ref().map(|state| state.remote).unwrap_or(false))
}

/// 最近一次成功刷新的时刻（毫秒；没刷过为 0）
pub fn last_refreshed_at() -> i64 {
    with_state(|slot| slot.as_ref().map(|state| state.fetched_at).unwrap_or(0))
}

/// 拉一次模型目录并落地（内存 + 持久化缓存）。
///
/// 端点公开（见模块头），鉴权头仍带上以兼容上游将来收紧。
/// 失败时**保留现有清单**，返回 `failed(原因)`。
pub async fn refresh(api_key: &str, base_url: &str, force: bool) -> ModelRefreshOutcome {
    let now = crate::server::logging::now_ms();
    if !force {
        let fresh = with_state(|slot| {
            slot.as_ref()
                .map(|state| state.remote && now - state.fetched_at < TTL_MS)
                .unwrap_or(false)
        });
        if fresh {
            return ModelRefreshOutcome::unchanged();
        }
    }
    let mut headers = vec![
        ("Accept".to_string(), "application/json".to_string()),
        ("User-Agent".to_string(), endpoints::CLIENT_USER_AGENT.to_string()),
    ];
    if !api_key.trim().is_empty() {
        headers.push((
            "Authorization".to_string(),
            format!("Bearer {}", api_key.trim()),
        ));
    }
    let url = format!("{}{}", endpoints::base_url(Some(base_url)), endpoints::MODELS_PATH);
    let response = match send_raw(
        "GET",
        &url,
        None,
        &headers,
        None,
        Some(REQUEST_TIMEOUT_MS),
    )
    .await
    {
        Ok(response) => response,
        Err(error) => {
            return ModelRefreshOutcome::failed(format!(
                "OpenCode 模型目录查询失败：{error}（保留现有清单）"
            ))
        }
    };
    if !response.ok {
        return ModelRefreshOutcome::failed(format!(
            "OpenCode 模型目录返回 HTTP {}（保留现有清单）",
            response.status
        ));
    }
    let payload = response.payload.unwrap_or(Value::Null);
    let models = parse_models(&payload);
    if models.is_empty() {
        return ModelRefreshOutcome::failed("OpenCode 模型目录为空或格式不认识（保留现有清单）");
    }
    let count = models.len();
    with_state(|slot| {
        *slot = Some(CatalogState {
            models: models.clone(),
            fetched_at: now,
            remote: true,
        });
    });
    catalog_cache::save(SCOPE, &models, now);
    ModelRefreshOutcome::refreshed(count)
}

/// 模型目录刷新结果（`ProviderAdapter::refresh_models` 的返回）
pub use crate::server::core::providers::adapter::ModelRefreshOutcome;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_models_accepts_envelope_and_bare_array() {
        let envelope = json!({"data": [{"id": "glm-5.3"}, {"id": "  kimi-k3  ", "name": "Kimi K3"}]});
        let parsed = parse_models(&envelope);
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0]["id"], "glm-5.3");
        assert_eq!(parsed[1]["name"], "Kimi K3");
        let bare = json!([{"id": "hy3"}]);
        assert_eq!(parse_models(&bare).len(), 1);
        assert!(parse_models(&json!({})).is_empty());
        assert!(parse_models(&json!({"data": [{"name": "no-id"}]})).is_empty());
    }

    #[test]
    fn fallback_covers_measured_catalog() {
        assert_eq!(FALLBACK_MODELS.len(), 37);
        let mut ids: Vec<&str> = FALLBACK_MODELS.iter().map(|(id, _)| *id).collect();
        ids.sort_unstable();
        let before = ids.len();
        ids.dedup();
        assert_eq!(ids.len(), before, "兜底表 id 不得重复");
    }
}
```

- [ ] **步骤 3：`mod.rs` 挂 `pub mod models;`，运行测试**

```bash
cargo test --manifest-path desktop-tauri/src-tauri/server/Cargo.toml opencode::models
```

---

### 任务 4：`opencode/adapter.rs`（ProviderAdapter 实现）

**文件：**
- 创建：`$P/opencode/adapter.rs`
- 修改：`$P/opencode/mod.rs`（挂 `pub mod adapter;` + 再导出）

- [ ] **步骤 1：写 `adapter.rs`**

```rust
//! OpenCode 适配器：**无状态**（OpenAI 兼容透传 + 会话头注入）。
//!
//! ── 为什么不上有状态路线 ────────────────────────────────────
//! 上游就是标准 OpenAI Chat：`build_chat_request` 保留请求体、拼头、
//! `ChatRequestPlan::chat` 一条路 —— `is_stateful` 用 trait 默认 false，
//! 不需要 `forward_conversation`（与 OfficeAce 同形态）。
//!
//! ── 会话头注入的落点 ───────────────────────────────────────
//! `build_chat_request` 是同步的、零网络，正适合做会话解析
//! （`session::resolve` 只查内存表）。会话键与账号无关（见 `session.rs`
//! 模块头）：编排层换账号重试时，同一会话仍拿到同一个 session id 与
//! **继续递增**的请求号 —— 上游缓存路由不受轮转影响。
//!
//! ── 错误分类（规格 §3.4）────────────────────────────────────
//!   - 429 → `QuotaLimited{reset_at: None}`（多账号轮转下的正确动作是换号，
//!     「Endpoint is unavailable」的伪 429 同理）；
//!   - 503 且错误体含 `Endpoint is unavailable` → 同样 `QuotaLimited`
//!     （参考实现实测：新路由把端点瞬时故障报成 503、老路由报 429，
//!     错误体相同 —— 固定老路由后这档是兜底，不白留）；
//!   - 401/403 → `TokenExpired`（编排层先原地试一次刷新（本家刷新必然
//!     失败）再换账号 —— 与 Command Code 同一处境）；
//!   - 其余 → 共用的内容拦截判定（`content_block`）。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic。

use axum::http::HeaderMap;
use serde_json::{json, Value};

use crate::server::core::account_store::AccountStore;
use crate::server::core::providers::adapter::{
    ChatRequestPlan, ModelRefreshOutcome, ProviderAdapter, UpstreamErrorClass,
};
use crate::server::core::providers::{content_block, ProviderKind};
use crate::server::errors::GatewayError;
use crate::server::logging;

use super::{credentials, endpoints, models, session};

/// OpenCode 适配器（无状态单例）
pub struct OpenCodeAdapter;

/// 静态单例（`adapter_for(ProviderKind::OpenCode)` 返回这一个）
pub static OPENCODE_ADAPTER: OpenCodeAdapter = OpenCodeAdapter;

/// 流式补 `stream_options.include_usage`（**不覆盖**客户端已有的取值）。
///
/// 与 `officeace::chat::ensure_include_usage` 同一份逻辑：上游只在显式要求时
/// 才回用量帧，不补的话报表的输入/输出列恒为 0。单独复制一份而不是跨 provider
/// 引用：providers 各家之间不互相 import 是本目录的既有边界（zcode 也是自持一份）。
fn ensure_include_usage(body: &mut Value) {
    if body.get("stream").and_then(Value::as_bool) != Some(true) {
        return;
    }
    let Some(object) = body.as_object_mut() else {
        return;
    };
    let options = object
        .entry("stream_options".to_string())
        .or_insert_with(|| json!({}));
    if let Some(options) = options.as_object_mut() {
        options
            .entry("include_usage".to_string())
            .or_insert(Value::Bool(true));
    }
}

impl ProviderAdapter for OpenCodeAdapter {
    fn kind(&self) -> ProviderKind {
        ProviderKind::OpenCode
    }

    /// 模型清单来自远程目录（`models.rs` 的进程缓存 + 落盘缓存 + 内置 37 项兜底）
    fn list_models(&self) -> Vec<Value> {
        models::list()
    }

    /// 构造 `POST {base}/zen/go/v1/chat/completions`：保留请求体、注入会话头。
    ///
    /// `account` 是会话形态（`store.get_session_by_id` 的返回值）：key 从
    /// `auth.accessToken` 取（`credentials::from_record` 兼容会话与原始记录
    /// 两种形态，见其模块头）；`baseUrl` 由 `store.rs` 的 OpenCode 附加键
    /// 注入会话顶层（没有就是官方默认）。
    fn build_chat_request(
        &self,
        account: &Value,
        body: &Value,
        client_headers: &HeaderMap,
    ) -> Result<ChatRequestPlan, GatewayError> {
        let credentials = credentials::from_record(Some(account))?;
        let mut outgoing = body.clone();
        // 用量帧：不补的话报表的三列恒为 0（见 `ensure_include_usage` 的说明）
        ensure_include_usage(&mut outgoing);
        // 会话解析（同步、零网络）：四档取值链见 `session.rs` 模块头
        let resolved = session::resolve(&outgoing, client_headers);
        logging::verbose(
            "[OpenCode]",
            &format!(
                "session={} request={} source={}",
                resolved.session_id, resolved.request_id, resolved.source
            ),
        );
        let mut headers = vec![
            ("Content-Type".to_string(), "application/json".to_string()),
            ("Accept".to_string(), "*/*".to_string()),
            (
                "Authorization".to_string(),
                format!("Bearer {}", credentials.api_key),
            ),
        ];
        headers.push(endpoints::user_agent_header(client_headers));
        headers.extend(endpoints::opencode_headers(
            &resolved.session_id,
            &resolved.request_id,
        ));
        let url = format!("{}{}", credentials.base_url, endpoints::CHAT_PATH);
        Ok(ChatRequestPlan::chat(url, headers, outgoing))
    }

    /// 上游错误分类（见模块头映射表）
    fn classify_error(&self, status: u16, error_body: &Value) -> UpstreamErrorClass {
        let raw = error_body
            .get("message")
            .or_else(|| error_body.get("msg"))
            .or_else(|| {
                error_body
                    .get("error")
                    .and_then(|error| error.get("message"))
            })
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .unwrap_or("上游错误");
        let message = format!("上游返回 {status}: {raw}");
        let code = error_body.get("code").and_then(Value::as_i64);
        if status == 429 || (status == 503 && raw.contains("Endpoint is unavailable")) {
            return UpstreamErrorClass::QuotaLimited {
                reset_at: None,
                message,
                upstream_code: code,
                status: 429,
            };
        }
        if status == 401 || status == 403 {
            return UpstreamErrorClass::TokenExpired { message };
        }
        let status = if (200..300).contains(&status) { 502 } else { status };
        content_block::classify_or_fatal(status, error_body, message, code)
    }

    /// 取可用 API Key：只读账号记录，**不续期**（没有续期手段）
    fn ensure_access_token<'a>(
        &'a self,
        store: &'a AccountStore,
        account_id: &'a str,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<String, GatewayError>> + Send + 'a>,
    > {
        Box::pin(async move {
            let record = store.opencode_account_record(account_id);
            if !account_id.is_empty() && record.is_none() {
                return Err(GatewayError::with_status(
                    401,
                    format!("OpenCode 账号 {account_id} 不存在（请重新添加）"),
                ));
            }
            let credentials = credentials::from_record(record.as_ref())?;
            Ok(credentials.api_key)
        })
    }

    /// 401 后的强制刷新：本家没有续期手段，如实报错让编排层换账号
    fn refresh_access_token<'a>(
        &'a self,
        _store: &'a AccountStore,
        _account_id: &'a str,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<String, GatewayError>> + Send + 'a>,
    > {
        Box::pin(async move {
            Err(GatewayError::with_status(
                401,
                "OpenCode 的 API Key 无法自动续期，请在「账号」页重新粘贴",
            ))
        })
    }

    /// 没有续期手段（静态 key，上游无 refresh 接口）——维护任务不问本家
    fn supports_refresh(&self) -> bool {
        false
    }

    /// 有远程目录（`GET /zen/go/v1/models`，公开端点）
    fn supports_model_refresh(&self) -> bool {
        true
    }

    /// SSE 帧里的上游模型名要改回客户端请求的名字（与 raccoon / autoclaw
    /// 同一处境：账号可配模型别名 / 映射，上游回的是它收到的名字）
    fn sse_model_rewrite(&self) -> bool {
        true
    }

    /// 刷模型目录：用指定账号（空串 = 库里第一个可用账号）的凭证。
    ///
    /// 「一个账号都没有」不是失败 —— 返回 `unchanged()`（与 Command Code
    /// 同一处置：脚本 / CI 不该看到一个红色失败）。
    fn refresh_models<'a>(
        &'a self,
        store: &'a AccountStore,
        account_id: &'a str,
        force: bool,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ModelRefreshOutcome> + Send + 'a>> {
        Box::pin(async move {
            let record = store.opencode_account_record(account_id);
            let Some(record) = record else {
                if account_id.is_empty() {
                    logging::verbose("[Models]", "OpenCode 模型目录刷新跳过：尚未添加账号");
                    return ModelRefreshOutcome::unchanged();
                }
                return ModelRefreshOutcome::failed("指定的账号不存在或不可用，请重新选择");
            };
            let credentials = match credentials::from_record(Some(&record)) {
                Ok(credentials) => credentials,
                Err(error) => return ModelRefreshOutcome::failed(error.message),
            };
            models::refresh(&credentials.api_key, &credentials.base_url, force).await
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn classify(status: u16, body: Value) -> UpstreamErrorClass {
        OPENCODE_ADAPTER.classify_error(status, &body)
    }

    #[test]
    fn classify_maps_429_and_endpoint_unavailable() {
        assert!(matches!(
            classify(429, json!({"message": "Rate limited"})),
            UpstreamErrorClass::QuotaLimited { status: 429, .. }
        ));
        assert!(matches!(
            classify(503, json!({"message": "Endpoint is unavailable"})),
            UpstreamErrorClass::QuotaLimited { status: 429, .. }
        ));
        // 普通无「Endpoint is unavailable」标记的 503 走 Fatal / 内容拦截判定，
        // 绝不是 QuotaLimited（参考实现实测的错误体判据不能扩大化）
        assert!(!matches!(
            classify(503, json!({"message": "maintenance"})),
            UpstreamErrorClass::QuotaLimited { .. }
        ));
    }

    #[test]
    fn classify_maps_auth_errors() {
        assert!(matches!(
            classify(401, json!({})),
            UpstreamErrorClass::TokenExpired { .. }
        ));
        assert!(matches!(
            classify(403, json!({"message": "forbidden"})),
            UpstreamErrorClass::TokenExpired { .. }
        ));
    }

    #[test]
    fn ensure_include_usage_only_fills_missing() {
        let mut body = json!({"stream": true, "stream_options": {"include_usage": false}});
        ensure_include_usage(&mut body);
        assert_eq!(body["stream_options"]["include_usage"], false);
        let mut body = json!({"stream": true});
        ensure_include_usage(&mut body);
        assert_eq!(body["stream_options"]["include_usage"], true);
        let mut body = json!({"stream": false});
        ensure_include_usage(&mut body);
        assert!(body.get("stream_options").is_none());
    }
}
```

- [ ] **步骤 2：`mod.rs` 挂 adapter + 再导出**

```rust
pub mod adapter;

pub use adapter::{OpenCodeAdapter, OPENCODE_ADAPTER};
```

- [ ] **步骤 3：运行测试**

```bash
cargo test --manifest-path desktop-tauri/src-tauri/server/Cargo.toml opencode
```

---

### 任务 5：身份注册接线（`providers/mod.rs` + `adapter.rs` + `catalog.rs`）

**文件：**
- 修改：`$P/mod.rs`（四处）
- 修改：`$P/adapter.rs`（两处）
- 修改：`$P/catalog.rs`（一处）

- [ ] **步骤 1：`$P/mod.rs` 模块声明**

在 `pub mod monkeycode;`（:138）与 `pub mod officeace;`（:139）之间插入（与现有清单的字母序一致）：

```rust
/// OpenCode（opencode.ai 的 Zen API）。适配实现在 `opencode/`：
/// 无状态 OpenAI 兼容转发 + 粘贴式 API Key + 会话头注入
/// （`x-opencode-session` 等，参考 `LIMTCYT/opencode-go-proxy-for-trae`），
/// `is_stateful` 为 false，见 `opencode/mod.rs` 的模块头。
pub mod opencode;
```

- [ ] **步骤 2：`ProviderKind` 追加变体（末尾）**

在 `OrcaTerm,`（:471）之后、枚举收尾 `}` 之前追加：

```rust
    /// OpenCode（opencode.ai 的 Zen API）。适配实现在 `opencode/`：
    /// 无状态 OpenAI 兼容转发（一次 HTTP 请求 = 一次生成）+ 粘贴式 API Key，
    /// 每请求注入 `x-opencode-session` / `x-opencode-request` /
    /// `x-opencode-client` / `x-opencode-project` 四个会话头（缺第一个上游
    /// 直接 400）。`is_stateful` 为 false，见 `opencode/mod.rs` 的模块头。
    OpenCode,
```

- [ ] **步骤 3：`PROVIDERS` 注册表追加条目（末尾）**

在 `ProviderMeta { id: "orcaterm", label: "OrcaTerm" },`（:567）之后追加：

```rust
    // OpenCode（opencode.ai 的 Zen API）：单一入口（粘贴 API Key），没有地区
    // 伴生。排在末尾（2026-10 接入，后到居后）。它的模型名（glm-5.3 /
    // kimi-k3 / deepseek-v4-pro 等）与 codearts / catpaw / zcode / officeace /
    // orcaterm 大面积同名，同样靠末位让同名模型优先归已有各家 —— 用户要用
    // OpenCode 的同名模型，可停用别家账号或用模型映射绑定（与 OrcaTerm
    // 接入时的既定处置一致）。
    ProviderMeta { id: "opencode", label: "OpenCode" },
```

- [ ] **步骤 4：`kind_from_id` 与 `kind_id` 各加分支**

`kind_from_id`（:623-660）在 `"orcaterm" => Some(ProviderKind::OrcaTerm),`（:651）之后追加：

```rust
        "opencode" => Some(ProviderKind::OpenCode),
```

`kind_id`（:668-695）在 `ProviderKind::OrcaTerm => "orcaterm",`（:693）之后追加（编译器会强制穷举，漏了这里编译不过）：

```rust
        ProviderKind::OpenCode => "opencode",
```

- [ ] **步骤 5：`$P/adapter.rs` 的 `adapter_for` 接真身**

在 `ProviderKind::OrcaTerm => &super::orcaterm::ORCATERM_ADAPTER,`（:1154）之后、match 收尾 `}` 之前追加：

```rust
        // OpenCode（opencode.ai 的 Zen API）：无状态 OpenAI 兼容转发
        // （粘贴 API Key + 会话头注入，见 `opencode/mod.rs` 的模块头）
        ProviderKind::OpenCode => &super::opencode::OPENCODE_ADAPTER,
```

- [ ] **步骤 6：`implemented_kinds()` 追加**

在 `ProviderKind::OrcaTerm,`（:1253）之后追加：

```rust
        // OpenCode：适配器已接线（账号 / 目录 / 转发），且有远程目录
        // （`GET /zen/go/v1/models`，公开端点）—— 必须在列表里，否则目录刷新
        // 循环不会问它。
        ProviderKind::OpenCode,
```

- [ ] **步骤 7：`$P/catalog.rs` 的 `refresh_meta` 加分支**

在 `ProviderKind::OrcaTerm => (false, 0),`（:192）之后追加：

```rust
        // OpenCode 的清单来自 `GET /zen/go/v1/models`（公开端点），拉不到时
        // 回落内置 37 项 —— 「有远程内容」才算远程来源（与 Command Code 同判据）。
        ProviderKind::OpenCode => (
            super::opencode::models::remote_refreshed(),
            super::opencode::models::last_refreshed_at(),
        ),
```

- [ ] **步骤 8：编译验证**

```bash
cargo check --manifest-path desktop-tauri/src-tauri/server/Cargo.toml 2>&1 | tail -20
```
预期：通过（此时账号存储还没接，但身份体系自洽：`kind_from_id` 能返回新 kind、`adapter_for` 有真身）。

---

### 任务 6：账号存储（`opencode_accounts.rs` + 三处接线）

**文件：**
- 创建：`$S/core/account_store/opencode_accounts.rs`
- 修改：`$S/core/account_store/mod.rs`
- 修改：`$S/core/account_store/store_view.rs`
- 修改：`$S/core/account_store/store.rs`（`session_from_record`）

- [ ] **步骤 1：创建 `opencode_accounts.rs`（结构克隆 `commandcode_accounts.rs`）**

```rust
//! OpenCode 账号：粘贴 API Key 落账号 + 公开形态。
//!
//! ── 账号字段（与别家的对照）────────────────────────────────────
//! ```text
//!   本项目记录：{ id, provider:"opencode", name, accessToken(= API Key),
//!                tokenTail, baseUrl?, source, priority, enabled,
//!                addedAt, updatedAt }
//! ```
//! `accessToken` 落盘存的是**API Key 本体**（键名与别家统一：`has_token()` /
//! `access_token()` 一族认这个键）。`baseUrl` 是账号级上游覆盖（自建 / 镜像
//! 网关；可缺省）。**没有** `expiresAt` / `refreshToken`：key 不续期也不过期
//! （`supports_refresh = false`，见 `providers/opencode/credentials.rs`）。
//!
//! ── 账号 id 的派生 ──────────────────────────────────────────
//! 与 Command Code 同一处境：没有可解析的用户标识，id 取
//! `sha256("opencode:account-id:v1\0" + key)` 前 12 个 hex —— 对同一个 key
//! 稳定（再粘一次=更新），不可反推。
//!
//! 本文件全是「读-改-写」存储操作，**没有任何网络请求**（持锁不做网络）。
//! 绝不 unwrap/expect（release 是 panic=abort）。

use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use crate::server::core::providers::opencode::credentials;

use super::priority::next_free_priority;
use super::sql;
use super::state::{mark_name_custom, StoredAccount};
use super::store::{AccountStore, AccountStoreError};
use super::store_util::{token_tail_of, truncate_chars};
use super::OPENCODE_PROVIDER_ID;
use crate::server::logging;

/// 备注名长度上限（与别家同一口径）
const MAX_NAME_LENGTH: usize = 100;

/// 输入长度上限
const MAX_KEY_LENGTH: usize = 8192;

/// 从 payload 里取第一个非空字符串
fn pick(payload: &Value, keys: &[&str]) -> String {
    for key in keys {
        let value = payload
            .get(*key)
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or("");
        if !value.is_empty() {
            return value.to_string();
        }
    }
    String::new()
}

/// key → 稳定的账号 id（见模块头）
fn account_id_for(api_key: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"opencode:account-id:v1\0");
    hasher.update(api_key.as_bytes());
    let digest = format!("{:x}", hasher.finalize());
    format!("opencode-{}", &digest[..12])
}

impl AccountStore {
    /// 取一条 OpenCode 账号记录（原始 JSON）。
    ///
    /// `account_id` 非空 → 按 id 直查；为空 → 启用中账号里优先级最靠前的一条
    /// （与 `commandcode_account_record` 的语义一致）。
    pub fn opencode_account_record(&self, account_id: &str) -> Option<Value> {
        let guard = self.guard();
        if !account_id.is_empty() {
            let record = self.record_by_id(&guard, account_id)?;
            return (record.provider() == OPENCODE_PROVIDER_ID).then(|| record.to_value());
        }
        let mut candidates: Vec<StoredAccount> = self
            .records_for_provider(&guard, OPENCODE_PROVIDER_ID)
            .into_iter()
            .filter(StoredAccount::enabled)
            .collect();
        candidates.sort_by_key(StoredAccount::order_key);
        candidates.into_iter().next().map(|item| item.to_value())
    }

    /// 添加/更新一个 OpenCode 账号（`login::verify_payload` 的返回为 payload）。
    ///
    /// payload：`apiKey`（必填）+ `baseUrl`（可选，已由 verify_payload 校验过
    /// scheme）。同一 key 的记录就地更新（保留优先级与启用状态）；撞到别的
    /// provider 的 id 时报错，绝不覆写。
    pub fn add_opencode_account(
        &self,
        payload: &Value,
        name: Option<&str>,
        source: &str,
    ) -> Result<Value, AccountStoreError> {
        let api_key = pick(payload, &["apiKey"]);
        if api_key.is_empty() {
            return Err(AccountStoreError::new(
                "缺少 API Key（apiKey）",
                400,
            ));
        }
        if api_key.chars().count() > MAX_KEY_LENGTH {
            return Err(AccountStoreError::new("API Key 过长", 400));
        }
        let base_url = pick(payload, &["baseUrl"]);
        let id = account_id_for(&api_key);

        let guard = self.guard();
        let existing = self.record_by_id(&guard, &id);
        if let Some(existing) = existing.as_ref() {
            let existing_provider = existing.provider();
            if existing_provider != OPENCODE_PROVIDER_ID {
                return Err(AccountStoreError::new(
                    format!(
                        "账号 id「{id}」已被{existing_provider}账号占用，无法添加同一个 Key 的\
                         OpenCode 账号（请先处理那个账号）"
                    ),
                    400,
                ));
            }
        }
        let explicit_name = name
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| truncate_chars(value, MAX_NAME_LENGTH));
        let record_name = explicit_name
            .or_else(|| {
                existing
                    .as_ref()
                    .map(StoredAccount::name)
                    .filter(|value| !value.is_empty())
            })
            .unwrap_or_else(|| format!("OpenCode {}", token_tail_of(&api_key)));
        let priority = match existing.as_ref() {
            Some(record) => record.priority(),
            None => {
                let used = self.with_conn(&guard, |conn| sql::priorities_all(conn))?;
                next_free_priority(&used)
            }
        };
        let now = logging::now_ms();
        let mut record = Map::new();
        record.insert("id".to_string(), Value::String(id.clone()));
        record.insert(
            "provider".to_string(),
            Value::String(OPENCODE_PROVIDER_ID.to_string()),
        );
        record.insert("name".to_string(), Value::String(record_name.clone()));
        mark_name_custom(
            &mut record,
            name.is_some_and(|value| !value.trim().is_empty()),
            existing.as_ref(),
        );
        record.insert("accessToken".to_string(), Value::String(api_key.clone()));
        record.insert(
            "tokenTail".to_string(),
            Value::String(token_tail_of(&api_key)),
        );
        // baseUrl：本次给了就写，没给且既有记录有就保留（清空覆盖交给删号重加）
        let effective_base_url = if base_url.is_empty() {
            existing
                .as_ref()
                .and_then(|record| record.fields().get("baseUrl"))
                .and_then(Value::as_str)
                .map(str::trim)
                .unwrap_or("")
                .to_string()
        } else {
            base_url
        };
        if !effective_base_url.is_empty() {
            record.insert("baseUrl".to_string(), Value::String(effective_base_url));
        }
        record.insert("source".to_string(), Value::String(source.to_string()));
        record.insert("priority".to_string(), Value::from(priority));
        record.insert(
            "enabled".to_string(),
            Value::Bool(
                existing
                    .as_ref()
                    .map(StoredAccount::enabled)
                    .unwrap_or(true),
            ),
        );
        record.insert(
            "addedAt".to_string(),
            Value::from(
                existing
                    .as_ref()
                    .map(StoredAccount::added_at)
                    .filter(|value| *value != 0)
                    .unwrap_or(now),
            ),
        );
        record.insert("updatedAt".to_string(), Value::from(now));
        let mut merged = record;
        if let Some(existing) = existing.as_ref() {
            for (key, value) in existing.fields() {
                merged.entry(key.clone()).or_insert_with(|| value.clone());
            }
        }
        let saved = StoredAccount::from_map(merged);
        self.with_conn(&guard, |conn| sql::put(conn, &saved))?;
        logging::log(
            "[Accounts]",
            &format!(
                "✅ OpenCode 账号已保存: {record_name}（优先级 {priority}，key {}）",
                credentials::masked(&api_key)
            ),
        );
        Ok(self.public_account(&saved))
    }

    /// OpenCode 账号的**公开形态**：去掉 key 本体，只留尾几位与「有没有 key」。
    pub fn to_opencode_public_account(&self, record: &StoredAccount) -> Value {
        let mut value = record.to_value();
        if let Some(object) = value.as_object_mut() {
            object.remove("accessToken");
            object.insert("available".to_string(), Value::Bool(record.enabled()));
            object.insert(
                "hasKey".to_string(),
                Value::Bool(
                    record
                        .get("accessToken")
                        .and_then(Value::as_str)
                        .is_some_and(|value| !value.trim().is_empty()),
                ),
            );
        }
        value
    }
}
```

- [ ] **步骤 2：`account_store/mod.rs` 两处**

在 `pub mod commandcode_accounts;`（:108）之后（保持字母序可放 `pub mod monkeycode_accounts;` 前后，实际按文件内既有顺序追加即可）追加：

```rust
pub mod opencode_accounts;
```

在 `COMMANDCODE_PROVIDER_ID` 常量（:214-216）之后追加：

```rust
/// OpenCode provider id（**从注册表推导**，同 [`COMMANDCODE_PROVIDER_ID`] 的口径）。
/// 账号形态见 `opencode_accounts.rs`（单一 provider、没有地区之分）。
pub(crate) const OPENCODE_PROVIDER_ID: &str = crate::server::core::providers::kind_id(
    crate::server::core::providers::ProviderKind::OpenCode,
);
```

- [ ] **步骤 3：`store_view.rs` 公开形态分支**

在 `} else if record.provider() == super::COMMANDCODE_PROVIDER_ID { … self.to_commandcode_public_account(record)`（:344-346）这一分支**之后**插入：

```rust
        } else if record.provider() == super::OPENCODE_PROVIDER_ID {
            // OpenCode：单一入口（粘贴 API Key），公开形态去掉 key 本体、
            // 只留尾号与「有没有 key」（见 `opencode_accounts.rs`）
            self.to_opencode_public_account(record)
```

- [ ] **步骤 4：`store.rs` 的 `session_from_record` 加 OpenCode 附加键**

在 Antigravity 的附加键块（:572-586）之后（OfficeAce 块之前）插入（与 ZCode / Antigravity 同一手法的先例：适配器只能看到会话，provider 专属字段按 id 条件注入）：

```rust
        // ── OpenCode 的附加键（`opencode::adapter` 用）────────────────
        // 记录上可选的 `baseUrl`（自建 / 镜像网关）。适配器只能看到会话，
        // 与 ZCode / Antigravity 同一手法按 provider id 注入 —— 对别家的会话
        // 是逐字空操作（连键都不会多）。
        if record.provider() == super::OPENCODE_PROVIDER_ID {
            if let Some(object) = session.as_object_mut() {
                let base_url = record
                    .fields()
                    .get("baseUrl")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .unwrap_or("");
                if !base_url.is_empty() {
                    object.insert("baseUrl".to_string(), Value::String(base_url.to_string()));
                }
            }
        }
```

- [ ] **步骤 5：编译验证**

```bash
cargo check --manifest-path desktop-tauri/src-tauri/server/Cargo.toml 2>&1 | tail -20
```

---

### 任务 7：`api/accounts.rs` 添加账号分支 + 刷新提示分支

**文件：**
- 修改：`$S/api/accounts.rs`

- [ ] **步骤 1：`add_account` 的 OpenCode 分支**

在 Command Code 分支（:1055-1076）之后插入（先读一遍该分支的上下文与结尾，保证插在正确的 match 臂之间）：

```rust
        // OpenCode（opencode.ai 的 Zen API）：**粘贴 API Key** → 本地归一化
        // （剥引号 / Bearer 前缀）+ baseUrl scheme 校验 → 落账号。key 本身是
        // 上游的账号身份（没有 userId 可解析，账号 id 由 key 摘要派生，见
        // `opencode_accounts.rs` 的模块头）。没有添加探活：本家没有能校验
        // key 的免费只读端点（`/models` 公开、不认 key），真实有效性由上游
        // 生成请求时的 401/403 回答。
        //
        // `importDesktop` 不提供：本家的凭证就是一枚 API Key，没有客户端
        // 「auth.json」那种可读登录态（与 Command Code / Loomy 同一处境）。
        Some(crate::server::core::providers::ProviderKind::OpenCode) => {
            if import_desktop {
                return management_error(
                    400,
                    "OpenCode 不支持导入桌面端登录态，请粘贴 API Key（opencode.ai 控制台获取）添加账号",
                );
            }
            let raw = payload
                .get("apiKey")
                .or_else(|| payload.get("accessToken"))
                .or_else(|| payload.get("token"))
                .or_else(|| payload.get("key"))
                .and_then(Value::as_str)
                .unwrap_or("");
            let credentials = match crate::server::core::providers::opencode::login::verify_payload(
                raw,
                payload.get("baseUrl").and_then(Value::as_str),
            ) {
                Ok(credentials) => credentials,
                Err(error) => return management_error(error.status_code, error.message),
            };
            store.add_opencode_account(&credentials, import_name, "manual")
        }
```

- [ ] **步骤 2：`refresh_account` 的提示分支**

先读 `$S/api/accounts.rs:1520-1540`（Command Code 的「无法续期」提示分支），仿其形状加 OpenCode 臂：

```rust
    if state.store().opencode_account_record(&id).is_some() {
        return management_error(
            400,
            "OpenCode 的 API Key 没有续期接口：key 失效后请在 opencode.ai 重新获取，\
             然后删除本账号重新粘贴",
        );
    }
```

（以 Command Code 分支的真实代码形状为准对齐插入位置——它判定「这个 id 是本家账号」后给出可执行的提示而不是笼统报错；OpenCode 语义完全相同。）

- [ ] **步骤 3：编译 + 全量测试**

```bash
cargo check --manifest-path desktop-tauri/src-tauri/server/Cargo.toml 2>&1 | tail -20
cargo test --manifest-path desktop-tauri/src-tauri/server/Cargo.toml 2>&1 | tail -15
```

---

### 任务 8：前端（表单 + 图标 + i18n）

**文件：**
- 修改：`$UI/ui-islands/src/islands/add-account-configs.ts`
- 修改：`$UI/ui-islands/src/islands/add-provider-pick.tsx`
- 创建：`$UI/ui/assets/providers/opencode.png`
- 修改：`$UI/ui/i18n/{en,ja,ko,pt-BR,zh-Hant}/add-account.js`

- [ ] **步骤 1：`add-account-configs.ts` 加 OPENCODE 配置**

在 `COMMANDCODE` 常量定义之后（`ANTIGRAVITY` 之前）插入（注释风格仿 Command Code 的段落）：

```ts
/**
 * OpenCode（opencode.ai 的 Zen API）：一家一个 provider，没有地区之分。
 *
 * ── 凭证是一枚 API Key，粘贴式 ─────────────────────────────
 * 在 opencode.ai 控制台获取。上游对 key 没有公开的前缀约定（参考实现
 * LIMTCYT/opencode-go-proxy-for-trae 原样透传 Authorization），网关只做
 * 归一化（剥引号 / Bearer 前缀 / 空白），不校验前缀；真实有效性由上游
 * 生成请求时的 401/403 回答。key 不过期、也没有续期接口，失效后重新
 * 获取即可；没有添加探活（/models 公开、不认 key，探不出无效）。
 *
 * ── 自定义 API 地址（可选）─────────────────────────────────
 * 默认上游是 https://opencode.ai；自建 / 镜像网关可填 baseUrl 覆盖
 * （完整 http(s) 地址，不带 /v1 之类的路径后缀）。
 */
const OPENCODE: ProviderConfig = {
  provider: 'opencode',
  label: t('OpenCode'),
  desktop: false,
  manualTitle: t('粘贴 API Key'),
  manualNote: t('粘贴 OpenCode 的 API Key（在 opencode.ai 控制台获取）。key 不过期、也没有续期接口，失效后重新获取即可。默认上游是 https://opencode.ai，自建 / 镜像网关可在下方填「自定义 API 地址」。'),
  fields: [
    { key: 'apiKey', label: 'apiKey', rows: 3, placeholder: t('OpenCode 的 API Key（opencode.ai 控制台获取；也可直接粘贴 Authorization: Bearer … 整行）') },
    { key: 'baseUrl', label: t('自定义 API 地址'), optional: true, placeholder: t('可选，默认 https://opencode.ai；自建 / 镜像网关填完整 http(s) 地址（不带路径后缀）') },
    { key: 'name', label: t('备注名'), optional: true, placeholder: t('可选，留空则用 key 尾号') },
  ],
}
```

并在 `BUILTIN_CONFIGS` 数组的 `COMMANDCODE,` 之后、与后端注册表一致地**排在表尾段**（Command Code / Antigravity / OfficeAce / OrcaTerm 之后）追加：

```ts
  // OpenCode（opencode.ai 的 Zen API）：粘贴 API Key，排在末尾（与后端
  // 注册表 PROVIDERS 的排列一致，2026-10 接入）。
  OPENCODE,
```

- [ ] **步骤 2：`add-provider-pick.tsx` 加图标条目**

在 `PROVIDER_ICONS` 的 `orcaterm: …` 条目之后追加（注释仿 Command Code / Antigravity 的资产来源说明）：

```ts
  // OpenCode：取自官网 `apple-touch-icon.png`（与官网标签页图标同一形象）
  opencode: 'assets/providers/opencode.png',
```

- [ ] **步骤 3：下载图标资产**

```bash
curl -sL --max-time 30 "https://opencode.ai/apple-touch-icon.png" \
  -o "desktop-tauri/ui/assets/providers/opencode.png"
```
验证：`file` 该 PNG（或用 Read 工具看图）非空、是正方形图标；失败则从 opencode.ai 首页 HTML 里找 favicon / og:image 的替代资产 URL 重试（`curl -sL https://opencode.ai | grep -iE "icon|og:image"`）。

- [ ] **步骤 4：五语 i18n 追加**

新字符串只有 4 条（`粘贴 API Key` / `备注名` / `可选，留空则用 key 尾号` 复用 Command Code 的既有条目）。往五个 `add-account.js` 的 `Object.assign(window.wbI18nDict, { … })` 对象里追加条目（键 = 中文原文逐字一致，值 = 译文）：

en：
```js
  "粘贴 OpenCode 的 API Key（在 opencode.ai 控制台获取）。key 不过期、也没有续期接口，失效后重新获取即可。默认上游是 https://opencode.ai，自建 / 镜像网关可在下方填「自定义 API 地址」。": "Paste your OpenCode API key (from the opencode.ai console). The key never expires and has no renewal endpoint; if it stops working, get a new one. The default upstream is https://opencode.ai; fill in a custom API address below for self-hosted / mirror gateways.",
  "OpenCode 的 API Key（opencode.ai 控制台获取；也可直接粘贴 Authorization: Bearer … 整行）": "OpenCode API key (from the opencode.ai console; pasting a whole Authorization: Bearer … line also works)",
  "自定义 API 地址": "Custom API address",
  "可选，默认 https://opencode.ai；自建 / 镜像网关填完整 http(s) 地址（不带路径后缀）": "Optional, default https://opencode.ai; for self-hosted / mirror gateways enter a full http(s) address (no path suffix)",
```

zh-Hant：
```js
  "粘贴 OpenCode 的 API Key（在 opencode.ai 控制台获取）。key 不过期、也没有续期接口，失效后重新获取即可。默认上游是 https://opencode.ai，自建 / 镜像网关可在下方填「自定义 API 地址」。": "貼上 OpenCode 的 API Key（在 opencode.ai 控制台取得）。key 不會過期、也沒有續期介面，失效後重新取得即可。預設上游是 https://opencode.ai，自建 / 鏡像閘道可在下方填「自訂 API 位址」。",
  "OpenCode 的 API Key（opencode.ai 控制台获取；也可直接粘贴 Authorization: Bearer … 整行）": "OpenCode 的 API Key（opencode.ai 控制台取得；也可直接貼上 Authorization: Bearer … 整行）",
  "自定义 API 地址": "自訂 API 位址",
  "可选，默认 https://opencode.ai；自建 / 镜像网关填完整 http(s) 地址（不带路径后缀）": "可選，預設 https://opencode.ai；自建 / 鏡像閘道填完整 http(s) 位址（不帶路徑後綴）",
```

ja：
```js
  "粘贴 OpenCode 的 API Key（在 opencode.ai 控制台获取）。key 不过期、也没有续期接口，失效后重新获取即可。默认上游是 https://opencode.ai，自建 / 镜像网关可在下方填「自定义 API 地址」。": "OpenCode の API キーを貼り付けてください（opencode.ai のコンソールで取得）。キーは無期限で更新 API もありません。失効したら再取得してください。既定の上流は https://opencode.ai で、セルフホスト / ミラーゲートウェイは下の「カスタム API アドレス」に入力します。",
  "OpenCode 的 API Key（opencode.ai 控制台获取；也可直接粘贴 Authorization: Bearer … 整行）": "OpenCode の API キー（opencode.ai コンソールで取得。Authorization: Bearer … の行全体を貼り付けても OK）",
  "自定义 API 地址": "カスタム API アドレス",
  "可选，默认 https://opencode.ai；自建 / 镜像网关填完整 http(s) 地址（不带路径后缀）": "任意。既定は https://opencode.ai。セルフホスト / ミラーゲートウェイは完全な http(s) アドレスを入力（パス接尾辞なし）",
```

ko：
```js
  "粘贴 OpenCode 的 API Key（在 opencode.ai 控制台获取）。key 不过期、也没有续期接口，失效后重新获取即可。默认上游是 https://opencode.ai，自建 / 镜像网关可在下方填「自定义 API 地址」。": "OpenCode API 키를 붙여넣으세요(opencode.ai 콘솔에서 발급). 키는 만료되지 않고 갱신 API도 없으며, 만료 시 새로 발급하면 됩니다. 기본 업스트림은 https://opencode.ai 이며, 자체 호스팅 / 미러 게이트웨이는 아래 '사용자 지정 API 주소'에 입력하세요.",
  "OpenCode 的 API Key（opencode.ai 控制台获取；也可直接粘贴 Authorization: Bearer … 整行）": "OpenCode API 키(opencode.ai 콘솔 발급. Authorization: Bearer … 전체 행을 붙여넣어도 됨)",
  "自定义 API 地址": "사용자 지정 API 주소",
  "可选，默认 https://opencode.ai；自建 / 镜像网关填完整 http(s) 地址（不带路径后缀）": "선택 사항, 기본값 https://opencode.ai. 자체 호스팅 / 미러 게이트웨이는 전체 http(s) 주소 입력(경로 접미사 없음)",
```

pt-BR：
```js
  "粘贴 OpenCode 的 API Key（在 opencode.ai 控制台获取）。key 不过期、也没有续期接口，失效后重新获取即可。默认上游是 https://opencode.ai，自建 / 镜像网关可在下方填「自定义 API 地址」。": "Cole a chave de API do OpenCode (obtida no console opencode.ai). A chave não expira e não há endpoint de renovação; se parar de funcionar, obtenha outra. O upstream padrão é https://opencode.ai; para gateways próprios / espelhados, preencha o endereço de API personalizado abaixo.",
  "OpenCode 的 API Key（opencode.ai 控制台获取；也可直接粘贴 Authorization: Bearer … 整行）": "Chave de API do OpenCode (console opencode.ai; também pode colar a linha inteira Authorization: Bearer …)",
  "自定义 API 地址": "Endereço de API personalizado",
  "可选，默认 https://opencode.ai；自建 / 镜像网关填完整 http(s) 地址（不带路径后缀）": "Opcional, padrão https://opencode.ai; para gateways próprios / espelhados informe um endereço http(s) completo (sem sufixo de caminho)",
```

（各文件是单行/紧凑 JSON 形态：在对象字面量的最后一个键值对后补逗号再插入。改完抽查一条：改 en 文件后确认该键在 zh-Hant/ja/ko/pt-BR 中都已存在。）

---

### 任务 9：文档（README × 6）

**文件：**
- 修改：`README.md`、`docs/README.{en,zh-Hant,ja,ko,pt-BR}.md`

- [ ] **步骤 1：`README.md` 三处**

1. 「✨ 核心特性」的 `🏪 **内置 17 家提供商**`（:26）→ `🏪 **内置 18 家提供商**`，并把括号里的列举追加 `OpenCode`；
2. 「🤝 支持的提供商」表格（:59-78）在 OfficeAce 行之后、自定义提供商行之前插入：

```markdown
| OpenCode（opencode.ai Zen） | ✓ | — | ✓ | — | — | — |
```

3. 表格下的注（:80）如有「模型列表的 ✓ 表示支持在线自动拉取」相关措辞，核对无需改动（OpenCode 是公开端点远端拉取 + 内置兜底，符合现有口径）；另在注的末尾追加一句：

```markdown
OpenCode 的同名模型（glm-5.3 / kimi-k3 等）按注册表顺序让位给先接入的各家——要指定用 OpenCode 的同名模型，可停用其它家账号或用模型映射绑定。
```

- [ ] **步骤 2：五个语言副本同步**

对 `docs/README.en.md` / `README.zh-Hant.md` / `README.ja.md` / `README.ko.md` / `README.pt-BR.md` 各自：grep 找到提供商表格与「17」计数，做与步骤 1 等义的本地化修改（表格行译名：en `| OpenCode (opencode.ai Zen) | ✓ | — | ✓ | — | — | — |`；其余语言按各自文档现有行的措辞风格翻译）。

---

### 任务 10：终验（工具链可用时）

- [ ] **步骤 1：全量编译 + 测试**

```bash
cargo check --manifest-path desktop-tauri/src-tauri/server/Cargo.toml 2>&1 | tail -5
cargo test --manifest-path desktop-tauri/src-tauri/server/Cargo.toml 2>&1 | tail -15
```
预期：0 error；新增测试全 PASS；存量测试无回归。

- [ ] **步骤 2：手动冒烟（需要一把真实 OpenCode API Key，用户提供）**

```bash
# 1) 起服务后添加账号（或面板操作）
curl -X POST http://127.0.0.1:3065/api/accounts -H 'Content-Type: application/json' \
  -d '{"provider":"opencode","apiKey":"<真实key>","name":"冒烟"}'
# 2) 模型清单里能看到 opencode 的模型
curl http://127.0.0.1:3065/v1/models
# 3) 流式与非流式各一次，检查日志 [OpenCode] session=… request=msg_1 → msg_2 递增
curl http://127.0.0.1:3065/v1/chat/completions -H 'Content-Type: application/json' \
  -d '{"model":"glm-5.3-flash","messages":[{"role":"user","content":"你好"}],"stream":true}'
```
预期：对话成功；同一会话续问一次时日志 `request=msg_2`（会话键 = 内容哈希档）；报表页出现该账号的用量。

- [ ] **步骤 3：无工具链时的替代终验**

若 cargo 不可用：逐文件对照本计划自查（签名、模块路径、match 穷举），并明确告知用户「代码未经本地编译，构建请跑 `npm run tauri:build` 或 `docker compose up -d --build`，编译错反馈回来我再修」。

---

## 自检记录（写计划后已执行）

1. **规格覆盖度**：规格 §3（端点/头/会话/错误）→ 任务 1-4；§4 注册表 → 任务 5；账号存储 → 任务 6；API 分支 → 任务 7；前端/文档 → 任务 8-9；§5 边界（缺 key 401、baseUrl scheme 校验、毒锁处理、400 透传）散布在对应任务代码中；§6 测试 → 各任务单测 + 任务 10。
2. **占位符扫描**：任务 7 步骤 2 要求「先读 :1520-1540 再仿写」——这是刻意留的读点（该区域未在计划期展开），已给出完整目标代码，不属于占位符；无「待定/TODO」。
3. **类型一致性**：`OpenCodeCredentials`（credentials.rs）在 adapter 与 models 的用法一致；`ResolvedSession` 字段（session_id/request_id/source）在 adapter 的解构一致；`opencode_account_record` / `add_opencode_account` / `to_opencode_public_account` 在任务 6 定义、任务 7 引用，名称一致；`SCOPE_OPENCODE` 任务 3 定义、models.rs 引用一致。
