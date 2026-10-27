# OrcaTerm 会话粘性 + 反问自动确认 — 设计规格

日期：2026-10-24
状态：已批准（用户确认：治本，让 OrcaTerm 能跑 agent 任务，不要每次咨询确认）

## 1. 背景与问题

用户经网关把 agent 客户端（模型 deepseek-v4-pro）接到 OrcaTerm 上游跑多步任务时，
陷入死循环：模型给方案 → 问「是否按这个思路开始执行?」→ 用户回「是」→ 模型**再给
一遍方案再问一次**，永不执行。两个结构性原因：

1. **每轮都是全新上游会话**：适配器是「请求内会话」（每个请求
   `POST /assistant/conversation` 新建会话，多轮历史拍平成一条文本）。上游 agent
   的「已批准」状态存在于服务端会话里，跨轮无法存活。
2. **上游产品人设是「先方案后动手」**：响应信封自带 `action=ask` 反问确认动作；
   没有会话粘性时，用户的「是」永远落不到它刚问过的那个会话里。

参考 [ZFXing-lite/orcaterm2api](https://github.com/ZFXing-lite/orcaterm2api)（协议
同源，cirnosalt/orcabridge 的研究）验证过的两板斧：**会话复用** + **NoAsk 行为指令 /
ask 自动确认**。

## 2. 上游协议要点（源自参考实现逐行核对）

- **ACP 信封**：`input.input` 可为 JSON 串
  `{"_format":"acp-prompt","version":1,"prompt":[{"type":"text","text":…},…]}`，
  多块 prompt 视为同一轮输入；**首块可作为行为指令**（桌面客户端同样以首块注入
  环境指令）。纯文本 input 也被接受（本仓库 2026-10-16 实测）。
- **会话续写**：同一 `conversationId` 连发多轮 `start` 能接上上下文（本仓库
  2026-10-16 实测；参考项目的工具结果回填同会话同机制）。
- **反问动作**：`{"action":"ask","question":"…"}` —— HTTP 200 正常结束。参考项目
  对它的处置：注入 NoAsk 行为指令抑制；仍 ask 时网关**自动回填一轮**
  「是的，请直接执行，无需再向我确认。」到**同一会话**（仅一次），取第二轮结果；
  第二轮仍 ask 则把问题原样交给下游。
- NoAsk 行为指令（参考原文，实测有效）：
  > 【运行模式】你正处于无人值守模式：默认直接执行用户的任务并给出最终结果，不要反问用户、不要请求确认、不要要求用户补充信息（信息不足时自行做出合理假设并在结果开头简要说明所做假设）；仅当用户明确表示想讨论、征求意见或等待你确认时，才允许向用户提问。

## 3. 方案决策

| 机制 | 结论 | 理由 |
| --- | --- | --- |
| 会话粘性（键 → conversationId 注册表） | **采纳** | 「确认」落到上游已有会话，多轮才成立 |
| 续轮只发**增量**（最后一条 assistant 之后的新消息） | **采纳** | 上游已存历史，全量重放会污染上下文 |
| NoAsk 行为指令（ACP 首块） | **采纳** | 直接抑制反问倾向；措辞取参考项目实战原文 |
| ask 自动确认（同会话补一轮，仅一次） | **采纳** | 指令外的兜底；ask 信封天然走缓冲模式，拦截窗口免费 |
| review→tool_calls 映射（工具闭环） | **本轮不做** | 上游只认原生工具名（终端/远端文件），客户端 agent 声明的是自己的工具集，映射关系无依据；先落地粘性+确认，工具闭环另立规格 |
| 键与账号绑定 | **采纳** | conversation 是**账号作用域**的，换账号续写必然失配；记录里存 account_id，不复用不一致的 |

「agentic 判定」（行为指令与自动确认的生效条件）：请求体声明了非空 `tools`，
**或**可用消息数 ≥ 2。单条消息的闲聊不注入指令、不自动确认（保持自然反问）。

### 3.1 会话键取值链（移植 opencode 适配器既有实现，四档）

1. 入站头：`x-session-id` / `x-conversation-id` / `x-thread-id`；
2. 请求体字段：`session_id/sessionId/conversation_id/conversationId/thread_id/threadId`（含 `metadata.*`）；
3. 内容哈希：**messages 里第一条 system + 第一条 user** 的 SHA-256（同一会话这两段稳定）；
4. 以上皆无 → 随机档：不入表，每请求新建会话（与改造前行为一致）。

### 3.2 增量输入（续轮）

`build_delta_prompt`：取**最后一条 assistant 消息之后**的消息（通常是用户确认 /
工具结果 / 新指令），按 `[用户]/[工具结果]/[系统指令]` 标记拼成单条输入。锚点不
存在（首轮）或增量为空 → `None` → 走全新会话全量拍平。

### 3.3 失效自愈

- 续会话路径打开流失败（401 续期重试一次后仍失败）：**丢弃该会话记录**，回落
  全新会话 + 全量拍平重试一次；再失败才报错。
- 全新会话成功建出后写入注册表（覆盖旧值）。

## 4. 改动清单

路径：`$P` = `desktop-tauri/src-tauri/server/src/server/core/providers`。

| # | 文件 | 改动 |
| --- | --- | --- |
| 1 | `$P/orcaterm/session.rs`（新建） | 会话键解析（四档）+ conversation 注册表（上限 256，插入序裁最旧，键含账号校验）+ 单测 |
| 2 | `$P/orcaterm/chat.rs` | `run_chat` 增加 `client_headers` 入参；粘性命中→增量续会话、未命中→新建并登记；`build_delta_prompt` / `wrap_acp` / `is_agentic_request` / `AGENT_NOTE` / `AUTO_CONFIRM_TEXT`；`OrcaStream` 的 Done/Error 事件改为**只记状态不产收尾帧**，收尾帧移到显式 `finalize`（ask 自动确认需要拦截窗口）；`unwrap_envelope_parts` 暴露 action；流式与非流式两条驱动都接 ask 自动确认 |
| 3 | `$P/orcaterm/adapter.rs` | `forward_conversation` 把 `client_headers` 传给 `run_chat`；模块头更新（会话不再用毕即弃） |
| 4 | `$P/orcaterm/mod.rs` | `pub mod session;`；模块头「请求内会话」段落改写为粘性语义 |
| 5 | 本文档 | — |

不改动：账号存储、登录、续期、余额、模型表、编排层（`provider_loop` 调用点本就传
headers，trait 签名不变）。

## 5. 错误处理与边界

- **跨账号**：注册表记录 account_id，取用时不一致视为未命中（并覆盖写新会话）。
- **并发**：同会话并行请求可能各建各的会话，后写覆盖先写——上游无事务语义，可接受。
- **注册表淘汰**：256 条按插入序裁最旧；被裁会话的下一次请求自动回落全新会话。
- **ask 自动确认只兜一次**：第二轮仍 ask → 问题原样下发；用户回答经粘性会话续写
  （这正是粘性的价值——手动确认也能接上）。
- **自动确认第二段失败**：回落下发原问题，不报错（用户手动回答的路径仍然可用）。
- **收尾帧语义保持**：Done→finish_reason=stop；流内 ERROR→无 finish_reason
  （与既有口径一致），正文为空的 ERROR 仍下发显式错误帧。
- `panic=abort`：零 unwrap/expect/panic，锁不跨 await（注册表锁内只做查/插/裁）。

## 6. 测试计划

- 单测（session.rs）：键四档优先级、normalize 拒绝非法 id、账号不一致不复用、
  256 裁最旧、drop 后未命中。
- 单测（chat.rs）：`build_delta_prompt`（首轮 None / 锚点后增量 / tool 角色标记 /
  全空消息 → None）、`wrap_acp`（带/不带指令块的信封形状）、
  `is_agentic_request`（tools 非空 / 多消息 / 单消息）、
  `unwrap_envelope_parts`（ask/review/completion）。
- `cargo test` 全绿；`cargo build` 通过。
- 手动冒烟：agent 客户端接 OrcaTerm 发多步任务 → 观察不再反问；手动回「是」→
  同会话续写（日志有「粘性命中」verbose）。
