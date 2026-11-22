//! 磁盘 topology 模块（Phase 14.5b）
//!
//! AI 知识库 Tab 的「磁盘整体架构图」数据源：
//! - `topology_load`：读 `~/.homebase/topology.json`（缺失返回硬编码默认）
//! - `topology_list_dir`：按需展开节点下一层（仅一级，避免一次性扫描整盘）
//! - `topology_open_dir`：OS 打开文件夹（explorer / open / xdg-open）
//! - `topology_parse_refs`：从节点 INDEX.md / README.md frontmatter 解析 `references:` 引用边
//!
//! ## 默认 topology
//!
//! - **本机操作指南** = `<Desktop>\本机操作指南`（用户桌面文件夹）
//! - **LLM Wiki** = `C:\HomeBase\Wiki`
//! - **项目工作区** = `C:\HomeBase`
//!
//! ## 用户自定义
//!
//! 用户可创建 `~/.homebase/topology.json`：
//! ```json
//! {
//!   "version": 1,
//!   "roots": [
//!     {
//!       "id": "manual",
//!       "label": "本机操作指南",
//!       "absolute_path": "C:\\Users\\homebase",
//!       "note": "桌面 · 本机软件 / 工具 / 流程参考",
//!       "child_labels": {
//!         "Agents": "AI 代理配置",
//!         "Projects": "项目仓库"
//!       }
//!     }
//!   ],
//!   "edges": [
//!     { "source": "manual", "target": "wiki", "label": "项目笔记索引" }
//!   ]
//! }
//! ```

use crate::util::strip_bom;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

const TOPOLOGY_VERSION: u32 = 2;

// =====================================================================
// 类型
// =====================================================================

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Topology {
    pub version: u32,
    pub roots: Vec<TopologyRoot>,
    #[serde(default)]
    pub edges: Vec<TopologyEdge>,
    /// 磁盘的「虚拟分组」（方案 A · v2 新增）
    ///
    /// key = root id（如 `"drive_e"` / `"manual"`）
    /// value = 该 root 下的虚拟分类节点列表（`影视 / 软件 / 图像 / 工作区`...）
    ///
    /// 展开逻辑：
    /// - drive 配了 `drive_groups[drive.id]` → 展开时显示虚拟组节点（不再直接列物理目录）
    /// - 再点虚拟组 → 显示组内 `paths` 的物理子目录
    /// - drive 未配 → 走原先的 `topology_list_dir`
    #[serde(default)]
    pub drive_groups: BTreeMap<String, Vec<VirtualGroup>>,
    /// 数据来源：默认 / 用户配置文件
    pub source: TopologySource,
    /// 配置文件绝对路径（前端展示「编辑配置」时用）
    pub config_path: String,
}

