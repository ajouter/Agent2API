//! 内部 Chat 体的**历史 sanitize**：把客户端持久化下来的畸形工具历史修成
//! 严格上游能接受的形态。
//!
//! ── 解决什么问题（真实会话里的两种形态）──────────────────────
//! 客户端会把上游偶发产出的畸形工具调用原样写进会话历史，坏历史此后被每次
//! 请求原样重放 —— 严格校验的上游对**之后每一条**请求都返回 400，整条会话
//! 报废。目前已知两种形态：
//!
//!   1. `tool_calls[].function.name` 为空串（上游偶发产出的畸形调用，客户端
//!      回一句 "No such tool available" 之后把它留在了历史里）；
//!   2. tool 结果与 assistant 的 `tool_calls` 之间插了别的消息（Claude Desktop
//!      加载 skill 时把 tool_result 与正文放在同一条 user 消息里，入口翻译
//!      `anthropic.rs` 拆分这条消息时顺序不对就会形成这个形态）。
//!
//! ── 为什么在转发入口做一次，而不是在某一家适配器里 ───────────
//! 坏历史会流到**任意一家**上游：内置各家是 chat 透传，自定义家按协议翻译成
//! Responses / Anthropic。同一份内部 Chat 体在转发入口 sanitize 一次，所有家、
//! 所有协议、所有重试轮次看到的历史完全一致（放适配器里就会按家各修一遍，
//! 还会随新增协议漏掉）。分工上与 [`super::strip_internal_fields`] 是同一层面
//! 的两半：那是「字段形态」的通用出站卫生，这里是「历史结构」的那一半。
//!
//! ── 与 WorkBuddy 适配器的关系 ───────────────────────────────
//! 配对重排与孤儿裁剪原先只存在于 `providers::workbuddy::normalize`（移植自
//! 参考项目 workbuddy2api 的 `tool_pairing.go`）。同一套规则两处维护必然漂移，
//! 现在统一到本模块；那家的归一化只留本家形态相关的那几步。
//!
//! ── 取舍：宁可丢一轮工具上下文，也好过整条会话死亡 ──────────
//! 三步都是**结构性修复**，不改内容语义：补名、挪位、剔除无法配对的条目。
//! 与参考项目同一条底线（见其 `tool_pairing.go` 的说明）。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! 同 `mod.rs`：零 unwrap/expect/panic；纯函数，不做 IO。

use serde_json::{Map, Value};

/// 工具调用空名补的占位名。
///
/// 与 `responses.rs` / `anthropic.rs` 处理**工具声明**空名时的兜底同名 ——
/// 同一类问题（名字缺失）用同一个占位符，排查时一眼认得出是网关补的。
const UNKNOWN_TOOL_NAME: &str = "unknown";

/// 历史 sanitize 做了什么（供调用方写一行详细日志）。
///
/// 与 `workbuddy::normalize` 的 `NormalizeReport` 同一取向：只统计**值得解释的
/// 修复**。这层改动的是历史结构（少一轮工具上下文、调用名被补），一旦出问题
/// 用户要能回答「网关对我的请求动了什么」。
#[derive(Clone, Debug, Default)]
pub struct HistoryReport {
    /// 补了占位名的空工具调用数
    pub renamed_tools: usize,
    /// 是否发生了配对断裂重排（插在 assistant 与其结果之间的消息被挪到组后）
    pub repacked: bool,
    /// 剔除的孤儿 tool_call / tool 结果条目数
    pub orphans_removed: usize,
}

impl HistoryReport {
    /// 是否有值得写进详细日志的修复
    pub fn notable(&self) -> bool {
        self.renamed_tools > 0 || self.repacked || self.orphans_removed > 0
    }

    /// 一行可读的中文描述（供 `logging::verbose`）
    pub fn describe(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        if self.renamed_tools > 0 {
            parts.push(format!("空名工具调用补占位名 {} 个", self.renamed_tools));
        }
        if self.repacked {
            parts.push("tool 结果重排（配对断裂修复）".to_string());
        }
        if self.orphans_removed > 0 {
            parts.push(format!("剔除无法配对的 tool 条目 {} 个", self.orphans_removed));
        }
        parts.join("；")
    }
}

