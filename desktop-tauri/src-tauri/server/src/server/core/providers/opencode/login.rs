//! OpenCode 粘贴归一化：剥引号 / `Authorization: Bearer` 前缀 / 空白 + baseUrl 校验。
//!
//! 上游对 key 没有可判的本地形态（见 `credentials` 的模块头），所以这里**不做
//! 前缀校验**：归一化后非空即通过，真实有效性由上游生成请求时的 401/403 回答
//! （`classify_error` 的 TokenExpired 档）。也没有 Command Code 那种添加探活 ——
//! 本家没有免费只读端点可探（`/models` 公开、不校验 key，探不出无效）。
//!
//! ── `Bearer` 剥离的边界（刻意为之，别当漏写）────────────────────
//! `bearer` 前缀**仅当其后紧跟空白（空格/制表）**才剥：`Authorization: Bearer x`
//! 与裸 `Bearer x` 都会被剥干净，但 `bearerabc` 这种以 bearer 开头的 key 若
//! 无条件剥前缀会被腰斩成 `abc` —— 前缀判定必须看边界。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic。

use serde_json::{json, Value};

use crate::server::errors::GatewayError;

use super::credentials;

/// 对 `candidate` 自身做 ASCII 大小写不敏感的前缀匹配，命中时返回剩余串。
///
/// 只在 `candidate` 自己的字节上比较（`eq_ignore_ascii_case` 只折叠 ASCII
/// 字母）：命中意味着前 `prefix.len()` 字节逐个等于 ASCII 字母/冒号 —— ASCII
/// 字节不可能是 UTF-8 续字节，故 `prefix.len()` 处必是字符边界，切片安全。
/// **不要**改成「对整串 `to_lowercase` 后跨两个字符串做长度算术」：非 ASCII
/// 字符的小写映射会改字节长度（如 U+212A K→k，3 字节→1 字节），偏移错位后
/// `candidate[skipped..]` 可切进 UTF-8 序列中间 → panic（release 下 abort）。
fn strip_prefix_ascii<'a>(candidate: &'a str, prefix: &str) -> Option<&'a str> {
    let bytes = candidate.as_bytes();
    if bytes.len() >= prefix.len()
        && bytes[..prefix.len()].eq_ignore_ascii_case(prefix.as_bytes())
    {
        return Some(&candidate[prefix.len()..]);
    }
    None
}

/// 归一化一段粘贴内容：剥成对引号 / `Authorization: Bearer` 前缀 / 空白。
pub fn normalize_paste(raw: &str) -> String {
    let mut value = raw.trim();
    for (open, close) in [("\"", "\""), ("'", "'")] {
        if value.len() >= 2 && value.starts_with(open) && value.ends_with(close) {
            value = value[1..value.len() - 1].trim();
        }
    }
    // 整行 `Authorization: …`（大小写不敏感；冒号本身就是分隔符）。剥完的剩余串
    // 再走同一条带空白判定的 Bearer 剥离 —— 两级剥离语义不变
    // （`Authorization: Bearer x` 的 `authorization:` 剥完剩 `Bearer x`）。
    let mut candidate = value;
    if let Some(rest) = strip_prefix_ascii(candidate, "authorization:") {
        candidate = rest.trim();
    }
    // 裸 `Bearer …`（大小写不敏感）：仅当其后紧跟空白才剥（见模块头），
    // `bearerabc` 这样的 key 原样保留。
    if let Some(rest) = strip_prefix_ascii(candidate, "bearer") {
        if rest.starts_with(' ') || rest.starts_with('\t') {
            candidate = rest.trim();
        }
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
        // `bearer` 后不跟空白就不是前缀（控制者裁定）：key 可能以 bearer 开头
        assert_eq!(normalize_paste("bearerabc"), "bearerabc");
        assert_eq!(normalize_paste(""), "");
    }

    /// 回归（审查轮 1）：U+212A KELVIN SIGN 的小写映射改字节长度（3 字节→1）。
    /// 旧实现跨两个字符串做 `candidate.len() - rest.len()` 算术，`skipped`
    /// 被尾部长度差带偏，`candidate[skipped..]` 切进 UTF-8 序列中间 → panic。
    /// 新实现只对 `candidate` 自身做 ASCII 前缀匹配（见 `strip_prefix_ascii`），
    /// 两种输入都不 panic，且 key 本体（含 U+212A）原样保留、不被小写化。
    #[test]
    fn normalize_survives_lowercase_shrinking_chars() {
        assert_eq!(normalize_paste("Authorization: K\u{212A}"), "K\u{212A}");
        assert_eq!(normalize_paste("Bearer K\u{212A}"), "K\u{212A}");
        // 混合大小写的 Bearer 前缀 + 尾部收缩字符：前缀照剥、剩余串不动
        assert_eq!(normalize_paste("BeArEr K\u{212A}"), "K\u{212A}");
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
