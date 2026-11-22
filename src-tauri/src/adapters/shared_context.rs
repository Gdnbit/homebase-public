//! 共享上下文池（S1 · 2026-05-06）
//!
//! 解决"5 个 agent 各自硬编码 vault / 网站 / 调性"的重复问题。
//! 单一数据源：`~/.homebase/shared/context.toml`
//!
//! 调用入口：
//! - `render_header()` 给所有 agent prompt 注入用 · 含当前时间 + 精准路径 + 调性
//! - `load()` 给 UI / 读取场景用 · 返结构化 `SharedContext`
//! - `save()` 给 UI 写入场景用
//!
//! 设计原则：
//! - 文件不存在 → 写入默认值（保留 P0 阶段硬编码的内容 · 零迁移成本）
//! - 每次 `render_header()` 都重新读盘（小文件 · 几 ms · USER 改完 toml 立即生效不用重启）
//! - 解析失败 → 退回默认值 + warn log（不让坏 toml 阻断 agent 运行）

use chrono::Local;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// 共享上下文 schema（写入 ~/.homebase/shared/context.toml）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SharedContext {
    #[serde(default)]
    pub style: StyleSection,
    #[serde(default)]
    pub locations: Vec<LocationEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StyleSection {
    /// 调性短代号（如 "SCS"）
    #[serde(default = "default_style_name")]
    pub name: String,
    /// 一句话描述
    #[serde(default = "default_style_desc")]
    pub description: String,
    /// 行为指令 · agent 必读（中文交流 / 不用 emoji 等）
    #[serde(default = "default_style_directives")]
    pub directives: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocationEntry {
    /// 短 id · 用于 UI 引用 / future 引用（如 "vault" / "gdnbit_web"）
    pub id: String,
    /// 显示名（"Vault" / "GDNbit Web"）
    pub name: String,
    /// 绝对路径
    pub path: String,
    /// 一句话说"干什么用 + 为什么存在"（替代之前空泛的"个人知识库"）
    pub purpose: String,
    /// 子路径列表 · 给 agent 知道哪个子目录干啥（不让 agent 再去 Read README）
    #[serde(default)]
    pub subpaths: Vec<SubpathEntry>,
    /// 给 agent 的额外提示（如"改之前先 Read"）· 可选
    #[serde(default)]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubpathEntry {
    /// 相对路径（如 "_raw/" / "content/blog/"）
    pub path: String,
    /// 这个子目录是干什么用的
    pub desc: String,
}

impl Default for SharedContext {
    fn default() -> Self {
        Self {
            style: StyleSection::default(),
            locations: default_locations(),
        }
    }
}

impl Default for StyleSection {
    fn default() -> Self {
        Self {
            name: default_style_name(),
            description: default_style_desc(),
            directives: default_style_directives(),
        }
    }
}

fn default_style_name() -> String {
    "SCS".into()
}

fn default_style_desc() -> String {
    "Safe / Clean / Soft · Off-White + minimal.gallery 字重对撞排版".into()
}

fn default_style_directives() -> Vec<String> {
    vec![
        "中文交流".into(),
        "不用营销腔".into(),
        "不用 emoji（除非主题需要）".into(),
    ]
}

fn default_locations() -> Vec<LocationEntry> {
    vec![
        LocationEntry {
            id: "vault".into(),
            name: "Vault".into(),
            path: r"C:\HomeBase\Wiki".into(),
            purpose: "Obsidian 笔记库 · 按 Karpathy LLM Wiki 模式沉淀工程与研究日志（raw 原始 → wiki/concepts 精炼 → creative 输出）".into(),
            subpaths: vec![
                SubpathEntry {
                    path: "_raw/".into(),
                    desc: "原始草稿 / Studio 落盘处（`studio-<ts>-<slug>.md`）".into(),
                },
                SubpathEntry {
                    path: "_raw/attachments/".into(),
                    desc: "AI 生成的图片（生图工具默认输出目录）".into(),
                },
                SubpathEntry {
                    path: "_schema/STYLE_GUIDE.md".into(),
                    desc: "风格规范（改文章前先 Read）".into(),
                },
                SubpathEntry {
                    path: "_schema/AGENTS.md".into(),
                    desc: "Agent 行为约束（消化任务前 Read）".into(),
                },
                SubpathEntry {
                    path: "_schema/NAMING_CONVENTION.md".into(),
                    desc: "文件命名规则".into(),
                },
                SubpathEntry {
                    path: "wiki/".into(),
                    desc: "已精炼的 concepts / entities / operators 页".into(),
                },
            ],
            note: None,
        },
        LocationEntry {
            id: "gdnbit_web".into(),
            name: "GDNbit Web".into(),
            path: r"C:\HomeBase\Site".into(),
            purpose: "配置的个人网站· Next.js + MDX + Tailwind".into(),
            subpaths: vec![
                SubpathEntry {
                    path: "content/blog/".into(),
                    desc: "已发布博文 · MDX · frontmatter: title/date/tags/description/slug".into(),
                },
                SubpathEntry {
                    path: "content/projects/".into(),
                    desc: "项目介绍页 · 同格式".into(),
                },
                SubpathEntry {
                    path: "README.md".into(),
                    desc: "架构与当前阶段".into(),
                },
            ],
            note: Some("改已发布内容前 · 先 Read 确认作者原意".into()),
        },
    ]
}

// =====================================================================
// 文件读写
// =====================================================================

/// `~/.homebase/shared/context.toml`
pub fn config_path() -> PathBuf {
    super::homebase_home_dir()
        .join(".homebase")
        .join("shared")
        .join("context.toml")
}

/// 读取共享上下文
///
/// 行为：
/// - 文件不存在 → 写入默认值 + 返回默认值
/// - 文件存在但 parse 失败 → 退回默认值 + warn（不阻断 agent 运行）
pub fn load() -> SharedContext {
    let path = config_path();
    if !path.exists() {
        let default = SharedContext::default();
        if let Err(e) = save(&default) {
            tracing::warn!("shared_context::save default 失败: {}", e);
        }
        return default;
    }
    match std::fs::read_to_string(&path) {
        Ok(s) => match toml::from_str::<SharedContext>(&s) {
            Ok(ctx) => ctx,
            Err(e) => {
                tracing::warn!("shared_context::parse 失败 · 退回默认值: {}", e);
                SharedContext::default()
            }
        },
        Err(e) => {
            tracing::warn!("shared_context::read 失败 · 退回默认值: {}", e);
            SharedContext::default()
        }
    }
}

/// 写入共享上下文（UI 编辑时调用）
pub fn save(ctx: &SharedContext) -> Result<(), String> {
    let path = config_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("create_dir {dir:?}: {e}"))?;
    }
    let s = toml::to_string_pretty(ctx).map_err(|e| format!("toml::to_string: {e}"))?;
    std::fs::write(&path, s).map_err(|e| format!("write {path:?}: {e}"))?;
    Ok(())
}