/// 对内部 Chat 体做一轮历史 sanitize（原地改写），返回修复报告。
///
/// 三步的顺序不能调换：先补名（让后续判定只看结构），再重排（把结果挪到
/// assistant 之后），最后裁剪（此时「配对齐全」的判据才与将发出去的字节一致
/// —— 先裁剪会把中间的插入消息当成孤儿误删）。
///
/// body 不是对象 / 没有 messages 数组时零改动返回（坏 body 不在这里二次
/// 错误化：上游会给出比我们更准确的解析错误）。
pub fn sanitize_history(body: &mut Value) -> HistoryReport {
    let mut report = HistoryReport::default();
    let Some(messages) = body.get_mut("messages").and_then(Value::as_array_mut) else {
        return report;
    };
    rename_empty_tool_names(messages, &mut report);
    repack_tool_results(messages, &mut report);
    cleanup_orphan_tools(messages, &mut report);
    report
}

/// 第一步：给空名工具调用补占位名。
///
/// 为什么补名而不是「删掉这条调用及其配对结果」：补名只动一个字符串 —— 配对
/// 保持完整、历史条数不变（前缀缓存的影响最小），也不会凭空造出「无 tool_calls
/// 的 assistant」这种新形态。删除则要连带删结果、再处理 assistant 变空的连带
/// 情形，改的东西比修的问题还多。
///
/// 只认规范形态（`function` 是对象）：`function` 缺失/非对象说明客户端的调用
/// 形态本身就无从判断，伪造一个只会让上游的校验指向更莫名其妙的位置 ——
/// 原样发、让上游如实报错（与 `anthropic_outbound` 对缺 id 的处理同一取向）。
fn rename_empty_tool_names(messages: &mut [Value], report: &mut HistoryReport) {
    for message in messages.iter_mut() {
        let Some(fields) = message.as_object_mut() else {
            continue;
        };
        if !is_role(fields, "assistant") {
            continue;
        }
        let Some(calls) = fields.get_mut("tool_calls").and_then(Value::as_array_mut) else {
            continue;
        };
        for call in calls.iter_mut() {
            let Some(function) = call.get_mut("function").and_then(Value::as_object_mut) else {
                continue;
            };
            let named = function
                .get("name")
                .and_then(Value::as_str)
                .map(str::trim)
                .is_some_and(|name| !name.is_empty());
            if named {
                continue;
            }
            function.insert(
                "name".to_string(),
                Value::String(UNKNOWN_TOOL_NAME.to_string()),
            );
            report.renamed_tools += 1;
        }
    }
}

/// 第二步：把插在 `assistant.tool_calls` 与其结果之间的非 tool 消息挪到整组之后，
/// 保证同一批调用的结果在 wire 上**连续**且**紧跟在 assistant 后面**。
///
/// OpenAI 兼容协议要求 tool 结果紧跟 assistant，中间插任何消息都算配对断裂
/// （上游判 11148 / "tool calls and tool results do not match"）并顶死会话；
/// Anthropic 侧同样要求 `tool_result` 落在 user 消息内容块最前。客户端不知道
/// 这条规则：Codex 的 image_resize_notice、Claude Desktop 的 skill 注入都会
/// 把一条消息插进中间。
///
/// 与参考项目 `repackToolResultBlocks` 的差异只有一处（其余逐条对齐）：**不再
/// 要求「assistant 后面第一条就得是结果」**。参考实现遇到「assistant → 插入消息
/// → 结果」直接 break（那一段交给孤儿裁剪），于是这一形态永远得不到重排 ——
/// 而它正是 Claude Desktop 混装历史的形态。这里改成向后找：本批结果收齐、
/// 撞见下一组 `assistant.tool_calls`、或撞见不属于本批的 tool 消息为止。
///
/// 只在「结果真的跟在被插入消息之后」时才重建：一个结果都找不到说明这是孤儿
/// 调用，整段不动（那交给第三步）；同批结果的原相对顺序保持不变。
///
/// 分两相（先只读扫出计划、有需要才重建消息数组）：本函数每个请求都会跑，
/// 而绝大多数历史本来合规 —— 单相实现会为每段历史深拷贝一遍全部消息（大历史
/// 在 MB 级）只为把结果原样丢掉。
fn repack_tool_results(messages: &mut Vec<Value>, report: &mut HistoryReport) {
    if messages.len() < 3 {
        return;
    }
    let plan = plan_repacks(messages.as_slice());
    if plan.is_empty() {
        return;
    }
    let mut out: Vec<Value> = Vec::with_capacity(messages.len());
    let mut plan_index = 0usize;
    let mut index = 0usize;
    while index < messages.len() {
        if let Some(step) = plan.get(plan_index).filter(|step| step.start == index) {
            out.push(messages[step.start].clone());
            for &result in &step.results {
                out.push(messages[result].clone());
            }
            for &moved in &step.moved {
                out.push(messages[moved].clone());
            }
            index = step.end;
            plan_index += 1;
            continue;
        }
        out.push(messages[index].clone());
        index += 1;
    }
    *messages = out;
    report.repacked = true;
}