/// 一个虚拟分组节点（方案 A v2）
///
/// 例：`{ id: "e-media", label: "影视", icon: "film", paths: ["E:\\电影", "E:\\电视剧"] }`
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct VirtualGroup {
    /// 全局唯一（跨所有 drive 也要唯一，方便前端 key）
    pub id: String,
    /// 中文显示名：影视 / 软件 / 图像 / 文档…
    pub label: String,
    /// 可选的 lucide 图标名（`film` / `package` / `image` / `folder` / `code` / `gamepad-2`…）
    #[serde(default)]
    pub icon: Option<String>,
    /// 可选的副标说明
    #[serde(default)]
    pub note: Option<String>,
    /// 该虚拟组包含的物理目录绝对路径列表
    ///
    /// 前端把每个 path 渲染成一个 ChildNode
    pub paths: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TopologySource {
    Default,
    UserConfig,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct TopologyRoot {
    pub id: String,
    pub label: String,
    pub absolute_path: String,
    #[serde(default)]
    pub note: Option<String>,
    /// subdir name → 中文 label override（用户在配置文件里写）
    #[serde(default)]
    pub child_labels: BTreeMap<String, String>,
    /// 运行时填充
    #[serde(default = "default_true")]
    pub exists: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct TopologyEdge {
    pub source: String,
    pub target: String,
    #[serde(default)]
    pub label: Option<String>,
}

/// 单层 list_dir 返回项
#[derive(Debug, Serialize, Deserialize)]
pub struct DirEntry {
    pub name: String,
    pub absolute_path: String,
    pub is_dir: bool,
}

/// 引用边（来自 frontmatter `references:`）
#[derive(Debug, Serialize, Deserialize)]
pub struct RefEdge {
    /// 解析自哪个文件
    pub source_path: String,
    /// 目标若是 root.id（无 / \ : 字符）则填这里
    pub target_id: Option<String>,
    /// 目标若像路径则填这里
    pub target_path: Option<String>,
    pub label: Option<String>,
}

// =====================================================================
// 路径辅助
// =====================================================================

fn home_dir() -> Option<PathBuf> {
    if let Some(path) = crate::legacy_isolation::test_path("") { return Some(path); }
    std::env::var("USERPROFILE").ok().map(PathBuf::from)
}

fn topology_config_path() -> Option<PathBuf> {
    home_dir().map(|h| h.join(".homebase").join("topology.json"))
}

fn desktop_dir() -> Option<PathBuf> {
    home_dir().map(|h| h.join("Desktop"))
}

// =====================================================================
// 默认 topology
// =====================================================================

fn default_topology() -> Topology {
    let mut roots = Vec::new();
    let mut edges = Vec::new();

    // 1. 枢纽节点：桌面 · 本机操作指南（AI 阅读入口）
    let mut manual_exists = false;
    if let Some(desk) = desktop_dir() {
        let manual_path = desk.join("本机操作指南");
        manual_exists = crate::legacy_isolation::ensure_test_path(&manual_path).is_ok() && manual_path.exists();
        roots.push(TopologyRoot {
            id: "manual".into(),
            label: "本机操作指南".into(),
            absolute_path: manual_path.to_string_lossy().into_owned(),
            note: Some("桌面 · AI 阅读入口 / 本机软件 / 流程参考".into()),
            child_labels: manual_child_labels(),
            exists: manual_exists,
        });
    }

    // Test profiles expose only their own workspace, never physical drives.
    if let Some(home) = crate::legacy_isolation::test_path("") {
        roots.push(TopologyRoot {
            id: "test-workspace".into(), label: "隔离工作区".into(),
            absolute_path: home.to_string_lossy().into_owned(), note: Some("仅用于合成资料验收".into()),
            child_labels: BTreeMap::new(), exists: home.is_dir(),
        });
        return Topology { version: TOPOLOGY_VERSION, roots, edges, drive_groups: BTreeMap::new(),
            source: TopologySource::Default, config_path: topology_config_path().map(|p|p.to_string_lossy().into_owned()).unwrap_or_default() };
    }
    // 2. 物理盘符：扫 C/D/E/F/G，存在的加入
    for (letter, label, note) in DRIVE_LABELS {
        let drive_path = PathBuf::from(format!("{letter}:\\"));
        if !drive_path.exists() {
            continue;
        }
        let id = format!("drive_{}", letter.to_ascii_lowercase());
        roots.push(TopologyRoot {
            id: id.clone(),
            label: label.to_string(),
            absolute_path: drive_path.to_string_lossy().into_owned(),
            note: Some(note.to_string()),
            child_labels: drive_child_labels(*letter),
            exists: true,
        });
        // 默认 edge：manual → drive（AI 从枢纽辐射到各盘）
        if manual_exists {
            edges.push(TopologyEdge {
                source: "manual".into(),
                target: id,
                label: Some(edge_label_for(*letter)),
            });
        }
    }

    Topology {
        version: TOPOLOGY_VERSION,
        roots,
        edges,
        drive_groups: BTreeMap::new(), // 方案 A：默认不填，由用户在 topology.json 里自行配置
        source: TopologySource::Default,
        config_path: topology_config_path()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default(),
    }
}

/// 默认扫描的盘符列表（letter, label, note）
///
/// 存在即加入。用户 D 盘标注根据常见本机布局。实际不存在的不会进 roots。
const DRIVE_LABELS: &[(char, &str, &str)] = &[
    ('C', "C 盘 · 系统", "Windows 系统盘 · 用户目录 / 程序"),
    ('D', "D 盘 · 软件", "第二物理盘 · 常驻软件 / 工具"),
    ('E', "E 盘 · 工作区", "项目仓库 / LLM Wiki / AI Hub"),
    ('F', "F 盘 · 数据", "数据 / 备份 / 归档"),
    ('G', "G 盘 · 其他", "扩展盘"),
];

/// manual → drive 边的 label（简短，显示在箭头上）
fn edge_label_for(letter: char) -> String {
    match letter.to_ascii_uppercase() {
        'C' => "系统 & 用户",
        'D' => "软件",
        'E' => "工作区",
        'F' => "数据",
        _ => "交叉索引",
    }
    .into()
}

/// 本机操作指南常见子目录的中文映射
///
/// 用户可在 topology.json `child_labels` 字段覆盖 / 扩展
fn manual_child_labels() -> BTreeMap<String, String> {
    [
        ("Agents", "AI 代理配置"),
        ("Projects", "项目索引"),
        ("Sessions", "会话存档"),
        ("Systems", "系统配置"),
        ("Tools", "工具链"),
        ("Toys", "实验 / 玩具"),
        ("index.md", "总目录"),
        ("INDEX.md", "总目录"),
        ("README.md", "仓库说明"),
        ("AGENTS.md", "Agent 规则"),
    ]
    .iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect()
}

/// 盘符的常见子目录中文映射（按盘符定制）
fn drive_child_labels(letter: char) -> BTreeMap<String, String> {
    let mut m = BTreeMap::new();
    match letter.to_ascii_uppercase() {
        'C' => {
            m.insert("Users".into(), "用户目录".into());
            m.insert("Program Files".into(), "程序（64 位）".into());
            m.insert("Program Files (x86)".into(), "程序（32 位）".into());
            m.insert("Windows".into(), "系统".into());
            m.insert("ProgramData".into(), "程序数据".into());
        }
        'D' => {
            m.insert("Typora".into(), "Typora 编辑器".into());
            m.insert("Program Files".into(), "程序（64 位）".into());
        }
        'E' => {
            m.insert("Wiki".into(), "LLM Wiki（Karpathy 模式）".into());
            m.insert("Works".into(), "项目工作区".into());
            m.insert("Personal".into(), "个人文档 / 日记".into());
            m.insert("Backup".into(), "备份".into());
            m.insert("zBackup".into(), "历史归档".into());
        }
        'F' => {
            m.insert("Backup".into(), "备份".into());
            m.insert("Secrets".into(), "SOPS 密钥仓（只读）".into());
        }
        _ => {}
    }
    m
}

// =====================================================================
// Tauri commands
// =====================================================================

/// 加载 topology（用户配置 > 默认硬编码）
#[tauri::command]
pub async fn topology_load() -> Result<Topology, String> {
    tracing::debug!("topology_load 调用");
    let cfg = topology_config_path();
    if let Some(path) = &cfg { crate::legacy_isolation::ensure_test_path(path)?; }
    let config_content = match cfg.as_ref() {
        Some(path) => match fs::read_to_string(path) {
            Ok(content) => Some(content),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(_) => return Err("QUERY_FAILED: 拓扑配置不可读取，未回退默认值".into()),
        },
        None => return Err("QUERY_FAILED: 无法定位拓扑配置目录".into()),
    };
    let mut topology = match config_content {
        Some(content) => match serde_json::from_str::<Topology>(strip_bom(&content)) {
            Ok(mut t) => {
                t.source = TopologySource::UserConfig;
                if let Some(p) = cfg.as_ref() {
                    t.config_path = p.to_string_lossy().into_owned();
                }
                t
            }
            Err(_) => return Err("QUERY_INVALID: 拓扑配置格式无效，未回退默认值".into()),
        },
        None => default_topology(),
    };
    // 实时校验 exists
    for r in &mut topology.roots {
        crate::legacy_isolation::ensure_test_path(Path::new(&r.absolute_path))?;
        r.exists = Path::new(&r.absolute_path).is_dir();
    }
    for groups in topology.drive_groups.values() {
        for group in groups { for path in &group.paths { crate::legacy_isolation::ensure_test_path(Path::new(path))?; } }
    }
    Ok(topology)
}

/// 列出某绝对路径下的子条目（仅一级）
///
/// 返回：
/// - 所有子目录（隐藏 / 系统目录除外）
/// - 顶级关键 markdown（INDEX.md / README.md / AGENTS.md / log.md / index.md）
///
/// 子目录在前，按字母序
#[tauri::command]
pub async fn topology_list_dir(absolute_path: String) -> Result<Vec<DirEntry>, String> {
    tracing::debug!("topology_list_dir 调用: {absolute_path}");
    let p = Path::new(&absolute_path);
    crate::legacy_isolation::ensure_test_path(p)?;
    if !p.is_dir() {
        return Err(format!("不是目录或不存在: {absolute_path}"));
    }
    let read = fs::read_dir(p).map_err(|e| format!("read_dir 失败: {e}"))?;
    let mut out = Vec::new();
    for entry in read {
        let entry = entry.map_err(|_| "QUERY_FAILED: 拓扑目录条目读取失败")?;
        let name = entry.file_name().to_string_lossy().into_owned();
        // 跳过隐藏 / 系统目录
        if name.starts_with('.')
            || matches!(
                name.as_str(),
                "node_modules"
                    | "target"
                    | "$RECYCLE.BIN"
                    | "System Volume Information"
                    | "__pycache__"
            )
        {
            continue;
        }
        let abs = entry.path();
        crate::legacy_isolation::ensure_test_path(&abs)?;
        let is_dir = entry.file_type().map_err(|_| "QUERY_FAILED: 拓扑目录条目类型不可读取")?.is_dir();
        // 文件只保留几个顶级关键 markdown
        if !is_dir {
            let lower = name.to_lowercase();
            if !matches!(
                lower.as_str(),
                "index.md" | "readme.md" | "agents.md" | "log.md"
            ) {
                continue;
            }
        }
        out.push(DirEntry {
            name,
            absolute_path: abs.to_string_lossy().into_owned(),
            is_dir,
        });
    }
    out.sort_by(|a, b| {
        if a.is_dir != b.is_dir {
            return if a.is_dir {
                std::cmp::Ordering::Less
            } else {
                std::cmp::Ordering::Greater
            };
        }
        a.name.to_lowercase().cmp(&b.name.to_lowercase())
    });
    Ok(out)
}

/// 用 OS 文件管理器打开文件夹
#[tauri::command]
pub async fn topology_open_dir(absolute_path: String) -> Result<(), String> {
    crate::legacy_isolation::ensure_test_path(Path::new(&absolute_path))?;
    tracing::debug!("topology_open_dir 调用: {absolute_path}");
    let p = Path::new(&absolute_path);
    if !p.exists() {
        return Err(format!("路径不存在: {absolute_path}"));
    }
    #[cfg(target_os = "windows")]
    {
        let mut cmd = Command::new("explorer.exe");
        cmd.arg(&absolute_path);
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
        cmd.spawn().map_err(|e| format!("explorer 启动失败: {e}"))?;
    }
    #[cfg(target_os = "macos")]
    {
        Command::new("open")
            .arg(&absolute_path)
            .spawn()
            .map_err(|e| format!("open 启动失败: {e}"))?;
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        Command::new("xdg-open")
            .arg(&absolute_path)
            .spawn()
            .map_err(|e| format!("xdg-open 启动失败: {e}"))?;
    }
    Ok(())
}

/// 解析某目录下的 INDEX.md / README.md frontmatter `references:`
///
/// 找 INDEX.md / index.md / README.md / AGENTS.md 任意一个，提取 references 数组
#[tauri::command]
pub async fn topology_parse_refs(absolute_path: String) -> Result<Vec<RefEdge>, String> {
    crate::legacy_isolation::ensure_test_path(Path::new(&absolute_path))?;
    tracing::debug!("topology_parse_refs 调用: {absolute_path}");
    let dir = Path::new(&absolute_path);
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut edges = Vec::new();
    for cand in &["INDEX.md", "index.md", "README.md", "AGENTS.md"] {
        let p = dir.join(cand);
        crate::legacy_isolation::ensure_test_path(&p)?;
        if !p.exists() {
            continue;
        }
        if let Ok(content) = fs::read_to_string(&p) {
            extract_refs(&p.to_string_lossy(), &content, &mut edges);
        }
    }
    Ok(edges)
}

// =====================================================================
// frontmatter 解析
// =====================================================================

/// 从 markdown 内容里抽取 frontmatter 的 `references:` 数组
///
/// 支持三种条目格式：
/// ```yaml
/// references:
///   - wiki                 # 简写 = root.id
///   - C:\Path\To\Other     # 绝对路径（按路径配 edge）
///   - target: wiki         # 完整对象
///     label: 项目笔记索引
/// ```
fn extract_refs(source_path: &str, content: &str, edges: &mut Vec<RefEdge>) {
    let stripped = content.strip_prefix('\u{feff}').unwrap_or(content);
    let trimmed = stripped.trim_start();
    if !trimmed.starts_with("---") {
        return;
    }
    let body = &trimmed[3..];
    let end = match body.find("\n---").or_else(|| body.find("\r\n---")) {
        Some(i) => i,
        None => return,
    };
    let yaml_str = &body[..end];
    let map: serde_yaml::Value = match serde_yaml::from_str(yaml_str) {
        Ok(v) => v,
        Err(_) => return,
    };
    let arr = match map.get("references").and_then(|v| v.as_sequence()) {
        Some(a) => a,
        None => return,
    };
    for item in arr {
        if let Some(s) = item.as_str() {
            let is_path = s.contains('/') || s.contains('\\') || s.contains(':');
            edges.push(RefEdge {
                source_path: source_path.into(),
                target_id: if !is_path { Some(s.into()) } else { None },
                target_path: if is_path { Some(s.into()) } else { None },
                label: None,
            });
        } else if let Some(m) = item.as_mapping() {
            let target = m
                .get(serde_yaml::Value::String("target".into()))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let label = m
                .get(serde_yaml::Value::String("label".into()))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            if let Some(t) = target {
                let is_path = t.contains('/') || t.contains('\\') || t.contains(':');
                edges.push(RefEdge {
                    source_path: source_path.into(),
                    target_id: if !is_path { Some(t.clone()) } else { None },
                    target_path: if is_path { Some(t) } else { None },
                    label,
                });
            }
        }
    }
}

// =====================================================================
// 测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "Explicit isolated pure path probe; never scans real drives"]
    fn legacy_isolation_path_probe() {
        let home = PathBuf::from(std::env::var_os("HOMEBASE_TEST_HOME").unwrap());
        assert_eq!(home_dir().unwrap(), home);
    }

    #[test]
    fn default_topology_uses_manual_as_hub_with_drive_nodes() {
        let t = default_topology();
        assert_eq!(t.version, TOPOLOGY_VERSION);
        assert_eq!(t.source, TopologySource::Default);
        if let Some(home) = crate::legacy_isolation::test_path("") {
            let workspace = t.roots.iter().find(|root| root.id == "test-workspace").unwrap();
            assert_eq!(Path::new(&workspace.absolute_path), home.as_path());
            assert!(t.roots.iter().all(|root| matches!(root.id.as_str(), "manual" | "test-workspace")));
            assert!(t.edges.is_empty());
            return;
        }
        // Windows 系统盘
        assert!(
            t.roots.iter().any(|r| r.id == "drive_c"),
            "默认 topology 必须包含 C 盘节点"
        );
        // E 盘通常有（开发机配置）— 非强制，但若有则验证 child_labels 预填
        if let Some(drive_e) = t.roots.iter().find(|r| r.id == "drive_e") {
            assert!(
                drive_e.child_labels.contains_key("Wiki"),
                "E 盘应预填 Wiki 子节点中文 label"
            );
        }
    }

    #[test]
    fn default_topology_has_manual_to_drive_edges_when_manual_exists() {
        let t = default_topology();
        if crate::legacy_isolation::test_path("").is_some() {
            assert!(t.edges.is_empty());
            return;
        }
        // 如果 manual（本机操作指南）存在，则必然有至少一条 manual→drive_X edge
        let has_manual = t.roots.iter().any(|r| r.id == "manual" && r.exists);
        if has_manual {
            let manual_edges = t
                .edges
                .iter()
                .filter(|e| e.source == "manual" && e.target.starts_with("drive_"))
                .count();
            assert!(
                manual_edges >= 1,
                "manual 存在时应有至少 1 条 manual→drive 边"
            );
        }
    }

    #[test]
    fn drive_child_labels_e_includes_wiki_and_works() {
        let labels = drive_child_labels('E');
        assert_eq!(labels.get("Wiki").map(|s| s.as_str()), Some("LLM Wiki（Karpathy 模式）"));
        assert_eq!(labels.get("Works").map(|s| s.as_str()), Some("项目工作区"));
    }

    #[test]
    fn drive_child_labels_unknown_letter_returns_empty() {
        assert!(drive_child_labels('Z').is_empty());
    }

    #[test]
    fn extract_refs_handles_simple_id() {
        let md = "---\nreferences:\n  - wiki\n  - works\n---\nbody";
        let mut edges = Vec::new();
        extract_refs("test.md", md, &mut edges);
        assert_eq!(edges.len(), 2);
        assert_eq!(edges[0].target_id.as_deref(), Some("wiki"));
        assert!(edges[0].target_path.is_none());
        assert_eq!(edges[1].target_id.as_deref(), Some("works"));
    }

    #[test]
    fn extract_refs_handles_absolute_path() {
        let md = "---\nreferences:\n  - 'C:\\HomeBase\\Wiki'\n---\n";
        let mut edges = Vec::new();
        extract_refs("test.md", md, &mut edges);
        assert_eq!(edges.len(), 1);
        assert!(edges[0].target_id.is_none());
        assert_eq!(edges[0].target_path.as_deref(), Some(r"C:\HomeBase\Wiki"));
    }

    #[test]
    fn extract_refs_handles_full_object_with_label() {
        let md = "---\nreferences:\n  - target: wiki\n    label: 项目笔记索引\n---\n";
        let mut edges = Vec::new();
        extract_refs("test.md", md, &mut edges);
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].target_id.as_deref(), Some("wiki"));
        assert_eq!(edges[0].label.as_deref(), Some("项目笔记索引"));
    }

    #[test]
    fn extract_refs_ignores_files_without_frontmatter() {
        let md = "# heading\nbody";
        let mut edges = Vec::new();
        extract_refs("test.md", md, &mut edges);
        assert!(edges.is_empty());
    }

    #[test]
    fn extract_refs_ignores_files_with_frontmatter_but_no_references() {
        let md = "---\ntitle: Foo\n---\nbody";
        let mut edges = Vec::new();
        extract_refs("test.md", md, &mut edges);
        assert!(edges.is_empty());
    }
}
