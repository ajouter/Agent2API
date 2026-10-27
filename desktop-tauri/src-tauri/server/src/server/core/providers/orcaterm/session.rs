//! OrcaTerm 会话粘性：客户端会话键 → 上游 conversationId 的注册表。
//!
//! ── 为什么需要它（2026-10-24，见 specs/2026-10-24-orcaterm-agent-session.md）──
//! 改造前每个请求都新建上游会话、历史拍平进输入 —— 上游 agent 的「已批准」
//! 状态存在服务端会话里，跨轮无法存活，agent 客户端每确认一次就被再问一次。
//! 粘性注册表让同一客户端会话续写同一个上游 conversationId：确认落进它刚
//! 问过的会话，多步任务才推得动。
//!
//! ── 会话键的取值链（优先级从高到低，移植 opencode/session.rs）────
//!   1. 入站头：x-session-id / x-conversation-id / x-thread-id
//!   2. 请求体字段：session_id / sessionId / conversation_id / conversationId /
//!      thread_id / threadId（含 metadata.* 同名键）
//!   3. 内容哈希：messages 里第一条 system + 第一条 user 消息 —— 同一会话
//!      这两段相对稳定，因此同一会话命中同一个键
//!   4. 以上皆无 → `None`：**不入表**，每个请求各自新建会话（与改造前的
//!      「请求内会话」行为一致）
//!
//! ── 与 opencode 那份的两处差别（别当漏抄）────────────────────
//!   - 表里存的是**上游 conversationId**（不是网关自造的 id）：上游会话
//!     才是这里要粘住的东西；键本身不外发。
//!   - 记录绑定**账号 id**：conversation 是账号作用域的，换账号续写必然
//!     失配 —— 取用时不一致一律视为未命中（调用方会新建并覆盖写）。
//!
//! ── 内容哈希的碰撞面 ────────────────────────────────────────
//! 两个不同客户端会话若 system 与首条 user 完全一致，会共享同一个上游
//! 会话（互相看得见对方历史）。agent 客户端的 system 都带会话级上下文
//! （任务、工作区、工具集），实际撞不上；与 opencode 适配器的既定取舍一致。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：锁内只做查/插/裁（不跨 await），毒锁按主代码
//! 口径恢复（`into_inner`），零 unwrap/expect/panic。

use std::collections::{HashMap, VecDeque};
use std::sync::{Mutex, OnceLock};

use axum::http::HeaderMap;
use serde_json::Value;
use sha2::{Digest, Sha256};

/// 注册表上限（与 opencode 会话表同一口径，防泄漏）
const MAX_CONVERSATIONS: usize = 256;
/// 显式会话 id 的合法长度上限（按字符数，与 opencode 一致）
const MAX_ID_LENGTH: usize = 128;

/// 一条粘性记录：上游会话 + 建它的账号。
struct ConversationRecord {
    conversation_id: String,
    account_id: String,
}

/// 全局注册表：`map` 存记录、`order` 记插入序（裁最旧用）。
struct ConversationTable {
    map: HashMap<String, ConversationRecord>,
    order: VecDeque<String>,
}

static TABLE: OnceLock<Mutex<ConversationTable>> = OnceLock::new();

fn table() -> &'static Mutex<ConversationTable> {
    TABLE.get_or_init(|| {
        Mutex::new(ConversationTable {
            map: HashMap::new(),
            order: VecDeque::new(),
        })
    })
}

/// 显式会话 id 的合法性（与 opencode `normalize_session_id` 同口径）：
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

/// 非字符串值的锚定文本（哈希键用）
fn anchor_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => serde_json::to_string(other).unwrap_or_default(),
    }
}

/// 消息数组里某一角色的第一条 content（无则 None）。
fn first_content_of<'a>(messages: &'a [Value], role: &str) -> Option<&'a Value> {
    messages
        .iter()
        .find(|message| message.get("role").and_then(Value::as_str) == Some(role))
        .and_then(|message| message.get("content"))
}

/// 内容哈希会话键（档 3）：messages 里第一条 system + 第一条 user 的
/// content，SHA-256。同一会话这两段稳定，续轮命中同一键；两段皆空 → None。
fn conversation_key(body: &Value) -> Option<String> {
    let mut anchor = String::new();
    if let Some(messages) = body.get("messages").and_then(Value::as_array) {
        if let Some(content) = first_content_of(messages, "system") {
            anchor.push_str(&anchor_text(content));
        }
        if let Some(content) = first_content_of(messages, "user") {
            anchor.push('\n');
            anchor.push_str(&anchor_text(content));
        }
    }
    if anchor.trim().is_empty() {
        return None;
    }
    let mut hasher = Sha256::new();
    hasher.update(anchor.as_bytes());
    Some(format!("k_{:x}", hasher.finalize()))
}

