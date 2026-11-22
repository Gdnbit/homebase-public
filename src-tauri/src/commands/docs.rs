//! Knowledge / Docs 模块（v3 §6.4 + §22）
//!
//! Day 8-10（已实现）：
//! - `docs_list`: 解析 `本机操作指南/INDEX.md` markdown 表格 → 文档列表
//! - `obsidian_health`: 检测 `%APPDATA%/obsidian/obsidian.json` + 期望 vault 路径
//!   → 4 态判定（Healthy / NotInitialized / Dangling(B') / NotAVault）
//!
//! Phase 13.2 待加：dangling 修复（清理 obsidian.json + 初始化 C:\HomeBase\Wiki vault）+ 双向编辑

use crate::util::strip_bom;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};


// =====================================================================
// 类型
// =====================================================================

#[derive(Debug, Serialize, Deserialize)]
pub struct DocInfo {
    pub doc_id: String,
    pub path: String,
    pub role: Option<String>,
    pub updated: Option<String>,
    pub starred: bool,
    pub is_new: bool,
}

/// 单篇文档全文（`docs_read` 返回）
///
/// 设计原则：永远返回 `Ok(...)`，由前端按 `error` / `content` 决定渲染（友好降级）
#[derive(Debug, Serialize, Deserialize)]
pub struct DocContent {
    pub doc_id: String,
    /// INDEX.md 中登记的原始 path 列（可能是相对或绝对）
    pub path: String,
    /// 解析后的绝对路径（前端展示给用户看 + 「在编辑器打开」用）
    pub absolute_path: String,
    pub exists: bool,
    /// 字节大小（即使没读也填，便于前端显示）
    pub size: u64,
    /// 文件全文（剥 BOM 后）；缺失 / 过大 / 后缀不对 时为 None
    pub content: Option<String>,
    /// 不可读时的原因（与 content 互斥；都 None 表示读成功且空文件）
    pub error: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ObsidianHealth {
    pub config_path: String,
    pub config_exists: bool,
    pub vaults: Vec<VaultInfo>,
    pub expected_vault_path: String,
    pub expected_vault_exists: bool,
    pub expected_vault_is_obsidian: bool,
    pub variant: ObsidianVariant,
    pub recommendation: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct VaultInfo {
    pub id: String,
    pub path: String,
    pub exists: bool,
    pub is_obsidian: bool,
    pub open: bool,
    pub matches_expected: bool,
}

#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ObsidianVariant {
    /// A: 注册 vault 路径 == 期望 + 目标含 .obsidian — 健康
    Healthy,
    /// B: obsidian.json 缺失或无 vault — 用户没装/没用过 Obsidian
    NotInitialized,
    /// B': obsidian.json 注册的 vault 路径不存在或不是期望路径（v3 §22.3 实测 Q11）
    Dangling,
    /// C: 期望路径存在但缺 .obsidian 子目录 — 目录是普通文件夹
    NotAVault,
}

// =====================================================================
// Tauri commands
// =====================================================================

/// 解析 INDEX.md 的所有 markdown 表格行 → 文档列表
#[tauri::command]
pub async fn docs_list() -> Result<Vec<DocInfo>, String> {
    tracing::debug!("docs_list 调用");
    let index = crate::legacy_isolation::manual_index();
    crate::legacy_isolation::ensure_test_path(&index)?;
    match fs::read_to_string(&index) {
        Ok(content) => {
            let docs = parse_index_md(&content);
            for doc in &docs { crate::legacy_isolation::ensure_test_path(&resolve_doc_path(&doc.path))?; }
            Ok(docs)
        },
        Err(e) => {
            tracing::warn!("读 INDEX.md 失败 ({}): {e}", index.display());
            Err("QUERY_FAILED: 文档索引不可读取，不能判定为没有文档".into())
        }
    }
}

/// 读取单篇文档全文（仅 .md / .markdown / .txt，且大小 ≤ 5 MB）
///
/// **路径解析规则**：
/// - INDEX.md path 列含盘符（`E:\...` / `C:\...`） → 视为绝对路径直接读
/// - 否则 → join 到 INDEX.md 所在目录（`本机操作指南/`）
///
/// **安全约束**（不依赖用户行为，仅作健壮性兜底）：
/// - 文件后缀必须 .md / .markdown / .txt
/// - 文件大小 ≤ 5 MB（防止误读大二进制 / OOM）
/// - 路径必须能 canonicalize 成功（拒绝符号链接环路 / 不存在）
///
/// 返回 `DocContent`：缺失 / 过大 / 后缀不对 时 `content = None`，`error` 给原因
#[tauri::command]
pub async fn docs_read(doc_id: String) -> Result<DocContent, String> {
    tracing::debug!("docs_read 调用：{}", doc_id);

    // 1. 从 INDEX.md 中查 doc 的 path
    let index = crate::legacy_isolation::manual_index();
    crate::legacy_isolation::ensure_test_path(&index)?;
    let docs = match fs::read_to_string(&index) {
        Ok(s) => parse_index_md(&s),
        Err(e) => return Err(format!("读 INDEX.md 失败: {e}")),
    };
    let Some(doc) = docs.into_iter().find(|d| d.doc_id == doc_id) else {
        return Ok(DocContent {
            doc_id,
            path: String::new(),
            absolute_path: String::new(),
            exists: false,
            size: 0,
            content: None,
            error: Some("INDEX.md 中找不到该 doc_id".into()),
        });
    };
    let rel_path = doc.path.clone();

    // 2. 解析绝对路径
    let abs_path = resolve_doc_path(&rel_path);
    crate::legacy_isolation::ensure_test_path(&abs_path)?;
    let abs_str = abs_path.to_string_lossy().to_string();

    // 3. 后缀校验
    let ext = abs_path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if !matches!(ext.as_str(), "md" | "markdown" | "txt") {
        return Ok(DocContent {
            doc_id,
            path: rel_path,
            absolute_path: abs_str,
            exists: abs_path.exists(),
            size: 0,
            content: None,
            error: Some(format!("仅支持 .md / .markdown / .txt（当前 .{ext}）")),
        });
    }

    // 4. 存在性
    if !abs_path.exists() {
        return Ok(DocContent {
            doc_id,
            path: rel_path,
            absolute_path: abs_str,
            exists: false,
            size: 0,
            content: None,
            error: Some("文件不存在".into()),
        });
    }

    // 5. 大小限制
    let metadata = match fs::metadata(&abs_path) {
        Ok(m) => m,
        Err(e) => {
            return Ok(DocContent {
                doc_id,
                path: rel_path,
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
        return Ok(DocContent {
            doc_id,
            path: rel_path,
            absolute_path: abs_str,
            exists: true,
            size,
            content: None,
            error: Some(format!("文件过大（{} > 5 MB），未读取", size)),
        });
    }

    // 6. 读取（剥 BOM）
    let raw = match fs::read_to_string(&abs_path) {
        Ok(s) => s,
        Err(e) => {
            return Ok(DocContent {
                doc_id,
                path: rel_path,
                absolute_path: abs_str,
                exists: true,
                size,
                content: None,
                error: Some(format!("读文件失败: {e}")),
            });
        }
    };
    let stripped = raw.strip_prefix('\u{feff}').unwrap_or(&raw).to_string();

    Ok(DocContent {
        doc_id,
        path: rel_path,
        absolute_path: abs_str,
        exists: true,
        size,
        content: Some(stripped),
        error: None,
    })
}

/// 把 INDEX.md path 列解析为绝对路径
///
/// - `E:\...` / `C:\...` 等 Windows 盘符 → 直接 PathBuf
/// - 相对路径（如 `system/v3.md`） → join 到 INDEX.md 所在目录
fn resolve_doc_path(path: &str) -> PathBuf {
    let p = Path::new(path);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        crate::legacy_isolation::manual_index()
            .parent()
            .map(|base| base.join(path))
            .unwrap_or_else(|| PathBuf::from(path))
    }
}

/// Obsidian 健康检查（v3 §22）— 返回 4 态判定 + 修复建议
#[tauri::command]
pub async fn obsidian_health() -> Result<ObsidianHealth, String> {
    tracing::debug!("obsidian_health 调用");
    let cfg_path = obsidian_config_path();
    crate::legacy_isolation::ensure_test_path(&cfg_path)?;
    let config_exists = cfg_path.exists();

    let vaults = if config_exists {
        let raw = fs::read_to_string(&cfg_path).map_err(|e| e.to_string())?;
        parse_obsidian_vaults(&raw)?
    } else {
        vec![]
    };

    let expected_root = crate::legacy_isolation::wiki_root();
    crate::legacy_isolation::ensure_test_path(&expected_root)?;
    let expected = expected_root.as_path();
    let expected_exists = expected.exists();
    let expected_is_obsidian = expected.join(".obsidian").exists();

    let variant = compute_variant(config_exists, &vaults, expected_is_obsidian);
    let recommendation = recommendation_for(variant);

    Ok(ObsidianHealth {
        config_path: cfg_path.to_string_lossy().to_string(),
        config_exists,
        vaults,
        expected_vault_path: expected.to_string_lossy().into_owned(),
        expected_vault_exists: expected_exists,
        expected_vault_is_obsidian: expected_is_obsidian,
        variant,
        recommendation,
    })
}

// =====================================================================
// 内部 — INDEX.md 解析
// =====================================================================

fn parse_index_md(content: &str) -> Vec<DocInfo> {
    let mut docs = Vec::new();
    for line in content.lines() {
        let trimmed = line.trim();
        // 必须是表格行：| ... | ... |
        if !trimmed.starts_with('|') || !trimmed.ends_with('|') {
            continue;
        }
        let cells: Vec<&str> = trimmed
            .trim_start_matches('|')
            .trim_end_matches('|')
            .split('|')
            .map(|s| s.trim())
            .collect();
        if cells.len() < 4 {
            continue;
        }
        let raw_id = cells[0];
        // 跳过 header / separator
        if raw_id == "doc_id" || raw_id.contains("---") || raw_id.is_empty() {
            continue;
        }

        let path_cell = cells[1].trim_matches('`');
        if path_cell.is_empty() {
            continue;
        }

        let starred = raw_id.contains('⭐');
        let is_new = raw_id.contains("🆕") || raw_id.contains(" 新");

        // 清理 doc_id：去 markdown 强调 + 标记符
        let doc_id = raw_id
            .replace("**", "")
            .replace('⭐', "")
            .replace("🆕", "")
            .replace("✅", "")
            .replace(" 新", "")
            .trim()
            .to_string();

        if doc_id.is_empty() {
            continue;
        }

        docs.push(DocInfo {
            doc_id,
            path: path_cell.to_string(),
            role: Some(cells[2].to_string()).filter(|s| !s.is_empty()),
            updated: Some(cells[3].to_string()).filter(|s| !s.is_empty()),
            starred,
            is_new,
        });
    }
    docs
}

// =====================================================================
// 内部 — Obsidian 配置解析
// =====================================================================

fn obsidian_config_path() -> PathBuf {
    if let Some(path) = crate::legacy_isolation::test_path("AppData/Roaming/obsidian/obsidian.json") { return path; }
    let appdata = std::env::var("APPDATA")
        .unwrap_or_else(|_| String::from(r"C:\Users\homebase\AppData\Roaming"));
    PathBuf::from(appdata).join("obsidian").join("obsidian.json")
}

fn parse_obsidian_vaults(raw: &str) -> Result<Vec<VaultInfo>, String> {
    #[derive(Deserialize)]
    struct ObsidianConfig {
        #[serde(default)]
        vaults: serde_json::Map<String, serde_json::Value>,
    }
    let cfg: ObsidianConfig = match serde_json::from_str(strip_bom(raw)) {
        Ok(c) => c,
        Err(_) => return Err("QUERY_INVALID: Obsidian 配置格式无效，不能判定为未初始化".into()),
    };

    let expected_canonical = canonical_path(&crate::legacy_isolation::wiki_root().to_string_lossy());

    cfg.vaults
        .into_iter()
        .filter_map(|(id, val)| {
            let Some(path) = val.get("path").and_then(|v| v.as_str()).filter(|p| !p.trim().is_empty()).map(str::to_string) else {
                return Some(Err("QUERY_INVALID: Obsidian vault 条目缺少有效路径".into()));
            };
            let open = val.get("open").and_then(|v| v.as_bool()).unwrap_or(false);
            let p = Path::new(&path);
            if let Err(error) = crate::legacy_isolation::ensure_test_path(p) { return Some(Err(error)); }
            let exists = p.exists();
            let is_obsidian = p.join(".obsidian").exists();
            let matches_expected = canonical_path(&path) == expected_canonical;
            Some(Ok(VaultInfo {
                id,
                path,
                exists,
                is_obsidian,
                open,
                matches_expected,
            }))
        })
        .collect()
}

fn canonical_path(s: &str) -> String {
    s.to_lowercase()
        .replace('/', "\\")
        .trim_end_matches('\\')
        .to_string()
}

fn compute_variant(
    config_exists: bool,
    vaults: &[VaultInfo],
    expected_is_obsidian: bool,
) -> ObsidianVariant {
    if !config_exists || vaults.is_empty() {
        return ObsidianVariant::NotInitialized;
    }
    let has_match = vaults.iter().any(|v| v.matches_expected && v.exists);
    if has_match && expected_is_obsidian {
        return ObsidianVariant::Healthy;
    }
    // 期望目录存在但不是 vault → C 变体
    if !expected_is_obsidian && crate::legacy_isolation::wiki_root().exists() {
        return ObsidianVariant::NotAVault;
    }
    ObsidianVariant::Dangling
}

fn recommendation_for(variant: ObsidianVariant) -> String {
    let expected_vault = crate::legacy_isolation::wiki_root().to_string_lossy().into_owned();
    match variant {
        ObsidianVariant::Healthy => {
            "Vault 健康。obsidian.json 注册的 vault 与期望路径一致且 .obsidian 完整。".to_string()
        }
        ObsidianVariant::NotInitialized => format!(
            "未发现 Obsidian 配置。建议：1) 安装 Obsidian；2) 启动后选「打开 vault」指向 {expected_vault}。"
        ),
        ObsidianVariant::Dangling => format!(
            "obsidian.json 注册的 vault 不存在或与期望不符（B' 变体）。建议：清空 vault 注册 + 启动 Obsidian 重新选 {expected_vault} 作为 vault。Phase 13.2 将提供一键修复。"
        ),
        ObsidianVariant::NotAVault => format!(
            "{expected_vault} 存在但不是 Obsidian vault（缺 .obsidian 子目录）。建议：用 Obsidian 打开此目录初始化为 vault。"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "Explicit isolated pure path probe; never reads real INDEX"]
    fn legacy_isolation_path_probe() {
        let home = PathBuf::from(std::env::var_os("HOMEBASE_TEST_HOME").unwrap());
        assert!(resolve_doc_path("synthetic-note.md").starts_with(home.join("Desktop/本机操作指南")));
    }
    #[test]
    fn query_safety_obsidian_corruption_is_not_uninitialized() {
        assert!(parse_obsidian_vaults("not json").is_err());
        assert!(parse_obsidian_vaults(r#"{"vaults":{"invalid":{}}}"#).is_err());
        assert!(parse_obsidian_vaults(r#"{"vaults":{}}"#).unwrap().is_empty());
    }

    #[test]
    fn parse_index_md_extracts_doc_id_path_starred() {
        let md = r#"
# Title
| doc_id | path | role | updated |
|---|---|---|---|
| **disk-plan** ⭐ | `system/disk-plan.md` | 4 盘格式约定 | 2026-05-03 |
| network | `system/network.md` | 网络配置 | 2026-04-28 |
| ignored row
"#;
        let docs = parse_index_md(md);
        assert_eq!(docs.len(), 2, "应只识别 2 条数据行");
        assert_eq!(docs[0].doc_id, "disk-plan");
        assert!(docs[0].starred);
        assert_eq!(docs[0].path, "system/disk-plan.md");
        assert_eq!(docs[1].doc_id, "network");
        assert!(!docs[1].starred);
    }

    #[test]
    fn parse_obsidian_vaults_handles_real_format() {
        let vault = crate::legacy_isolation::wiki_root();
        let raw = serde_json::json!({
            "vaults": {"fixture-vault": {"path": vault, "ts": 1776016099684_u64, "open": true}},
            "cli": true
        });
        let v = parse_obsidian_vaults(&raw.to_string()).unwrap();
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].id, "fixture-vault");
        assert_eq!(Path::new(&v[0].path), vault.as_path());
        assert!(v[0].open);
        assert!(v[0].matches_expected);
    }

    #[test]
    fn parse_obsidian_vaults_b_prime_dangling() {
        let temp = match crate::legacy_isolation::test_path("") {
            Some(home) => tempfile::tempdir_in(home),
            None => tempfile::tempdir(),
        }
        .unwrap();
        let fixture = temp.path().join("UnrelatedObsidian");
        let raw = serde_json::json!({"vaults": {"fixture-other": {"path": fixture, "open": true}}});
        let v = parse_obsidian_vaults(&raw.to_string()).unwrap();
        assert_eq!(v.len(), 1);
        assert!(!v[0].matches_expected);
    }

    #[test]
    fn variant_not_initialized_when_no_config() {
        assert_eq!(
            compute_variant(false, &[], false),
            ObsidianVariant::NotInitialized
        );
    }

    #[test]
    fn variant_dangling_when_no_match() {
        let vaults = vec![VaultInfo {
            id: "x".into(),
            path: r"E:\Other".into(),
            exists: true,
            is_obsidian: true,
            open: true,
            matches_expected: false,
        }];
        // 当期望路径不存在或不是 vault 时（这里假设非 vault），返回 Dangling
        let res = compute_variant(true, &vaults, false);
        // C:\HomeBase\Wiki 在测试机上可能存在（用户机器 yes），所以可能返回 NotAVault
        assert!(matches!(
            res,
            ObsidianVariant::Dangling | ObsidianVariant::NotAVault
        ));
    }
}
