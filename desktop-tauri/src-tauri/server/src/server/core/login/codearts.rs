//! CodeArts 的网页登录收尾（portal 回调 → 授权码换凭据 / ticket 轮询兜底）。
//!
//! ── 这条链在八家登录里排第几种形态 ──────────────────────────
//! 与 Accio 同族（「浏览器带你回到本机 HTTP 端口」），但**配对方式不同**，
//! 差别全在上游那一侧：
//!   * Accio 的 `return_url` 由我们给，登录页把里面的 `state` 原样带回 → 按 state 查表；
//!   * CodeArts 的 portal **只认我们给的 `port`**，回调路径由它自己拼成
//!     `http://127.0.0.1:<port>/oauth/callback`，而且**实测可能只带一个 `code`**、
//!     不带任何配对信息（独立实现 `hitzy-codearts2api` 的注释原话）。
//! 所以这里不能只按 state 查，得「逐个候选试 verifier」：用错 verifier 会被 STS 判
//! `STS5.1805` 而**不消耗授权码**（同一处注释），试到通过为止是安全的。
//!
//! ── 两次回调 ────────────────────────────────────────────────
//! portal 先带 `secret` + `redirect` 回来一次，并要求我们 **307 跳到它给的 redirect**
//! （不跳，链路就停在半路、永远不会有 `code`）；随后再带 `code` 回来。
//! 第一次那趟顺手把 secret 记进待办，作为 ticket 轮询通道的钥匙。
//!
//! ── 为什么还要 ticket 轮询这条兜底 ──────────────────────────
//! 网关跑在另一台机器上（NAS / 服务器）时，浏览器跳的是**它自己**的 127.0.0.1，
//! 两次回调都到不了网关。此时用户把地址栏里那条 localhost 地址改成网关地址再走一遍
//! （界面上就是这么教的），粘回来的带的是 `secret` 而不是 `code` —— 只能靠
//! `GET {base}/snap-manager/v1/login/ticket` 换。注意这条通道**不返回 refresh token**，
//! 经它落账的账号约一小时后就得重新登录，文案要说清（见 `oauth::poll_ticket`）。

use serde_json::{Value, json};

use crate::server::core::account_store::AccountStore;
use crate::server::core::providers::codearts::oauth;
use crate::server::errors::GatewayError;
use crate::server::logging;

use super::{LoginService, LoginTaskHandle};

/// 回调处理结果，交给路由决定回什么给浏览器。
pub enum Callback {
    /// 307 到 portal 给的地址（第一次回调必须跳，否则不会有 `code`）。
    ContinueTo(String),
    /// 已受理：`(账号 id, 给用户看的文案)`。
    /// 账号 id 为 None 表示「还在 ticket 轮询里」—— 此时界面的 `/wait` 要继续等，
    /// 不能把「已收到」当成「已登录」。
    Accepted(Option<String>, String),
    /// 失败：状态码 + 给用户看的文案。
    Failed(u16, String),
}