/// 一次重排计划（下标寻址；只读扫描的产物）
struct Repack {
    /// 组头（带 `tool_calls` 的 assistant）的下标
    start: usize,
    /// 本批结果消息的下标（按原相对顺序）
    results: Vec<usize>,
    /// 插在组头与其结果之间的消息下标（按原相对顺序，整体后移）
    moved: Vec<usize>,
    /// 组扫描的停止处（不含）：应用计划后外层从这里继续
    end: usize,
}

/// 只读扫描出需要重排的组（规则见 [`repack_tool_results`]）。
///
/// 计划按下标升序、互不重叠：每组的扫描都止于下一个带 `tool_calls` 的 assistant
/// （即下一组的组头），所以一个下标只会落进一组。
fn plan_repacks(messages: &[Value]) -> Vec<Repack> {
    let mut plan: Vec<Repack> = Vec::new();
    let mut index = 0usize;
    while index < messages.len() {
        let want = assistant_call_ids(&messages[index]);
        if want.is_empty() {
            index += 1;
            continue;
        }
        let start = index;
        index += 1;
        let mut results: Vec<usize> = Vec::new();
        let mut moved: Vec<usize> = Vec::new();
        let mut needs = false;
        while index < messages.len() {
            let Some(fields) = messages[index].as_object() else {
                break;
            };
            let role = fields.get("role").and_then(Value::as_str).unwrap_or("");
            if role == "tool" {
                let id = fields
                    .get("tool_call_id")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                if !want.iter().any(|call| call == id) {
                    break; // 不属于本批：交还外层
                }
                results.push(index);
                if !moved.is_empty() {
                    needs = true; // 结果跟在插入物之后 = 确实需要重排
                }
                index += 1;
                if results.len() >= want.len() {
                    break; // 本批结果收齐，后面的消息与新的一轮无关
                }
                continue;
            }
            // 下一组 assistant.tool_calls 是新组头，绝不能当插入物吞掉：
            // 一旦收进 moved，它自己那批结果就永远得不到重排。
            if is_role(fields, "assistant") && !assistant_call_ids(&messages[index]).is_empty() {
                break;
            }
            // 结果尚未收齐时，任何非 tool 消息都是「插在中间」的候选；一个
            // 结果都找不到时 needs 不会置位，整段原样保留。
            moved.push(index);
            index += 1;
        }
        if needs {
            plan.push(Repack {
                start,
                results,
                moved,
                end: index,
            });
        }
    }
    plan
}

