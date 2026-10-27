//! OrcaTerm（腾讯云遨驰终端桌面客户端，`orcaterm.cloud.tencent.com`）适配器。
//!
//! ── 上游长什么样（2026-10-16 桌面客户端 1.6.4 + Web 版实测，见本目录各模块头）──
//! OrcaTerm 桌面端是 Tauri 应用（`orcaterm_desktop_lib`），微信 OAuth 登录后
//! 拿到一枚约 2 小时时效的 JWT（`oauth_access_token`）。它的 AI 助手
//! （OrcaTerm Agent）不走终端的 WebSocket，而是走一套独立的
//! 「assistant」HTTP + SSE 接口（`lightai.cloud.tencent.com`，与 CodeBuddy
//! 的 agent 网关同域），鉴权就是那枚 JWT 的 Bearer 头：
//!
//! ```text
//! POST {LIGHTAI}/assistant/conversation                       建会话
//! POST {LIGHTAI}/assistant/chat?name=orcaterm&mode=0&csrfCode=&sseresume=true
//!      → SSE：data: {content|reasoningContent 增量} … data: [DONE]
//! POST {CGI}/cgi/api?action=GetUserInfo&uin=<userId>          账号信息（探活）
//! POST {CGI}/cgi/api?action=DescribeUserBillingStatus&uin=…   月度配额（余额）
//! POST {CGI}/cgi/api?action=OAuthRefreshToken&uin=…           续期（旧 token 换新）
//! ```
//!
//! ── 凭证 ──────────────────────────────────────────────────
//! JWT 明文存在 `%APPDATA%\com.orcaterm-desktop.app\data.bin`（Tauri store，
//! 无加密），可直接做「导入本机登录态」。上游有真续期接口
//! （`OAuthRefreshToken`：传旧 accessToken 换新 token），因此
//! `supports_refresh = true`，由网关单飞续期并回写账号记录
//! （**不回写 data.bin**：那是桌面客户端自己的数据，客户端在线时随时可能
//! 覆盖它 —— 账号记录自添加那一刻起由网关接管续期）。
//!
//! ── 对话链路：会话粘性 + 增量输入（2026-10-24 起，见 session.rs）──
//! 上游按 `conversationId` 在服务端留存历史，且实测同会话连发多轮 `start`
//! 能接上上下文（输入通道只有 `start` 一条，没有「追加历史」接口，但同一
//! 会话可以继续发 `start`）。粘性注册表把同一客户端会话续写到同一个上游
//! 会话：确认 / 结果落进上游 agent 刚问过的那个会话，多步 agent 任务才推得
//! 动（此前「每请求新建会话 + 全量拍平」会让 agent 客户端陷入「方案→确认→
//! 再方案」死循环）。反问抑制见 `chat.rs` 的两道闸（行为指令 + ask 自动
//! 确认）；`is_stateful()` 为 true 的含义不变：一次转发由适配器自己完成
//! 多步时序（建会话 → SSE 流）。
//! 好处：确认与结果跨轮不丢，重启后最多丢一次粘性（自动回落全新会话）。
//!
//! ── 模型 ──────────────────────────────────────────────────
//! 模型 id 形如 `TokenHub/glm-5.3-flash`（桌面端代码缓存 + 真实对证实测，
//! 见 `models.rs`）。上游没有模型清单接口（面板列表来自客户端内置），
//! 因此 `supports_model_refresh = false`、静态兜底清单，未知模型显式回退
//! 默认模型（上游对未知名回流内 `LLM_ERROR`，显式回退更清晰）。
//!
//! ── 余额 ──────────────────────────────────────────────────
//! `DescribeUserBillingStatus` 返回按月重置的 `hunyuan_ai` 配额
//! （基础版 1000 次/月、专业版 6000 次/月），归一成统一的 usage 形状
//! （见 `balance.rs`）。
//!
//! ── 工具 / 审批 ───────────────────────────────────────────
//! 上游是 agent 协议（mcpServers / approvedTools / 审批流），网关侧全部置空
//! （`hideToolMessage: true`）：不做工具注入，纯文本直通 —— 与 KukuAI
//! 「上游有产品级人格保护，注入式协议不可靠」同一结论。
//!
//! ── 硬约束 ────────────────────────────────────────────────
//! release 是 `panic=abort`：本模块零 unwrap/expect/panic，不持锁跨 await。

pub mod adapter;
pub mod balance;
pub mod chat;
pub mod credentials;
pub mod http;
pub mod login;
pub mod models;
pub mod session;

/// CGI 网关（账号 / 配额 / 续期；Bearer JWT 鉴权，2026-10-16 实测）
pub const CGI_BASE: &str = "https://api.orcaterm.cloud.tencent.com";

/// AI 助手网关（对话 + 会话管理；Bearer JWT 鉴权，跨源自 orcaterm 站点调用）
pub const LIGHTAI_BASE: &str = "https://lightai.cloud.tencent.com";

/// 桌面端登录态文件（Tauri store，明文 JSON；`oauth_access_token` +
/// `oauth_expires_at` + `auth_identity`）
pub const DESKTOP_DATA_FILE: &str = "com.orcaterm-desktop.app/data.bin";

/// 出站 User-Agent（Web 版同款；上游不敏感，但不能用 reqwest 默认值）
pub const USER_AGENT: &str = concat!(
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 ",
    "(KHTML, like Gecko) Chrome/155.0.0.0 Safari/537.36"
);

/// assistant 接口的固定 query（桌面端 / Web 版一致；csrfCode 为空可用）
pub const ASSISTANT_QUERY: &str = "name=orcaterm&mode=0&csrfCode=&sseresume=true";

/// 默认模型（上游自动选路形态未知，显式回退到实测可用的默认档）
pub const DEFAULT_MODEL: &str = "TokenHub/glm-5.3-flash";

/// 进程级实例（`adapter_for` 返回它的 `&'static` 引用）
pub static ORCATERM_ADAPTER: adapter::OrcatermAdapter = adapter::OrcatermAdapter;

/// 本家 provider id（`providers::kind_id` 的常量形态，供账号存储使用）
pub fn orcaterm_id() -> &'static str {
    crate::server::core::providers::kind_id(crate::server::core::providers::ProviderKind::OrcaTerm)
}
