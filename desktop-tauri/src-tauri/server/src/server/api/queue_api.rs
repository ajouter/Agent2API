//! GET/PUT /api/queue —— 排队等待设置（次数与单次时长）。
//!
//! ── 这条设置管什么 ──────────────────────────────────────────
//! 有些上游在模型繁忙时**不报错、只排队**：回一句「暂不可服务，建议 N 秒后
//! 再来」（Qoder 免费模型用的就是这套，业务码 10605）。参考实现的做法是按上游
//! 建议时长等一会儿再发同一请求，等几次还排不上才把排队态报给客户端。
//!
//!   - `queueMaxWaits`：最多等几次（0 = 不等，排队即返回 503）；
//!   - `queueWaitSeconds`：单次等待秒数（0 = 跟随上游建议，上游没给建议时
//!     适配器退到 15 秒）。
//!
//! 两项存在配置库（键名契约见 `config.rs` 的 `KEY_QUEUE_*`），由设置页
//! 「通用 → 排队等待」经 HTTP 桥读写 —— 与 /api/timeouts 同一模式：
//! 独立端点、允许部分字段、返回生效后的全量值、无副作用（不重启进程）。
//! 保存后对下一个请求立即生效（走排队制的适配器每次遇到排队都读内存快照）。
//!
//! ── 与「等待响应超时」的关系（写进设置页提示文案的同一条）───────
//! 排队等待发生在首字节之前，吃的是「等待响应超时」那份预算：两项设置一起
//! 决定「一次请求最多卡多久」。适配器不会为了排队突破等待响应超时。

use axum::body::Bytes;
use axum::extract::State;
use axum::response::Response;
use serde_json::{json, Value};

use crate::server::config::{
    self, QueuePatch, QueueSettings, KEY_QUEUE_MAX_WAITS, KEY_QUEUE_WAIT_SECONDS,
    QUEUE_MAX_MAX_WAITS, QUEUE_MAX_WAIT_SECONDS, QUEUE_MIN_MAX_WAITS, QUEUE_MIN_WAIT_SECONDS,
};
use crate::server::errors;
use crate::server::http::{ok_json, parse_body};
use crate::server::logging;
use crate::server::ServerState;

/// GET /api/queue
pub async fn get_queue(State(_state): State<ServerState>) -> Response {
    ok_json(queue_json(config::queue_settings()))
}

/// PUT /api/queue —— body `{queueMaxWaits?, queueWaitSeconds?}`
///
/// 允许部分字段（未出现的项保持原值，null 同义）。校验通过后：写配置库
/// → 返回**生效后**的值（前端直接用响应刷新界面，不必再 GET 一次）。
pub async fn put_queue(State(_state): State<ServerState>, body: Bytes) -> Response {
    let payload = match parse_body(&body) {
        Ok(value) => value,
        Err(error) => return errors::management_error(400, error.message),
    };
    let Some(object) = payload.as_object() else {
        return errors::management_error(400, "请求体必须是 JSON 对象");
    };

    // 校验与写盘分两步（与 put_timeouts 同一顺序）：两项里有一项非法时整体
    // 不落盘，避免出现「次数改了、时长没改」的半套设置
    let mut patch = QueuePatch::default();
    let mut updated = false;
    let targets = [
        (
            KEY_QUEUE_MAX_WAITS,
            object.get(KEY_QUEUE_MAX_WAITS),
            QUEUE_MIN_MAX_WAITS,
            QUEUE_MAX_MAX_WAITS,
            "排队等待次数",
            &mut patch.max_waits,
        ),
        (
            KEY_QUEUE_WAIT_SECONDS,
            object.get(KEY_QUEUE_WAIT_SECONDS),
            QUEUE_MIN_WAIT_SECONDS,
            QUEUE_MAX_WAIT_SECONDS,
            "排队等待秒数",
            &mut patch.wait_seconds,
        ),
    ];
    for (key, value, min, max, label, slot) in targets {
        let Some(value) = value else {
            continue;
        };
        // null 视为「这一项不改」：前端整份回传时未填项常见为 null
        if value.is_null() {
            continue;
        }
        match parse_bounded_int(key, value, min, max, label) {
            Ok(number) => {
                *slot = Some(number);
                updated = true;
            }
            Err(message) => return errors::management_error(400, message),
        }
    }

    if updated && !config::set_queue(patch) {
        // 写盘失败：内存快照已更新（本次运行仍生效），但重启后会回到旧值 ——
        // 必须让用户知道，否则「改了设置重启又变回去」会被当成玄学问题
        logging::log("[Config]", "⚠️  排队等待设置写入失败，本次运行内仍生效");
    }

    let settings = config::queue_settings();
    if updated {
        let wait = if settings.wait_seconds > 0 {
            format!("{} 秒", settings.wait_seconds)
        } else {
            "跟随上游建议".to_string()
        };
        logging::log(
            "[Config]",
            &format!("排队等待已更新: 最多 {} 次 / 单次 {wait}", settings.max_waits),
        );
    }
    ok_json(queue_json(settings))
}

/// 两项的响应体（GET 与 PUT 共用，键名用常量标识符而不是手写字符串 ——
/// 与 `timeouts_api::timeouts_json` 同一手法，键名对前端是契约）。
fn queue_json(settings: QueueSettings) -> Value {
    json!({
        KEY_QUEUE_MAX_WAITS: settings.max_waits,
        KEY_QUEUE_WAIT_SECONDS: settings.wait_seconds,
    })
}

/// 单个整数的校验：必须是范围内的整数，否则给出可读的 400 文案。
/// 与 `timeouts_api::parse_seconds` 同一口径（只认 JSON 数字，容忍 `2.0`
/// 这种整值浮点；字符串 `"2"` 视为非法）。
fn parse_bounded_int(
    key: &str,
    value: &Value,
    min: i64,
    max: i64,
    label: &str,
) -> Result<i64, String> {
    let number = match value {
        Value::Number(number) => number.as_i64().or_else(|| {
            number
                .as_f64()
                .filter(|raw| raw.is_finite() && raw.fract() == 0.0)
                .map(|raw| raw as i64)
        }),
        _ => None,
    };
    let Some(number) = number else {
        return Err(format!("{label}（{key}）必须是整数"));
    };
    if !(min..=max).contains(&number) {
        return Err(format!("{label}（{key}）必须在 {min}~{max} 之间"));
    }
    Ok(number)
}