/// 第三步：剔除无法配对的 `tool_call` 与 tool 结果（双侧按同一份 keep 集对称裁剪）。
///
///   - 收集全线 `role:"tool"` 的 `tool_call_id`（结果集）与
///     `assistant.tool_calls[].id`（调用集）；
///   - `assistant.tool_calls` 按 keep 集裁剪：只留有结果配对的调用，裁空则删掉
///     整个 `tool_calls` 键；
///   - `role:"tool"` 只在对应调用被保留时才保留，孤儿结果整条删除。
///
/// **两侧必须共用同一份 keep 集**：若只裁调用侧（保留整个 tool_calls 键或整批
/// 删除），会留下「无 tool_calls 的 assistant + 孤儿 tool」这种半截配对，上游
/// 照样 400。空 id 的调用/结果配不上任何东西，按同一规则一并剔除 —— 逐字沿用
/// 参考项目与 WorkBuddy 适配器既有的判据（那里已在生产上验证）。
fn cleanup_orphan_tools(messages: &mut Vec<Value>, report: &mut HistoryReport) {
    if messages.is_empty() {
        return;
    }
    let mut call_ids: Vec<String> = Vec::new();
    let mut result_ids: Vec<String> = Vec::new();
    for message in messages.iter() {
        let Some(fields) = message.as_object() else {
            continue;
        };
        match fields.get("role").and_then(Value::as_str).unwrap_or("") {
            "tool" => {
                if let Some(id) = non_empty_str(fields.get("tool_call_id")) {
                    result_ids.push(id);
                }
            }
            "assistant" => {
                if let Some(calls) = fields.get("tool_calls").and_then(Value::as_array) {
                    for call in calls {
                        if let Some(id) = call
                            .as_object()
                            .and_then(|call| non_empty_str(call.get("id")))
                        {
                            call_ids.push(id);
                        }
                    }
                }
            }
            _ => {}
        }
    }
    if call_ids.is_empty() && result_ids.is_empty() {
        return; // 无工具流量：零改动
    }
    // keep 集 = 调用与结果双侧齐全的 id
    let keep: Vec<&String> = call_ids
        .iter()
        .filter(|id| result_ids.iter().any(|result| result == *id))
        .collect();
    // 1) 调用侧裁剪
    for message in messages.iter_mut() {
        let Some(fields) = message.as_object_mut() else {
            continue;
        };
        if !is_role(fields, "assistant") {
            continue;
        }
        let Some(calls) = fields.get("tool_calls").and_then(Value::as_array) else {
            continue;
        };
        if calls.is_empty() {
            continue;
        }
        let kept: Vec<Value> = calls
            .iter()
            .filter(|call| {
                call.as_object()
                    .and_then(|call| non_empty_str(call.get("id")))
                    .map(|id| keep.iter().any(|kept| **kept == id))
                    .unwrap_or(false)
            })
            .cloned()
            .collect();
        if kept.len() == calls.len() {
            continue; // 整批齐全：零改动
        }
        report.orphans_removed += calls.len() - kept.len();
        if kept.is_empty() {
            fields.remove("tool_calls");
        } else {
            fields.insert("tool_calls".to_string(), Value::Array(kept));
        }
    }
    // 2) 结果侧裁剪：孤儿 tool 消息整条删除
    let before = messages.len();
    messages.retain(|message| {
        let Some(fields) = message.as_object() else {
            return true;
        };
        if fields.get("role").and_then(Value::as_str) != Some("tool") {
            return true;
        }
        non_empty_str(fields.get("tool_call_id"))
            .map(|id| keep.iter().any(|kept| **kept == id))
            .unwrap_or(false)
    });
    report.orphans_removed += before - messages.len();
}

/// 一条消息是否是指定角色
fn is_role(fields: &Map<String, Value>, role: &str) -> bool {
    fields.get("role").and_then(Value::as_str) == Some(role)
}

/// 取一个非空字符串字段（`tool_calls[].id` / `tool_call_id` 的判据）
fn non_empty_str(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_string)
}

/// 取 assistant 消息里 `tool_calls[].id` 的集合（非 assistant / 无调用则空 vec）
fn assistant_call_ids(message: &Value) -> Vec<String> {
    message
        .as_object()
        .and_then(|fields| {
            if !is_role(fields, "assistant") {
                return None;
            }
            fields.get("tool_calls").and_then(Value::as_array)
        })
        .map(|calls| {
            calls
                .iter()
                .filter_map(|call| call.as_object().and_then(|call| non_empty_str(call.get("id"))))
                .collect()
        })
        .unwrap_or_default()
}