// =====================================================================
// 渲染：给所有 agent prompt 用的统一头部
// =====================================================================

/// 渲染统一的 HomeBase 注入上下文头（每次 spawn agent 时调用）
///
/// 输出格式：
/// ```text
/// # HomeBase 注入上下文（每次对话刷新 · 不用再去 Read 确认）
///
/// - **当前时间**：2026-05-06 22:05 (UTC+08:00)
/// - **Vault**：`C:\HomeBase\Wiki` — Obsidian 笔记库 ...
///   - `_raw/` — 原始草稿 ...
///   - ...
/// - **GDNbit Web**：`C:\HomeBase\Site` — ...
///   - `content/blog/` — ...
/// - **USER 调性（SCS）**：Safe / Clean / Soft · ... · 中文交流 / 不用营销腔 / ...
///
/// ```
///
/// 末尾保留一个空行 · 便于直接拼接到下一段 prompt
pub fn render_header() -> String {
    render_from(&load())
}

/// 内部纯函数 · 给定 ctx 渲染 header（测试用 · 不读盘）
fn render_from(ctx: &SharedContext) -> String {
    let now = Local::now().format("%Y-%m-%d %H:%M (UTC%:z)").to_string();

    let mut out = String::new();
    out.push_str("# HomeBase 注入上下文（每次对话刷新 · 不用再去 Read 确认）\n\n");
    out.push_str(&format!("- **当前时间**：{now}\n"));

    for loc in &ctx.locations {
        out.push_str(&format!(
            "- **{name}**：`{path}` — {purpose}\n",
            name = loc.name,
            path = loc.path,
            purpose = loc.purpose
        ));
        for sub in &loc.subpaths {
            out.push_str(&format!(
                "  - `{path}` — {desc}\n",
                path = sub.path,
                desc = sub.desc
            ));
        }
        if let Some(note) = &loc.note {
            out.push_str(&format!("  - {note}\n"));
        }
    }

    let directives = ctx.style.directives.join(" / ");
    out.push_str(&format!(
        "- **USER 调性（{name}）**：{desc} · 中文交流 · {directives}\n\n",
        name = ctx.style.name,
        desc = ctx.style.description,
        directives = if directives.is_empty() {
            String::new()
        } else {
            directives
        }
    ));

    out
}

// =====================================================================
// Tests
// =====================================================================

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn default_renders_essential_fields() {
        let ctx = SharedContext::default();
        // 关键字段非空
        assert!(!ctx.style.name.is_empty());
        assert!(!ctx.style.description.is_empty());
        assert!(!ctx.style.directives.is_empty());
        assert!(ctx.locations.len() >= 2, "至少 vault + gdnbit_web 两条");

        // 路径关键词
        let vault = ctx.locations.iter().find(|l| l.id == "vault").unwrap();
        assert_eq!(vault.path, r"C:\HomeBase\Wiki");
        assert!(!vault.subpaths.is_empty());

        let web = ctx.locations.iter().find(|l| l.id == "gdnbit_web").unwrap();
        assert!(web.path.contains("Gdnbit_Web"));
    }

    #[test]
    fn render_header_contains_essentials() {
        // 用纯函数 render_from + Default · 不读盘 · 不污染真实文件
        let header = render_from(&SharedContext::default());
        assert!(header.contains("HomeBase 注入上下文"));
        assert!(header.contains("当前时间"));
        assert!(header.contains("Vault"));
        assert!(header.contains("GDNbit Web"));
        assert!(header.contains("SCS"));
    }

    /// 串行锁 · 避免多个测试并行竞争 `HOMEBASE_TEST_HOME` env 变量
    /// （shared_context / mcp_reconcile 测试都用同一个 env · 必须串行）
    pub(crate) static TEST_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn save_then_load_roundtrip() {
        let _guard = TEST_ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        // 用 HOMEBASE_TEST_HOME env 隔离 · 不污染真实 ~/.homebase
        let tmp = tempfile::tempdir().expect("tempdir");
        let prev = std::env::var("HOMEBASE_TEST_HOME").ok();
        std::env::set_var("HOMEBASE_TEST_HOME", tmp.path());

        let mut ctx = SharedContext::default();
        ctx.style.name = "TestTone".into();
        save(&ctx).expect("save ok");
        let loaded = load();
        assert_eq!(loaded.style.name, "TestTone");

        match prev {
            Some(v) => std::env::set_var("HOMEBASE_TEST_HOME", v),
            None => std::env::remove_var("HOMEBASE_TEST_HOME"),
        }
    }
}
