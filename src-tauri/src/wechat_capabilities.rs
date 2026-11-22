use super::{clip, Action};
use crate::capability_discovery::DiscoveryRequest;
use serde_json::{json, Value};

pub(super) const COMMAND: &str = "capability_index_query";
const PAGE_SIZE: usize = 6;

pub(super) fn parse(text: &str) -> Option<Result<Action, String>> {
    let text = text.trim();
    let (prefix, rest) = ["/能力", "能力", "/找能力", "帮我找能力", "找能力"]
        .into_iter()
        .find_map(|prefix| {
            let rest = text.strip_prefix(prefix)?;
            (rest.is_empty()
                || rest.starts_with([':', '：'])
                || rest.starts_with(char::is_whitespace))
            .then_some((prefix, rest))
        })?;
    let term = rest.trim_start().trim_start_matches([':', '：']).trim();
    if term.is_empty() && prefix.contains("找") {
        return Some(Err(
            "你想找哪种能力？试试「找能力 转写」或「能力 MCP」。".into()
        ));
    }
    let kind = match term.to_ascii_lowercase().as_str() {
        "mcp" => Some("mcp"),
        "skill" | "技能" => Some("skill"),
        "api" | "服务" => Some("service"),
        "模型" => Some("model"),
        "仓库" | "github" => Some("repository"),
        "本机" => Some("local"),
        _ => None,
    };
    let request =
        json!({"kind":kind,"query":if kind.is_some() { "" } else { term },"limit":PAGE_SIZE});
    let valid = serde_json::from_value::<DiscoveryRequest>(request.clone())
        .is_ok_and(|request| request.filters().is_ok());
    Some(if valid {
        Ok(Action::Call(COMMAND.into(), json!({"request":request})))
    } else {
        Err("关键词太长或包含换行等控制字符，请用一个简短关键词。".into())
    })
}

fn entry_kind(kind: &str) -> &'static str {
    match kind {
        "mcp" => "MCP",
        "skill" => "Skill",
        "model" => "模型",
        "service" => "API 服务",
        "repository" => "仓库",
        "local" => "本机",
        _ => "能力",
    }
}

fn entry_state(state: &str) -> &'static str {
    match state {
        "authorization_required" => "需要授权",
        "authorized_not_probed" => "已授权，连接待核对",
        "secure_reference_required" => "需要更换为凭据引用",
        "blocked_secret_export" => "助理不可使用",
        "disabled" => "本机工具已停用",
        "not_configured" => "配置待补齐",
        "unknown" => "可用性待核对",
        "instructions_available" => "可读取工作流",
        "incomplete" => "工作流说明不完整",
        "configured" => "已配置，接口待核对",
        "registered" => "已登记，调用前核对",
        "blocked" => "当前受阻",
        "expired" => "已到期",
        _ => "调用前核对",
    }
}

pub(super) fn summary(page: &Value) -> String {
    let Some(items) = page["entries"].as_array() else {
        return "暂时无法核对能力库，请稍后再查。".into();
    };
    let Some(total) = page["total"].as_u64() else {
        return "暂时无法核对能力库，请稍后再查。".into();
    };
    let mut lines = Vec::new();
    for item in items.iter().take(PAGE_SIZE) {
        let kind = entry_kind(item["kind"].as_str().unwrap_or_default());
        let state = entry_state(item["state"].as_str().unwrap_or_default());
        let name = item["name"].as_str().unwrap_or_default();
        let name = clip(&name.split_whitespace().collect::<Vec<_>>().join(" "), 72);
        if !name.is_empty() {
            lines.push(format!("· {name}（{kind}，{state}）"));
        }
    }
    if let Some(groups) = page["groups"]
        .as_array()
        .filter(|_| page["filters"]["query"] == "" && page["filters"]["kind"].is_null())
    {
        let functions = groups
            .iter()
            .filter_map(|group| {
                Some(match group["function"].as_str()? {
                    "text" => "文本",
                    "image" => "图像",
                    "video" => "视频",
                    "speech" | "audio" => "语音",
                    "document" => "文档",
                    "tools" => "工具",
                    "workflow" => "工作流",
                    "development" => "开发",
                    _ => return None,
                })
            })
            .collect::<Vec<_>>();
        if !functions.is_empty() {
            lines.insert(0, format!("功能有{}。", functions.join("、")));
        }
    }
    if total == 0 {
        lines.push("暂时没有匹配项。".into());
    } else if page["next_offset"].is_u64() {
        lines.push(format!("共找到 {total} 项，先显示前 {PAGE_SIZE} 项。"));
    }
    if page["source_complete"] != true {
        lines.push("部分来源暂未读全。".into());
    }
    lines.push(if page["filters"]["kind"].is_null() {
        "可发「能力 MCP」「能力 Skill」「能力 API」，或用「找能力 关键词」缩小范围。".into()
    } else {
        "可用「找能力 关键词」找具体实现，也可以告诉助理你想用哪项做什么。".into()
    });
    lines.join("\n")
}
