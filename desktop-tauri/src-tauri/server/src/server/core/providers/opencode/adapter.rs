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
        // 广告名剥前缀：清单里广告的是 `opencode-{上游名}`（与其他渠道的同名
        // 模型互不冲突，机制见 `models.rs` 模块头），上游只认真实名。编排层
        // `send_body` 已把 body.model 改写成清单 id（即广告名），这里剥回真实名
        // 再发；裸名（经模型映射指到本家的）`upstream_name` 原样放行 —— 与
        // Trae 的 `upstream_name` 同一语义。
        let wire_model = outgoing
            .get("model")
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or("")
            .to_string();
        if !wire_model.is_empty() {
            outgoing["model"] = Value::String(models::upstream_name(&wire_model));
        }
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

    /// 广告名剥前缀的转发语义（`models::upstream_name` 的端到端走查）
    #[test]
    fn build_strips_advertise_prefix_from_model() {
        let account = json!({"auth": {"accessToken": "sk-test"}});
        let headers = HeaderMap::new();
        let build = |body: &Value| match OPENCODE_ADAPTER.build_chat_request(&account, body, &headers) {
            Ok(plan) => plan,
            Err(error) => unreachable!("计划构建失败: {}", error.message),
        };
        let prefixed = json!({
            "model": "opencode-glm-5.3",
            "messages": [{"role": "user", "content": "hi"}],
        });
        assert_eq!(build(&prefixed).body["model"], "glm-5.3");
        let bare = json!({
            "model": "glm-5.3",
            "messages": [{"role": "user", "content": "hi"}],
        });
        assert_eq!(build(&bare).body["model"], "glm-5.3", "裸名原样放行");
        let slashed = json!({
            "model": "opencode-deepseek/deepseek-v4-pro",
            "messages": [{"role": "user", "content": "hi"}],
        });
        assert_eq!(build(&slashed).body["model"], "deepseek/deepseek-v4-pro");
    }
}
