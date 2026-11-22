//! LLM Wiki 模块（Phase 14.5）
//!
//! Karpathy LLM Wiki 模式：递归扫描 `C:\HomeBase\Wiki\` 目录树，把所有 markdown 暴露给前端。
//!
//! 区别于 `docs.rs`（读 INDEX.md 单文件索引）：
//! - `wiki_list`: 递归遍历 C:\HomeBase\Wiki，发现所有 .md/.markdown/.txt
//! - `wiki_read`: 读单篇（路径白名单：必须在 C:\HomeBase\Wiki 下）
//! - `docs_open_in_typora`: 用 Typora.exe 打开任意 .md（Wiki / INDEX 共用）
//!
//! 设计：
//! - **不引入 walkdir** — std::fs::read_dir 递归足够
//! - **frontmatter 解析**：用已有 serde_yaml 解析 `--- ... ---` 头，提取 title / doc_id
//! - **路径白名单**：wiki_read / docs_open_in_typora 都校验路径必须在 C:\HomeBase\Wiki 或 INDEX.md 登记的文档列表内

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;


/// Typora 候选路径（按优先级）
const TYPORA_CANDIDATES: &[&str] = &[
    r"C:\HomeBase\Tools\Typora\Typora.exe",
    r"C:\Program Files\Typora\Typora.exe",
    r"C:\Program Files (x86)\Typora\Typora.exe",
];

// =====================================================================
// 类型
// =====================================================================

/// Wiki 文档元数据（不含文件内容）
#[derive(Debug, Serialize, Deserialize)]
pub struct WikiDoc {
    /// 相对 C:\HomeBase\Wiki 的 POSIX 风格路径，例如 `wiki/notes/llm/transformers.md`
    pub rel_path: String,
    /// 绝对路径（Windows 反斜杠）
    pub absolute_path: String,
    /// 顶级类别（按相对路径前缀分组）
    pub kind: WikiKind,
    /// 标题（frontmatter `title` > 第一个 `# heading` > 文件名）
    pub title: String,
    /// frontmatter `doc_id`（如有），用于跨 wiki 互链
    pub doc_id: Option<String>,
    /// 字节大小
    pub size: u64,
    /// 修改时间（Unix ms）
    pub modified_ms: i64,
}

#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WikiKind {
    /// 根目录的 README / AGENTS / index.md / log.md / OBSIDIAN_SETUP
    Root,
    /// raw/ — 原始素材（Karpathy: immutable，LLM 只读）
    Raw,
    /// wiki/notes/** — evergreen 笔记
    Note,
    /// wiki/moc/** — Maps of Content
    Moc,
    /// wiki/journals/** — 工程 / 研究日志
    Journal,
    /// wiki/projects/** — 项目笔记
    Project,
    /// wiki/templates/** — 笔记模板
    Template,
    /// wiki/index/** — 派生索引（tags/backlinks/orphans）
    DerivedIndex,
    /// wiki/_meta/** — 元数据
    Meta,
    /// wiki/ 下其他位置
    WikiOther,
    /// 其他（不应该出现，兜底）
    Other,
}

/// `wiki_read` 返回（与 `DocContent` 类似，但解耦）
#[derive(Debug, Serialize, Deserialize)]
pub struct WikiContent {
    pub rel_path: String,
    pub absolute_path: String,
    pub exists: bool,
    pub size: u64,
    pub content: Option<String>,
    pub error: Option<String>,
}

// =====================================================================
// Tauri commands
// =====================================================================

/// 递归扫描 C:\HomeBase\Wiki，列出所有 .md / .markdown / .txt
///
/// **排除**：`.obsidian/` `.git/` 子树（噪音）
///
/// 软失败：扫描错误返回空列表 + warning 日志（前端显示「未读取到」）
#[tauri::command]
pub async fn wiki_list() -> Result<Vec<WikiDoc>, String> {
    tracing::debug!("wiki_list 调用");
    let root_path = crate::legacy_isolation::wiki_root();
    crate::legacy_isolation::ensure_test_path(&root_path)?;
    let root = root_path.as_path();
    if !root.exists() {
        tracing::warn!("Wiki 根目录不存在: {}", root.display());
        return Ok(vec![]);
    }
    let mut docs = Vec::new();
    walk_dir(root, root, &mut docs)?;
    docs.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
    Ok(docs)
}