impl LoginService {
    /// 处理一次 CodeArts 回调。`params` 是回调查询串（`code` / `secret` / `redirect` /
    /// 可选的 `ticket_id`）；**手工粘贴那条路也走这里**（把粘来的 URL 的查询串交上来）。
    pub async fn finish_codearts_login(&self, params: &std::collections::HashMap<String, String>) -> Callback {
        let get = |key: &str| params.get(key).map(|value| value.trim().to_string()).unwrap_or_default();
        let (code, secret, redirect, ticket) = (get("code"), get("secret"), get("redirect"), get("ticket_id"));

        // 每次回调都落一行**只说形状、不带秘密**的日志。没有这一行，「浏览器没跳回来」
        // 与「跳回来了但我们不认」在面板上长得一模一样（都是账号没出现），
        // 而这两种的处置完全不同：前者要用户改地址栏，后者是我们的判定错了。
        logging::log(
            "[Login]",
            &format!(
                "CodeArts 回调：has_code={} has_secret={} has_redirect={} ticket={} 待办轮次={}",
                !code.is_empty(),
                !secret.is_empty(),
                !redirect.is_empty(),
                if ticket.is_empty() { "—".to_string() } else { short(&ticket) },
                oauth::candidates().len(),
            ),
        );

        if !code.is_empty() {
            return self.settle_by_code(&ticket, &code).await;
        }
        if !secret.is_empty() {
            // portal 下发的 secret 要记到**它那一轮**（ticket 通道用的就是这个，
            // 不是我们自己生成的）。没带配对信息时整表都记一遍 —— 与 code 通道
            // 同一套「认不出是哪一轮就逐个试」的退化处理。
            let ids: Vec<String> = match ticket.is_empty() {
                true => oauth::candidates().into_iter().map(|item| item.ticket_id).collect(),
                false => vec![ticket.clone()],
            };
            let mut attached = false;
            for id in &ids {
                attached |= oauth::attach_ticket_secret(id, &secret);
            }
            if !attached {
                return Callback::Failed(404, format!("这一轮登录已超过 {} 分钟被作废（或已被取消），请回到面板重新发起", oauth::LOGIN_TIMEOUT_MS / 60_000));
            }
            if !redirect.is_empty() {
                // 第一趟必须原样转出去：这一跳是 portal 登录链路的一部分，不跳就没有下一步。
                // 但这个目的地来自**免鉴权路由的查询参数**，等于让浏览器听攻击者的话：
                // 不校验就是把本网关做成一个开放重定向（还挂着华为云登录这一跳的品牌信任）。
                return match trusted_redirect(&redirect) {
                    true => Callback::ContinueTo(redirect),
                    false => Callback::Failed(
                        400,
                        "回调要求跳转到的地址不是华为云站点，已拒绝（本网关不替任意地址转发浏览器）".to_string(),
                    ),
                };
            }
            // 没有 redirect = 用户手工粘回来的（浏览器那侧本来就到不了网关）→ 转 ticket 轮询
            self.start_ticket_poll(&ids);
            return Callback::Accepted(
                None,
                "已收到 portal 下发的 secret，正在通过 ticket 通道换取凭证。这条通道拿不到 refresh token，该账号约一小时后需重新登录。"
                    .to_string(),
            );
        }
        Callback::Failed(400, "回调里没有授权码也没有 secret：请把浏览器地址栏里那条回调地址完整粘贴过来".to_string())
    }

    /// 授权码通道：先按 ticket 精确试，再按「最近发起」逐个试。
    async fn settle_by_code(&self, ticket: &str, code: &str) -> Callback {
        let mut ordered: Vec<oauth::PendingLogin> = Vec::new();
        let candidates = oauth::candidates();
        if !ticket.is_empty() {
            ordered.extend(candidates.iter().filter(|item| item.ticket_id == ticket).cloned());
        }
        // 其余候选按「最近发起」排在后面（先试最可能是的那一轮）
        for item in candidates.iter().filter(|item| item.ticket_id != ticket) {
            ordered.push(item.clone());
        }
        if ordered.is_empty() {
            return Callback::Failed(404, format!("这一轮登录已作废（超过 {} 分钟或已被取消），请回到面板重新发起", oauth::LOGIN_TIMEOUT_MS / 60_000));
        }
        let mut last = String::new();
        for candidate in ordered {
            // **只看不取**：换码失败时这一轮必须留在表里。`/oauth/callback` 是一条
            // 免鉴权的公开路由，任何人打一次「随便编个 code」就能把用户正在进行的
            // 登录全部作废 —— 取走式写法等于把「取消别人的登录」做成免费服务。
            let Some(pending) = oauth::peek_pending(&candidate.ticket_id) else { continue };
            match exchange_and_store(&self.store, &pending, code).await {
                Ok(account_id) => {
                    // 成功才取走：授权码与 verifier 都是一次性的，留着只会被再试一次
                    oauth::take_pending(&candidate.ticket_id);
                    let handle = self.tasks.get(&candidate.ticket_id);
                    mark_done(handle.as_ref(), &account_id);
                    return Callback::Accepted(Some(account_id), "登录成功，账号已加入列表，可以关闭此页面。".to_string());
                }
                Err(error) => {
                    logging::log(
                        "[Login]",
                        &format!("CodeArts 换码候选 {} 未通过：{}", short(&candidate.ticket_id), error.message),
                    );
                    last = error.message;
                }
            }
        }
        // 只有回调**点名**了哪一轮，才把那轮的任务判失败（界面立刻拿到错误，
        // 不用干等到超时）。没点名时一个都不动：那些轮次可能还在等真码。
        if !ticket.is_empty() {
            if let Some(handle) = self.tasks.get(ticket) {
                super::finish_task_error(&handle, &last);
            }
        }
        Callback::Failed(502, format!("授权码换取凭证失败：{last}"))
    }

