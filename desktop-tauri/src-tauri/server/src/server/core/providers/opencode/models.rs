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
//! ── 广告名前缀（`opencode-`，与 Trae 的 `-solo` 同一机制）──────
//! OpenCode 的模型名（glm-5.3 / kimi-k3 / deepseek-v4-pro 等）与 codearts /
//! catpaw / zcode / officeace / orcaterm **大面积同名**，而模型路由按名字全局
//! 匹配 —— 裸名发布的话这些模型永远轮不到本家（注册表顺序让位）。因此本家
//! 在清单里统一以 `opencode-{上游名}` 广告：与其他家互不冲突、用户一眼看出
//! 归属（Cline 的 `cline-free/` 前缀、Trae 的 `-solo` 后缀是同一先例）。
//! 转发侧由 `adapter::build_chat_request` 调 [`upstream_name`] 剥掉前缀再发
//! 上游；**裸名也认**（用户照上游文档写名字、经模型映射指到本家的，原样
//! 放行）—— 与 Trae 的语义逐字一致。
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

/// 广告名前缀：清单里所有 id 都以它开头（见模块头「广告名前缀」）。
pub const ADVERTISE_PREFIX: &str = "opencode-";

/// 广告名 → 上游真实名：剥**一层** `opencode-` 前缀；裸名原样放行。
///
/// 与 Trae 的 `upstream_name` 同一语义（连「只剥一层」的边界用例也一致）：
/// 上游哪天真出一个自带 `opencode-` 开头的模型名，它的广告形态是
/// `opencode-opencode-…`，剥一层正好还原。带斜杠的名字不受影响。
pub fn upstream_name(model: &str) -> String {
    model.strip_prefix(ADVERTISE_PREFIX).unwrap_or(model).to_string()
}

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

/// 组装一条**广告形态**的清单项（远程目录与内置兜底共用，见模块头前缀说明）。
fn advertised(id: &str, name: &str) -> Value {
    let mut entry = json!({
        "id": id,
        "name": name,
        "supportsToolCall": true,
        "supportsReasoning": true,
        "supportsImages": false,
    });
    // models.dev 的元数据（上下文长度等）按**真实上游 id** 索引：先补元数据、
    // 再改写为带前缀的广告名 —— 顺序反了就查不到，那些列会显示成「没数据」。
    // manifest_for 里那次 apply 是按广告名查的（必然未命中），与本处不冲突。
    crate::server::core::model_meta::apply(&mut entry);
    entry["id"] = Value::String(format!("{ADVERTISE_PREFIX}{id}"));
    entry
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
    Some(advertised(id, name))
}

fn fallback_models() -> Vec<Value> {
    FALLBACK_MODELS
        .iter()
        .map(|(id, name)| advertised(id, name))
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
        assert_eq!(parsed[0]["id"], "opencode-glm-5.3", "广告名一律带前缀");
        assert_eq!(parsed[1]["name"], "Kimi K3");
        let bare = json!([{"id": "hy3"}]);
        assert_eq!(parse_models(&bare).len(), 1);
        assert!(parse_models(&json!({})).is_empty());
        assert!(parse_models(&json!({"data": [{"name": "no-id"}]})).is_empty());
    }

    #[test]
    fn advertised_names_round_trip_to_upstream() {
        assert_eq!(upstream_name("opencode-glm-5.3"), "glm-5.3");
        assert_eq!(upstream_name("glm-5.3"), "glm-5.3", "裸名也认（用户照上游文档写名字）");
        assert_eq!(upstream_name("opencode-opencode-x"), "opencode-x", "只剥一层：真以 opencode- 开头的上游名还能回来");
        assert_eq!(upstream_name("opencode-deepseek/deepseek-v4-pro"), "deepseek/deepseek-v4-pro", "带斜杠的名字不能被切坏");
        // 远程解析与内置兜底的条目一律带前缀
        let parsed = parse_models(&json!({"data": [{"id": "kimi-k3"}]}));
        assert_eq!(parsed[0]["id"], "opencode-kimi-k3");
        assert!(fallback_models().iter().all(|entry| entry["id"]
            .as_str()
            .is_some_and(|id| id.starts_with(ADVERTISE_PREFIX))));
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