/// 递归列出 C:\HomeBase\Wiki 下所有**目录**（含空目录）的相对路径
///
/// 用于前端 LLM Wiki sidebar 的目录树：`wiki_list` 只返回有 markdown 的目录，
/// 空骨架目录（如刚初始化的 `wiki/notes/llm/`）不会出现。本命令补上这缺口。
///
/// **排除**：`.obsidian/` `.git/` `.trash/` `node_modules/` 子树
///
/// 返回 POSIX 风格相对路径数组，按字母序排序
#[tauri::command]
pub async fn wiki_dirs() -> Result<Vec<String>, String> {
    tracing::debug!("wiki_dirs 调用");
    let root_path = crate::legacy_isolation::wiki_root();
    crate::legacy_isolation::ensure_test_path(&root_path)?;
    let root = root_path.as_path();
    if !root.exists() {
        return Ok(vec![]);
    }
    let mut dirs = Vec::new();
    walk_dirs_only(root, root, &mut dirs)?;
    dirs.sort();
    Ok(dirs)
}

/// 读取单篇 wiki 文档
///
/// **安全**：rel_path 必须能解析到 C:\HomeBase\Wiki 子树内（防 `../` 越权）
#[tauri::command]
pub async fn wiki_read(rel_path: String) -> Result<WikiContent, String> {
    tracing::debug!("wiki_read 调用：{rel_path}");
    let abs_path = match resolve_within_wiki(&rel_path) {
        Some(p) => p,
        None => {
            return Ok(WikiContent {
                rel_path,
                absolute_path: String::new(),
                exists: false,
                size: 0,
                content: None,
                error: Some("路径超出当前 Wiki 范围".into()),
            });
        }
    };
    let abs_str = abs_path.to_string_lossy().to_string();
    crate::legacy_isolation::ensure_test_path(&abs_path)?;

    let ext = abs_path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if !matches!(ext.as_str(), "md" | "markdown" | "txt") {
        return Ok(WikiContent {
            rel_path,
            absolute_path: abs_str,
            exists: abs_path.exists(),
            size: 0,
            content: None,
            error: Some(format!("仅支持 .md / .markdown / .txt（当前 .{ext}）")),
        });
    }

    if !abs_path.exists() {
        return Ok(WikiContent {
            rel_path,
            absolute_path: abs_str,
            exists: false,
            size: 0,
            content: None,
            error: Some("文件不存在".into()),
        });
    }

    let metadata = match fs::metadata(&abs_path) {
        Ok(m) => m,
        Err(e) => {
            return Ok(WikiContent {
                rel_path,
                absolute_path: abs_str,
                exists: true,
                size: 0,
                content: None,
                error: Some(format!("读取元数据失败: {e}")),
            });
        }
    };
    let size = metadata.len();
    const MAX_SIZE: u64 = 5 * 1024 * 1024;
    if size > MAX_SIZE {
        return Ok(WikiContent {
            rel_path,
            absolute_path: abs_str,
            exists: true,
            size,
            content: None,
            error: Some(format!("文件过大（{} > 5 MB），未读取", size)),
        });
    }

    match fs::read_to_string(&abs_path) {
        Ok(raw) => {
            let stripped = raw.strip_prefix('\u{feff}').unwrap_or(&raw).to_string();
            Ok(WikiContent {
                rel_path,
                absolute_path: abs_str,
                exists: true,
                size,
                content: Some(stripped),
                error: None,
            })
        }
        Err(e) => Ok(WikiContent {
            rel_path,
            absolute_path: abs_str,
            exists: true,
            size,
            content: None,
            error: Some(format!("读文件失败: {e}")),
        }),
    }
}