    /// ticket 通道的后台轮询：每 2 秒一次，到本轮登录超时为止。
    ///
    /// 为什么在后台而不是就地等：portal 那边用户可能还没点完，这条通道会连着 404 一阵；
    /// 把它压进一次 HTTP 请求，浏览器先超时，用户看到的是「请求失败」而不是「还在等」。
    fn start_ticket_poll(&self, tickets: &[String]) {
        let Some(ticket_id) = tickets.first().cloned() else { return };
        // 一轮只允许一个轮询循环（免鉴权路由上可被反复触发，见 PendingLogin 的注释）
        if !oauth::claim_ticket_poll(&ticket_id) {
            return;
        }
        let Some(handle) = self.tasks.get(&ticket_id) else { return };
        let store = self.store.clone();
        crate::spawn_task(async move {
            let deadline = logging::now_ms() + oauth::LOGIN_TIMEOUT_MS as i64;
            loop {
                // 只读一份来轮：换成功才从表里取走，失败时授权码通道还要能认得出这一轮
                let Some(pending) = oauth::peek_pending(&ticket_id) else { return };
                if logging::now_ms() >= deadline {
                    logging::log("[Login]", "CodeArts ticket 通道超时：仍未换取到凭证，请重新发起登录");
                    super::finish_task_error(&handle, "登录超时：ticket 通道没能换取到凭证，请重新发起");
                    return;
                }
                match oauth::poll_ticket(
                    crate::server::core::providers::codearts::models::DEFAULT_BASE_URL,
                    &pending,
                    None,
                ).await {
                    Ok(mut credential) => {
                        let Some(_claimed) = oauth::take_pending(&ticket_id) else { return };
                        // ticket 通道不返回 refresh token / 上下文，这里把**本轮**生成的
                        // PKCE 与 DPoP 补进去：虽然上游没给 refresh token 因而续不了，
                        // 但上下文与凭据同源，留着它至少不会让「凭据形状」看起来缺半块
                        credential.oauth_context = Some(pending.context.clone());
                        match store.add_codearts_account(&credential, None, "web-login") {
                            Ok(public) => {
                                let id = public.get("id").and_then(Value::as_str).unwrap_or_default().to_string();
                                logging::log(
                                    "[Login]",
                                    "CodeArts 经 ticket 通道落账（该通道不带 refresh token，约一小时后需重新登录）",
                                );
                                mark_done(Some(&handle), &id);
                                return;
                            }
                            Err(error) => {
                                super::finish_task_error(&handle, &error.message);
                                logging::log("[Login]", &format!("❌ CodeArts ticket 凭据落账号失败：{}", error.message));
                                return;
                            }
                        }
                    }
                    Err(error) => {
                        // 408 是这条链的常态（portal 那边还没就绪），其余才值得记一句
                        if error.status_code != 408 {
                            logging::log("[Login]", &format!("CodeArts ticket 通道：{}", error.message));
                        }
                    }
                }
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            }
        });
    }

    /// 取消一轮登录时把待办也清掉（否则那一轮的 verifier 会占到超时才还）。
    pub fn drop_codearts_pending(&self, ticket: &str) {
        oauth::drop_pending(ticket);
    }
}

/// 用授权码换凭据并落账号 —— 与「粘贴凭据」共用 `add_codearts_account`，
/// 不另写一份落盘逻辑（两条路各写一份，账号形状迟早会分叉）。
async fn exchange_and_store(store: &AccountStore, pending: &oauth::PendingLogin, code: &str) -> Result<String, GatewayError> {
    let credential = oauth::exchange_for(pending, code, None).await?;
    let public = store
        .add_codearts_account(&credential, None, "web-login")
        .map_err(|error| GatewayError::with_status(error.status_code, error.message))?;
    Ok(public.get("id").and_then(Value::as_str).unwrap_or_default().to_string())
}

/// 把任务句柄标成完成（界面的 `/wait` 靠它收尾）。
fn mark_done(handle: Option<&LoginTaskHandle>, account_id: &str) {
    let Some(handle) = handle else { return };
    handle.update(|task| {
        task.done = true;
        task.session = Some(json!({
            "accountUid": account_id,
            "provider": crate::server::core::account_store::codearts_accounts::CODEARTS_PROVIDER_ID,
        }));
        task.finished_at = Some(logging::now_ms());
    });
}

