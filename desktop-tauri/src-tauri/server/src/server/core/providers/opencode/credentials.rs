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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// 测试装配破形时的带标签跳板（`#[cfg(test)]` 内允许 panic，主代码不允许）
    fn unpack<T>(result: Result<T, GatewayError>, label: &str) -> T {
        match result {
            Ok(value) => value,
            Err(_) => unreachable!("test setup broken: {label}"),
        }
    }

    #[test]
    fn from_record_reads_both_record_shapes() {
        // 原始记录形态：key 在顶层 `accessToken`，baseUrl 同在顶层
        let raw = json!({
            "id": "acc-1",
            "name": "主号",
            "accessToken": "oc_raw_key",
            "baseUrl": "https://mirror.example.com/",
        });
        let creds = unpack(from_record(Some(&raw)), "raw record");
        assert_eq!(creds.api_key, "oc_raw_key");
        assert_eq!(creds.base_url, "https://mirror.example.com");
        assert_eq!(creds.id, "acc-1");
        assert_eq!(creds.name, "主号");

        // 会话形态：key 在 `auth.accessToken`（baseUrl 由 store 注入顶层）
        let session = json!({
            "id": "acc-1",
            "auth": {"accessToken": "oc_ses_key"},
            "baseUrl": "https://mirror.example.com",
        });
        let creds = unpack(from_record(Some(&session)), "session record");
        assert_eq!(creds.api_key, "oc_ses_key");
        assert_eq!(creds.base_url, "https://mirror.example.com");

        // 无覆盖 → 默认基址
        let plain = unpack(from_record(Some(&json!({"accessToken": "oc_key"}))), "plain record");
        assert_eq!(plain.base_url, endpoints::DEFAULT_BASE_URL);

        // 缺 key / 无记录 → 401
        assert!(from_record(Some(&json!({"id": "x"}))).is_err());
        assert!(from_record(None).is_err());
    }

    #[test]
    fn masked_shows_only_tail() {
        assert_eq!(masked("short"), "…");
        assert_eq!(masked("sk-abcdef1234"), "…1234");
    }
}