/// 用 Typora 打开 markdown 文件
///
/// **优先级**：
/// 1. `C:\HomeBase\Tools\Typora\Typora.exe`（已知用户安装位置）
/// 2. `C:\Program Files\Typora\Typora.exe`
/// 3. `C:\Program Files (x86)\Typora\Typora.exe`
/// 4. fallback: 用 OS 默认应用打开（`tauri-plugin-shell` 的 open）
///
/// **安全**：
/// - 路径必须存在
/// - 后缀必须 .md / .markdown / .txt
/// - spawn 时附 CREATE_NO_WINDOW 防闪窗
#[tauri::command]
pub async fn docs_open_in_typora(absolute_path: String) -> Result<TyporaResult, String> {
    crate::legacy_isolation::ensure_test_path(Path::new(&absolute_path))?;
    tracing::debug!("docs_open_in_typora 调用：{absolute_path}");

    let p = Path::new(&absolute_path);
    if !p.exists() {
        return Err(format!("文件不存在: {absolute_path}"));
    }
    let ext = p
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if !matches!(ext.as_str(), "md" | "markdown" | "txt") {
        return Err(format!(
            "Typora 仅支持 .md / .markdown / .txt（当前 .{ext}）"
        ));
    }

    // 找 Typora exe
    for cand in TYPORA_CANDIDATES {
        if Path::new(cand).exists() {
            tracing::info!("用 Typora 打开: {cand} {absolute_path}");
            let mut cmd = Command::new(cand);
            cmd.arg(&absolute_path);
            // Windows: 防闪窗
            #[cfg(target_os = "windows")]
            {
                const CREATE_NO_WINDOW: u32 = 0x0800_0000;
                cmd.creation_flags(CREATE_NO_WINDOW);
            }
            match cmd.spawn() {
                Ok(_) => {
                    return Ok(TyporaResult {
                        opened_with: TyporaSource::Typora,
                        path_used: Some(cand.to_string()),
                    });
                }
                Err(e) => {
                    tracing::warn!("Typora spawn 失败 ({cand}): {e}");
                    // 继续尝试下一个候选 / fallback
                }
            }
        }
    }

    // Fallback: 用 OS 默认（前端会用 plugin-shell 的 open；这里返回信号让前端走那条路）
    tracing::info!("未找到 Typora.exe，回退到 OS 默认应用");
    Ok(TyporaResult {
        opened_with: TyporaSource::FallbackShell,
        path_used: None,
    })
}

#[derive(Debug, Serialize, Deserialize)]
pub struct TyporaResult {
    pub opened_with: TyporaSource,
    /// 实际用的 Typora.exe 路径（成功时）
    pub path_used: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone, Copy)]
#[serde(rename_all = "snake_case")]
pub enum TyporaSource {
    /// 成功用 Typora 打开
    Typora,
    /// Typora 未找到，前端应回退到 plugin-shell 的 open
    FallbackShell,
}

// =====================================================================
// 内部 — 目录遍历
// =====================================================================

/// 递归遍历，把所有 .md / .markdown / .txt 加入 docs
///
/// `root` = C:\HomeBase\Wiki，`current` = 当前递归层
fn walk_dir(root: &Path, current: &Path, docs: &mut Vec<WikiDoc>) -> Result<(), String> {
    crate::legacy_isolation::ensure_test_path(current)?;
    for entry in crate::query::entries(current, false)? {
        let path = entry.path();
        crate::legacy_isolation::ensure_test_path(&path)?;
        let name = entry.file_name().to_string_lossy().to_string();

        // 排除噪音目录
        if path.is_dir() {
            if matches!(name.as_str(), ".obsidian" | ".git" | "node_modules" | "raw" | "_raw") {
                continue;
            }
            walk_dir(root, &path, docs)?;
            continue;
        }

        // 文件后缀
        let ext = path
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if !matches!(ext.as_str(), "md" | "markdown" | "txt") {
            continue;
        }
        // 排除特殊文件
        if name.starts_with('.') || name == ".manifest.json" {
            continue;
        }

        docs.push(build_wiki_doc(root, &path)?);
    }
    Ok(())
}

/// 递归收集所有目录（不含文件），返回 POSIX 风格相对路径
///
/// 用于 `wiki_dirs` — 让前端看到空骨架目录
fn walk_dirs_only(root: &Path, current: &Path, dirs: &mut Vec<String>) -> Result<(), String> {
    crate::legacy_isolation::ensure_test_path(current)?;
    for entry in crate::query::entries(current, false)? {
        let path = entry.path();
        crate::legacy_isolation::ensure_test_path(&path)?;
        if !path.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        // 排除噪音目录（与 walk_dir 保持一致 + .trash）
        if matches!(
            name.as_str(),
            ".obsidian" | ".git" | "node_modules" | ".trash" | "raw" | "_raw"
        ) {
            continue;
        }
        // 加入当前目录（相对 root）
        if let Ok(rel) = path.strip_prefix(root) {
            let rel_str = rel.to_string_lossy().replace('\\', "/");
            if !rel_str.is_empty() {
                dirs.push(rel_str);
            }
        }
        // 递归
        walk_dirs_only(root, &path, dirs)?;
    }
    Ok(())
}