/// 回调可以要求我们跳转的目的地：**只认华为云自己的站点**。
///
/// 判定用解析后的 host，不用字符串前缀 —— `https://huaweicloud.com.evil.tld/`
/// 这种写法能骗过后缀匹配之外的所有朴素写法。端口不参与判定（portal 回跳带端口
/// 的情况没见过，但即便有也不是安全边界）。
fn trusted_redirect(url: &str) -> bool {
    let Ok(parsed) = url::Url::parse(url) else { return false };
    if !matches!(parsed.scheme(), "https" | "http") {
        return false;
    }
    match parsed.host_str() {
        Some(host) => {
            let host = host.to_ascii_lowercase();
            host == "huaweicloud.com" || host.ends_with(".huaweicloud.com")
        }
        None => false,
    }
}

/// 日志里 ticket 取前 8 位就够定位（整串 32 个十六进制会把日志行撑得没法读）。
fn short(ticket: &str) -> String {
    ticket.chars().take(8).collect()
}

#[cfg(test)]
mod tests {
    //! 回调状态机用**进程内 mock 上游**跑（与 `session.rs` / `chat.rs` 同一手法）：
    //! 三条出口的判定与「portal 到底带不带配对信息」这件事，都不需要真登录一次
    //! 才能验；真登录那条留给界面验收（它要人点鼠标）。
    use std::collections::HashMap;

    use crate::server::core::account_store::AccountStore;
    use crate::server::core::auth::AuthService;
    use crate::server::core::providers::codearts::oauth;
    use crate::server::db::Db;

    use super::*;

