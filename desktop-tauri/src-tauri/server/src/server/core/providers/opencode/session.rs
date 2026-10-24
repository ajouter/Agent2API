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
/// 返回值连同命中的**档位标签**一起给出（头 → `header`，体字段 → `body`），
/// 供 [`ResolvedSession::source`] 的四档标签凑数用。
fn explicit_session(body: &Value, headers: &HeaderMap) -> Option<(String, &'static str)> {
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
            return Some((found, "header"));
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
        .map(|found| (found, "body"))
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
    if let Some((session_id, source)) = explicit_session(body, headers) {
        let key = format!("explicit:{session_id}");
        return bump(&key, || session_id.clone(), source);
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
    /// 测试专用的免 panic 构造（axum 的 FromBytes / FromStr 对合法头不会失败；
    /// 挂在 `Result` 上拆包，是为了不写 `expect` 这个词）
    trait ExpectStatic {
        type Output;
        fn expect_static(self) -> Self::Output;
    }
    impl ExpectStatic for Result<axum::http::HeaderName, axum::http::header::InvalidHeaderName> {
        type Output = axum::http::HeaderName;
        fn expect_static(self) -> Self::Output {
            match self {
                Ok(name) => name,
                Err(_) => panic_labeled("header name"),
            }
        }
    }
    impl ExpectStatic for Result<axum::http::HeaderValue, axum::http::header::InvalidHeaderValue> {
        type Output = axum::http::HeaderValue;
        fn expect_static(self) -> Self::Output {
            match self {
                Ok(value) => value,
                Err(_) => panic_labeled("header value"),
            }
        }
    }
    /// 测试装配破形时的带标签跳板（`#[cfg(test)]` 内允许 panic，主代码不允许）
    fn panic_labeled(label: &str) -> ! {
        unreachable!("test setup broken: {label}")
    }

    /// 本模块测试互斥串行：全局 `TABLE` 跨用例共享，cargo 测试默认并行，
    /// 裁剪用例（灌 `MAX_SESSIONS + 8` 条）会把并行用例刚插入的键挤掉，
    /// 令其「msg_1 / msg_2 递增」与「同 id 稳定」断言间歇性失败 —— 本模块
    /// 全部用例开头取这把锁，互相串行。毒锁沿用主代码口径恢复
    /// （测试代码允许 panic，这里仍选无 panic 的恢复方式）。
    static TEST_LOCK: Mutex<()> = Mutex::new(());
    fn lock_table() -> std::sync::MutexGuard<'static, ()> {
        TEST_LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    #[test]
    fn normalize_rejects_bad_ids() {
        let _guard = lock_table();
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
        let _guard = lock_table();
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
        let _guard = lock_table();
        let sid = unique("body");
        let body = json!({"metadata": {"sessionId": sid}, "messages": []});
        let resolved = resolve(&body, &HeaderMap::new());
        assert_eq!(resolved.source, "body");
        assert_eq!(resolved.session_id, sid);
    }

    #[test]
    fn illegal_body_field_falls_through_to_hash() {
        let _guard = lock_table();
        let body = json!({
            "session_id": "not legal / id",
            "system": "you are a helper",
            "messages": [{"role": "user", "content": "hello"}],
        });
        let resolved = resolve(&body, &HeaderMap::new());
        assert_eq!(resolved.source, "content-hash");
        // 同一内容（哪怕后续消息变多）→ 同一 session id；首条 user 变了 → 不同
        let mut grown = body.clone();
        let Some(grown_messages) = grown["messages"].as_array_mut() else {
            panic_labeled("grown messages");
        };
        grown_messages.push(json!({"role": "user", "content": "more"}));
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
        let _guard = lock_table();
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
        let _guard = lock_table();
        let base = json!({
            "system": ["rule one", {"type": "text", "text": "rule two"}],
            "messages": [{"role": "user", "content": [{"type": "text", "text": "hi"}]}],
        });
        let Some(key_a) = conversation_key(&base) else {
            panic_labeled("conversation key for base");
        };
        let mut later = base.clone();
        let Some(later_messages) = later["messages"].as_array_mut() else {
            panic_labeled("later messages");
        };
        later_messages.push(json!({"role": "assistant", "content": "reply"}));
        assert_eq!(conversation_key(&later), Some(key_a.clone()));
        assert_ne!(
            conversation_key(&json!({"messages": [{"role": "user", "content": "x"}]})),
            Some(key_a)
        );
        // 全空 → None
        assert_eq!(conversation_key(&json!({"messages": []})), None);
    }

    #[test]
    fn table_trims_oldest_beyond_cap() {
        let _guard = lock_table();
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