/// 从绝对路径构建 WikiDoc（解析 frontmatter / 推导 kind）
fn build_wiki_doc(root: &Path, abs: &Path) -> Result<WikiDoc, String> {
    crate::legacy_isolation::ensure_test_path(abs)?;
    let rel = abs.strip_prefix(root).map_err(|_| "QUERY_INVALID: Wiki 路径不在根目录中")?;
    let rel_str = rel.to_string_lossy().replace('\\', "/");
    let kind = infer_kind(&rel_str);

    let metadata = fs::metadata(abs).map_err(|_| "QUERY_FAILED: Wiki 文件元数据不可读取")?;
    let size = metadata.len();
    let modified_ms = metadata
        .modified()
        .map_err(|_| "QUERY_FAILED: Wiki 修改时间不可读取")?
        .duration_since(std::time::UNIX_EPOCH).map_err(|_| "QUERY_INVALID: Wiki 修改时间无效")?
        .as_millis() as i64;

    // 读前 4 KB 用于 frontmatter 解析（不读全文，避免内存浪费）
    let preview = read_preview(abs, 4096).map_err(|_| "QUERY_FAILED: Wiki 文件预览不可读取")?;
    let (title, doc_id) = parse_frontmatter_title(&preview);
    let title = title
        .or_else(|| parse_first_heading(&preview))
        .unwrap_or_else(|| {
            // fallback：文件名（去后缀）
            abs.file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("(untitled)")
                .to_string()
        });

    Ok(WikiDoc {
        rel_path: rel_str,
        absolute_path: abs.to_string_lossy().to_string(),
        kind,
        title,
        doc_id,
        size,
        modified_ms,
    })
}

fn infer_kind(rel: &str) -> WikiKind {
    let parts: Vec<&str> = rel.split('/').collect();
    match parts.as_slice() {
        // 根目录文件
        [_single] => WikiKind::Root,
        // raw/**
        ["raw", ..] => WikiKind::Raw,
        // wiki/<sub>/**
        ["wiki", "notes", ..] => WikiKind::Note,
        ["wiki", "moc", ..] => WikiKind::Moc,
        ["wiki", "journals", ..] => WikiKind::Journal,
        ["wiki", "projects", ..] => WikiKind::Project,
        ["wiki", "templates", ..] => WikiKind::Template,
        ["wiki", "index", ..] => WikiKind::DerivedIndex,
        ["wiki", "_meta", ..] => WikiKind::Meta,
        ["wiki", ..] => WikiKind::WikiOther,
        _ => WikiKind::Other,
    }
}