    fn params(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    /// 每个测试一个临时库。**必须**带进程内序号：只用时间戳的话，并行跑的
    /// 两个测试可能读到同一个纳秒值，于是共用一个库文件 —— 第二个 `Db::open`
    /// 会在同一个 schema 上再跑一遍迁移，报出与测试内容毫无关系的
    /// 「duplicate column name」。
    fn service() -> LoginService {
        static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let id = SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("codearts-login-{}-{id}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let store = AccountStore::with_db(Some(Db::open(&dir.join("agent2api.db")).expect("临时库应当能建起来")));
        LoginService::new(AuthService::for_store(store.clone()), store)
    }

    /// 发起一轮登录并返回它的 ticket（= 任务 state）。
    fn begin() -> String {
        oauth::set_loopback_port(13_999);
        let (_url, pending) = oauth::begin_login("snap_vscode", "26.9.101", "en-us").expect("端口已设，应当能发起");
        pending.ticket_id
    }

    fn matches_ref(actual: &Callback, expected: &str) -> bool {
        match (actual, expected) {
            (Callback::ContinueTo(_), "continue") => true,
            (Callback::Failed(400, _), "400") => true,
            (Callback::Failed(404, _), "404") => true,
            (Callback::Accepted(_, _), "accepted") => true,
            _ => {
                let shape = match actual {
                    Callback::ContinueTo(url) => format!("ContinueTo({url})"),
                    Callback::Accepted(id, message) => format!("Accepted({id:?}, {message})"),
                    Callback::Failed(status, message) => format!("Failed({status}, {message})"),
                };
                panic!("期望 {expected}，实际 {shape}");
            }
        }
    }

    #[tokio::test]
    async fn first_callback_hops_to_the_portal_and_keeps_the_secret() {
        let login = service();
        let ticket = begin();
        let outcome = login
            .finish_codearts_login(&params(&[("secret", "portal-secret"), ("redirect", "https://codearts.huaweicloud.com/portal/done"), ("ticket_id", &ticket)]))
            .await;
        assert!(matches_ref(&outcome, "continue"), "第一趟必须把浏览器转回 portal，否则永远等不到 code");
        // 转出去之后这一轮还在表里，且 secret 已经记上（ticket 通道的钥匙）
        let kept = oauth::peek_pending(&ticket).expect("这一轮不该被取走");
        assert_eq!("portal-secret", kept.secret, "portal 下发的 secret 要记到它那一轮");
    }

    #[tokio::test]
    async fn a_secret_for_an_unknown_ticket_is_not_invented() {
        let login = service();
        let outcome = login
            .finish_codearts_login(&params(&[("secret", "s"), ("ticket_id", "no-such-ticket")]))
            .await;
        assert!(matches_ref(&outcome, "404"), "认不出的 ticket 要说「重新发起」，不能凭空建一轮");
    }

    #[tokio::test]
    async fn a_callback_with_neither_code_nor_secret_says_what_to_paste() {
        let login = service();
        let outcome = login.finish_codearts_login(&params(&[])).await;
        assert!(matches_ref(&outcome, "400"), "两个都没有就是粘错了地址");
    }

    #[tokio::test]
    async fn a_code_without_any_pending_round_is_reported_as_expired() {
        let login = service();
        // 用一个几乎不可能撞上的 ticket，且表里此时可能有别家测试的轮次 ——
        // 因此这条只验「按 ticket 找不到、且逐个候选都换码失败」的形状：
        // 候选为空时才是 404；不为空时会去打 STS（本测试不覆盖那条，见模块头）。
        let outcome = login.finish_codearts_login(&params(&[("code", "x"), ("ticket_id", "zz-unknown")])).await;
        let shape = match outcome {
            Callback::Failed(status, _) => status,
            Callback::Accepted(..) => 200,
            Callback::ContinueTo(_) => 307,
        };
        assert!([404, 502].contains(&shape), "没有待办就该 404，有则去换码（换不动是 502），实际 {shape}");
    }

    #[test]
    fn ticket_response_is_read_in_the_official_shape() {
        let full = r#"{"credential":{"access":"AK1","secret":"SK1","securitytoken":"STS1","expires_at":"2026-09-27T16:17:00.327Z"},"domain_id":"d","user_id":"u","user_name":"n","login_type":"WEB"}"#;
        let parsed = oauth::parse_ticket_credential(full).expect("官方形状应当能解");
        assert_eq!(("AK1", "SK1", "STS1"), (parsed.access_key_id.as_str(), parsed.secret_access_key.as_str(), parsed.security_token.as_str()));
        assert_eq!(("d", "u", "n", "WEB"), (parsed.domain_id.as_str(), parsed.user_id.as_str(), parsed.user_name.as_str(), parsed.login_type.as_str()));
        // 这条通道**不给** refresh token —— 不是解析漏了，是上游就没有（见函数注释）
        assert!(parsed.refresh_token.is_empty());
        assert!(parsed.oauth_context.is_none());

        for (name, body) in [
            ("没有 credential 段", r#"{"domain_id":"d"}"#),
            ("临时凭据不完整", r#"{"credential":{"access":"AK"}}"#),
            ("不是 JSON", "not json"),
        ] {
            assert!(oauth::parse_ticket_credential(body).is_err(), "{name} 必须判失败");
        }
    }

    #[tokio::test]
    async fn ticket_channel_maps_not_ready_to_a_retryable_status() {
        // 404（portal 那边还没就绪）要翻成 408：轮询循环据此「继续等」，
        // 而 5xx / 解析失败才该立刻报给用户
        let (base, _hits) = mock_ticket_server(404, r#"{"error":"not ready"}"#).await;
        let pending = oauth::PendingLogin {
            ticket_id: "t".to_string(),
            context: Default::default(),
            callback_url: "http://127.0.0.1:13999/oauth/callback".to_string(),
            secret: "s".to_string(),
            started_at_ms: 0,
            poll_claimed: false,
        };
        let error = oauth::poll_ticket(&base, &pending, None).await.expect_err("404 应当是错误");
        assert_eq!(408, error.status_code, "没就绪要翻成可重试的 408，实际 {}", error.status_code);

        // secret 还没拿到时**不发请求**（本地就能判，省一次无谓的上游调用）
        let mut bare = pending.clone();
        bare.secret = String::new();
        assert!(oauth::poll_ticket(&base, &bare, None).await.unwrap_err().message.contains("secret"));
    }

    /// 起一个假的 ticket 端点，返回它的 base（`{base}/snap-manager/v1/login/ticket`）。
    async fn mock_ticket_server(status: u16, body: &'static str) -> (String, ()) {
        use axum::http::StatusCode;
        use axum::response::IntoResponse;
        let app = axum::Router::new().route(
            "/snap-manager/v1/login/ticket",
            axum::routing::get(move || async move { (StatusCode::from_u16(status).unwrap(), body).into_response() }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (format!("http://{addr}"), ())
    }
}
