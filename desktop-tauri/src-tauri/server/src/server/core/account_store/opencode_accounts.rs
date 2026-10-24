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