fn read_preview(path: &Path, max_bytes: usize) -> std::io::Result<String> {
    use std::io::Read;
    let mut f = fs::File::open(path)?;
    let mut buf = Vec::with_capacity(max_bytes);
    let _ = f.take(max_bytes as u64).read_to_end(&mut buf)?;
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

/// 解析 frontmatter 头部 `--- ... ---`，返回 (title, doc_id)
///
/// 用 serde_yaml 解析为通用 Map，避免硬编码字段
fn parse_frontmatter_title(content: &str) -> (Option<String>, Option<String>) {
    // 必须以 `---` 开头
    let stripped = content.strip_prefix('\u{feff}').unwrap_or(content);
    let trimmed = stripped.trim_start();
    if !trimmed.starts_with("---") {
        return (None, None);
    }
    // 找到结束的 `---`
    let body = &trimmed[3..]; // skip 开头 ---
    let end_idx = body.find("\n---").or_else(|| body.find("\r\n---"));
    let yaml_str = match end_idx {
        Some(i) => &body[..i],
        None => return (None, None),
    };
    let map: serde_yaml::Value = match serde_yaml::from_str(yaml_str) {
        Ok(v) => v,
        Err(_) => return (None, None),
    };
    let title = map
        .get("title")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let doc_id = map
        .get("doc_id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    (title, doc_id)
}

/// 提取第一个 `# heading`（可能带 `#`、`##` 前缀清理）
fn parse_first_heading(content: &str) -> Option<String> {
    for line in content.lines().take(50) {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("# ") {
            return Some(rest.trim().to_string());
        }
    }
    None
}

/// 把 rel_path 解析到 C:\HomeBase\Wiki 子树，校验不越权
fn resolve_within_wiki(rel_path: &str) -> Option<PathBuf> {
    // 拒绝绝对路径 / 包含 ".."
    if Path::new(rel_path).is_absolute() {
        return None;
    }
    if rel_path.split(['/', '\\']).any(|seg| seg == "..") {
        return None;
    }
    let root = crate::legacy_isolation::wiki_root();
    let joined = root.join(rel_path);

    // 简单 prefix 校验（不依赖 canonicalize，因为目标可能不存在）
    let joined_str = joined.to_string_lossy().to_lowercase();
    let root_str = root.to_string_lossy().to_lowercase();
    if joined_str.starts_with(&root_str) {
        Some(joined)
    } else {
        None
    }
}

// =====================================================================
// 测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "Explicit isolated pure path probe; never scans real Wiki"]
    fn legacy_isolation_path_probe() {
        let home = PathBuf::from(std::env::var_os("HOMEBASE_TEST_HOME").unwrap());
        assert!(resolve_within_wiki("synthetic-note.md").unwrap().starts_with(home.join("Wiki")));
    }

    #[test]
    fn infer_kind_categorizes_paths() {
        assert_eq!(infer_kind("README.md"), WikiKind::Root);
        assert_eq!(infer_kind("raw/papers/foo.md"), WikiKind::Raw);
        assert_eq!(infer_kind("wiki/notes/llm/x.md"), WikiKind::Note);
        assert_eq!(infer_kind("wiki/moc/y.md"), WikiKind::Moc);
        assert_eq!(infer_kind("wiki/journals/daily/d.md"), WikiKind::Journal);
        assert_eq!(infer_kind("wiki/projects/p.md"), WikiKind::Project);
        assert_eq!(infer_kind("wiki/templates/t.md"), WikiKind::Template);
        assert_eq!(infer_kind("wiki/index/tags.md"), WikiKind::DerivedIndex);
        assert_eq!(infer_kind("wiki/_meta/taxonomy.md"), WikiKind::Meta);
        assert_eq!(infer_kind("wiki/orphan.md"), WikiKind::WikiOther);
    }

    #[test]
    fn parse_frontmatter_extracts_title_and_doc_id() {
        let md = "---\ndoc_id: my-note\ntitle: My Note\ntags: [a, b]\n---\n# heading\nbody";
        let (title, doc_id) = parse_frontmatter_title(md);
        assert_eq!(title, Some("My Note".to_string()));
        assert_eq!(doc_id, Some("my-note".to_string()));
    }

    #[test]
    fn parse_frontmatter_returns_none_when_no_frontmatter() {
        let md = "# Just a heading\nbody";
        let (title, doc_id) = parse_frontmatter_title(md);
        assert_eq!(title, None);
        assert_eq!(doc_id, None);
    }

    #[test]
    fn parse_frontmatter_handles_bom() {
        let md = "\u{feff}---\ntitle: BOM\n---\nbody";
        let (title, _) = parse_frontmatter_title(md);
        assert_eq!(title, Some("BOM".to_string()));
    }

    #[test]
    fn parse_first_heading_finds_h1() {
        let md = "some preamble\n\n# Real Title\n## sub";
        assert_eq!(parse_first_heading(md), Some("Real Title".to_string()));
    }

    #[test]
    fn resolve_within_wiki_rejects_traversal() {
        assert!(resolve_within_wiki("../etc/passwd").is_none());
        assert!(resolve_within_wiki("wiki/../../escape.md").is_none());
        assert!(resolve_within_wiki("/abs/path").is_none());
    }

    #[test]
    fn resolve_within_wiki_accepts_relative() {
        let p = resolve_within_wiki("wiki/notes/foo.md").unwrap();
        assert_eq!(p, crate::legacy_isolation::wiki_root().join("wiki/notes/foo.md"));
    }
}
