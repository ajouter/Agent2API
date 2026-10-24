//! OpenCode（opencode.ai Zen API）适配实现：**无状态 OpenAI 兼容转发 + 粘贴式 API Key**。
//!
//! ── 上游长什么样（逆向来源：`LIMTCYT/opencode-go-proxy-for-trae` 的
//! 单文件 Python 代理 + opencode.ai 官方文档 <https://opencode.ai/v2/docs/console/go/>）
//! ────────────────────────────────────────────────────────────
//! OpenAI 兼容协议（`POST /zen/go/v1/chat/completions`），凭
//! `Authorization: Bearer <API Key>` 鉴权（opencode.ai 控制台获取，无 OAuth、
//! 无续期）。每请求必须带四个会话头（缺 `x-opencode-session` 上游 400）：
//! `x-opencode-session` / `x-opencode-request` / `x-opencode-client: cli` /
//! `x-opencode-project: global`，另注入 UA `opencode/1.18.29 cli`。
//! 模型目录 `GET /zen/go/v1/models`（公开、无需鉴权，2026-10-24 实测）。
//!
//! ── 与参考实现的差别（原生的改进，别当漏抄）────────────────────
//!   - 参考代理要用户按模型手填不同 Base URL（当时两条路由清单不同）；
//!     2026-10-24 实测两条路由的 `/models` 已完全一致，故固定走推荐的
//!     `/zen/go/v1`（新路由 `/inference/go/v1` 间歇性 429，参考实现同口径）；
//!   - 会话键与账号无关：网关多账号轮转时同一会话仍映射同一个 session id，
//!     上游提示词缓存路由不受换号影响（参考代理单 key，无此问题也无此能力）；
//!   - 参考代理的心跳保活 / 竞速重试 / 429 指数退避不移植：编排层
//!     （`upstream::provider_loop`）已有重试、轮转与冷却，多账号场景下
//!     「换号重试」优于「同号死等」；`proxy-` 模型别名同理不移植
//!     （网关有自己的模型映射体系）。
//!
//! ── 子模块分工 ──────────────────────────────────────────────
//! ```text
//!   endpoints.rs    基址 / 路径 / UA / 四个会话头的构造
//!   session.rs      会话解析与请求计数（x-opencode-session / x-opencode-request）
//!   credentials.rs  账号记录 → 凭证快照；脱敏
//!   login.rs        粘贴归一化 + baseUrl 校验
//!   models.rs       GET /zen/go/v1/models（5 分钟缓存 + 内置 37 项兜底）
//!   adapter.rs      ProviderAdapter 实现（装配层）
//! ```
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本目录零 unwrap/expect/panic。

pub mod adapter;
pub mod credentials;
pub mod endpoints;
pub mod login;
pub mod models;
pub mod session;

pub use adapter::{OpenCodeAdapter, OPENCODE_ADAPTER};