/// 显式会话探测（档 1 头 + 档 2 体字段）。
fn explicit_session(body: &Value, headers: &HeaderMap) -> Option<String> {
    for name in ["x-session-id", "x-conversation-id", "x-thread-id"] {
        let value = headers.get(name).and_then(|header| header.to_str().ok());
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

/// 解析本次请求的会话键（见模块头取值链）。
///
/// `None` = 随机档（无法稳定锚定会话），调用方不查表、不登记 —— 每个请求
/// 各自新建上游会话，与改造前的行为一致。
pub fn resolve_session_key(headers: &HeaderMap, body: &Value) -> Option<String> {
    if let Some(explicit) = explicit_session(body, headers) {
        return Some(format!("explicit:{explicit}"));
    }
    conversation_key(body)
}

/// 取该会话键当前粘住的上游会话。**账号必须一致**：conversation 是账号
/// 作用域的，换账号续写必然失配（视为未命中，由调用方新建并覆盖写）。
pub fn conversation_for(key: &str, account_id: &str) -> Option<String> {
    let guard = table()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let record = guard.map.get(key)?;
    (record.account_id == account_id)
        .then(|| record.conversation_id.clone())
}

/// 登记一个会话键 → 上游会话（重复登记覆盖旧值；超限按插入序裁最旧）。
pub fn remember_conversation(key: &str, account_id: &str, conversation_id: &str) {
    let mut guard = table()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if !guard.map.contains_key(key) {
        guard.order.push_back(key.to_string());
    }
    guard.map.insert(
        key.to_string(),
        ConversationRecord {
            conversation_id: conversation_id.to_string(),
            account_id: account_id.to_string(),
        },
    );
    while guard.map.len() > MAX_CONVERSATIONS {
        let Some(oldest) = guard.order.pop_front() else {
            break;
        };
        guard.map.remove(&oldest);
    }
}

/// 丢弃一个会话键的记录（续写失败的自愈：下次重新建会话）。
pub fn drop_conversation(key: &str) {
    let mut guard = table()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if guard.map.remove(key).is_some() {
        guard.order.retain(|stored| stored != key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// 每个用例独立的键（全局表跨用例共享，键不能撞）
    static CASE_SEQ: AtomicU64 = AtomicU64::new(1);
    fn unique(text: &str) -> String {
        format!("{text}-{}", CASE_SEQ.fetch_add(1, Ordering::Relaxed))
    }

    fn headers_with(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut headers = HeaderMap::new();
        for (name, value) in pairs {
            let name = axum::http::HeaderName::from_bytes(name.as_bytes())
                .unwrap_or_else(|_| unreachable!("test header name"));
            let value = axum::http::HeaderValue::from_str(value)
                .unwrap_or_else(|_| unreachable!("test header value"));
            headers.insert(name, value);
        }
        headers
    }

    fn body_with_first_user(content: &str) -> Value {
        json!({"messages": [{"role": "user", "content": content}]})
    }

    #[test]
    fn explicit_header_wins() {
        let sid = unique("hdr");
        let headers = headers_with(&[("x-session-id", &sid)]);
        let key = resolve_session_key(&headers, &json!({}));
        assert_eq!(key, Some(format!("explicit:{sid}")));
    }

    #[test]
    fn body_field_is_second_priority() {
        let sid = unique("body");
        let body = json!({"metadata": {"sessionId": sid}, "messages": []});
        assert_eq!(
            resolve_session_key(&HeaderMap::new(), &body),
            Some(format!("explicit:{sid}"))
        );
    }

    #[test]
    fn illegal_id_falls_through_to_hash() {
        let body = json!({
            "session_id": "not legal / id",
            "messages": [
                {"role": "system", "content": "be helpful"},
                {"role": "user", "content": "hello"},
            ],
        });
        let key = resolve_session_key(&HeaderMap::new(), &body);
        let Some(hash_key) = &key else {
            unreachable!("hash key expected");
        };
        assert!(hash_key.starts_with("k_"));
        // 同一 system + 首条 user（后续消息变多）→ 同一键
        let mut grown = body.clone();
        grown["messages"].as_array_mut().map(|items| {
            items.push(json!({"role": "assistant", "content": "reply"}));
            items.push(json!({"role": "user", "content": "更多"}));
        });
        assert_eq!(resolve_session_key(&HeaderMap::new(), &grown), key);
        // 首条 user 变了 → 不同键
        let changed = json!({
            "messages": [
                {"role": "system", "content": "be helpful"},
                {"role": "user", "content": "different"},
            ],
        });
        assert_ne!(resolve_session_key(&HeaderMap::new(), &changed), key);
    }

    #[test]
    fn empty_body_yields_none() {
        assert_eq!(resolve_session_key(&HeaderMap::new(), &json!({})), None);
        assert_eq!(
            resolve_session_key(&HeaderMap::new(), &json!({"messages": []})),
            None
        );
    }

    #[test]
    fn remember_and_reuse_is_account_scoped() {
        let key = unique("sticky");
        remember_conversation(&key, "acc-1", "cid-a");
        assert_eq!(conversation_for(&key, "acc-1"), Some("cid-a".to_string()));
        // 换账号：视为未命中
        assert_eq!(conversation_for(&key, "acc-2"), None);
        // 覆盖写后换回来也不行（记录已被 acc-2 的会话覆盖）
        remember_conversation(&key, "acc-2", "cid-b");
        assert_eq!(conversation_for(&key, "acc-2"), Some("cid-b".to_string()));
        assert_eq!(conversation_for(&key, "acc-1"), None);
    }

    #[test]
    fn drop_removes_record() {
        let key = unique("drop");
        remember_conversation(&key, "acc-1", "cid-a");
        drop_conversation(&key);
        assert_eq!(conversation_for(&key, "acc-1"), None);
        // 重复 drop 无害
        drop_conversation(&key);
    }

    #[test]
    fn table_trims_oldest_beyond_cap() {
        let first_key = unique("trim");
        remember_conversation(&first_key, "acc-1", "cid-first");
        for index in 0..(MAX_CONVERSATIONS + 8) {
            remember_conversation(&unique("fill"), "acc-1", &format!("cid-{index}"));
        }
        // 最早的被裁掉：即使账号一致也取不到
        assert_eq!(conversation_for(&first_key, "acc-1"), None);
    }
}
