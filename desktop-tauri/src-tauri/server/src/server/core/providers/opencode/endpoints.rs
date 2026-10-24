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
    use axum::http::{HeaderMap, HeaderValue};

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
        headers.insert("user-agent", HeaderValue::from_static("my-client/1.0"));
        assert_eq!(user_agent_header(&headers).1, CLIENT_USER_AGENT);
        let mut headers = HeaderMap::new();
        headers.insert("user-agent", HeaderValue::from_static("OpenCode/1.19 cli"));
        assert_eq!(user_agent_header(&headers).1, "OpenCode/1.19 cli");
        let empty = HeaderMap::new();
        assert_eq!(user_agent_header(&empty).1, CLIENT_USER_AGENT);
    }
}
