//! Skills 共享池（S4 · 2026-05-06）
//!
//! 单一数据源 · `~/.homebase/shared/skills/<name>/`（含 SKILL.md + 任意 supporting files）
//! Reconcile 到：
//! - `~/.claude/skills/homebase-<name>/`（Claude Code 格式）
//! - `~/.openclaw/skills/homebase-<name>/`（OpenClaw fork · 同 Claude 格式）
//!
//! 严格不变量：**只动以 `homebase-` 前缀开头的 skill 目录**。
//! USER 自加的 skill（如 `~/.claude/skills/my-deploy/`）一字不动。
//!
//! 不支持 codex / gemini-cli（skill 格式不同 · 推后到 plugin 阶段）。

use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::PathBuf;

pub const HB_MANAGED_PREFIX: &str = "homebase-";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillInfo {
    /// 短 id（不含前缀）· UI 看到的 / `~/.homebase/shared/skills/<id>/`
    pub id: String,
    /// 文件大小（SKILL.md）· 给 UI 显示
    pub size_bytes: u64,
    /// 是否含 SKILL.md
    pub has_skill_md: bool,
    /// 修改时间（Unix 秒 · None = 取不到）
    pub modified_at: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillsReconcileEntry {
    pub agent_id: String,
    pub path: String,
    pub status: String,
    pub copied_count: usize,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SkillsReconcileReport {
    pub agents: Vec<SkillsReconcileEntry>,
}

// =====================================================================
// 路径定位
// =====================================================================

fn home() -> PathBuf {
    super::homebase_home_dir()
}

pub fn shared_skills_root() -> PathBuf {
    home().join(".homebase").join("shared").join("skills")
}

fn claude_skills_root() -> PathBuf {
    home().join(".claude").join("skills")
}

fn openclaw_skills_root() -> PathBuf {
    home().join(".openclaw").join("skills")
}

// =====================================================================
// 列表
// =====================================================================

/// 列出 ~/.homebase/shared/skills/ 下所有 skill（每个子目录就是一个 skill）
pub fn list_skills() -> Result<Vec<SkillInfo>, String> {
    let root = shared_skills_root();
    list_skills_at(&root)
}

fn list_skills_at(root: &std::path::Path) -> Result<Vec<SkillInfo>, String> {
    let mut out: Vec<SkillInfo> = Vec::new();
    for entry in crate::query::entries(&root, true)? {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let id = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };
        // 跳隐藏目录
        if id.starts_with('.') {
            continue;
        }
        let skill_md = path.join("SKILL.md");
        let has_skill_md = skill_md.exists();
        let (size_bytes, modified_at) = match std::fs::metadata(&skill_md) {
            Ok(m) => (
                m.len(),
                m.modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs()),
            ),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (0, None),
            Err(_) => return Err("QUERY_FAILED: Skill 元数据不可读取，不能当作空文件".into()),
        };
        out.push(SkillInfo {
            id,
            size_bytes,
            has_skill_md,
            modified_at,
        });
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(out)
}

// =====================================================================
// Reconcile
// =====================================================================

/// 把 ~/.homebase/shared/skills/<id>/ 复制到 2 个 agent 的 skills 目录
/// （加 `homebase-` 前缀避免冲突 USER 自加 skill）
pub fn reconcile_all() -> SkillsReconcileReport {
    let mut report = SkillsReconcileReport::default();
    let skills = match list_skills() {
        Ok(s) => s,
        Err(e) => {
            // 列失败 · 两个 agent 都返 error
            for (id, root) in [
                ("claude-code", claude_skills_root()),
                ("openclaw", openclaw_skills_root()),
            ] {
                report.agents.push(SkillsReconcileEntry {
                    agent_id: id.into(),
                    path: root.to_string_lossy().into_owned(),
                    status: "error".into(),
                    copied_count: 0,
                    message: Some(format!("列共享 skills 失败: {e}")),
                });
            }
            return report;
        }
    };

    let agents = [
        ("claude-code", claude_skills_root(), home().join(".claude")),
        ("openclaw", openclaw_skills_root(), home().join(".openclaw")),
    ];

    for (id, dest_root, agent_dir) in agents {
        let entry = sync_one_agent(id, &dest_root, &agent_dir, &skills);
        report.agents.push(entry);
    }
    report
}

fn sync_one_agent(
    agent_id: &str,
    dest_root: &PathBuf,
    agent_dir: &PathBuf,
    skills: &[SkillInfo],
) -> SkillsReconcileEntry {
    if !agent_dir.exists() {
        return SkillsReconcileEntry {
            agent_id: agent_id.into(),
            path: dest_root.to_string_lossy().into_owned(),
            status: "skipped".into(),
            copied_count: 0,
            message: Some(format!(
                "{agent_id} 未安装（{} 不存在）",
                agent_dir.display()
            )),
        };
    }

    let snapshot = match super::snapshot::create_snapshot(
        &format!("before skills_pool::{agent_id}"),
        &[dest_root.clone()],
    ) {
        Ok(s) => s,
        Err(e) => {
            return SkillsReconcileEntry {
                agent_id: agent_id.into(),
                path: dest_root.to_string_lossy().into_owned(),
                status: "error".into(),
                copied_count: 0,
                message: Some(format!("snapshot: {e}")),
            }
        }
    };

    if let Err(e) = std::fs::create_dir_all(dest_root) {
        return SkillsReconcileEntry {
            agent_id: agent_id.into(),
            path: dest_root.to_string_lossy().into_owned(),
            status: "error".into(),
            copied_count: 0,
            message: Some(format!("create_dir {dest_root:?}: {e}")),
        };
    }

    // 1. 删旧的 homebase- 前缀 skill 目录
    if let Ok(entries) = std::fs::read_dir(dest_root) {
        for ent in entries.flatten() {
            let p = ent.path();
            if !p.is_dir() {
                continue;
            }
            let name = match p.file_name().and_then(|n| n.to_str()) {
                Some(n) => n.to_string(),
                None => continue,
            };
            if name.starts_with(HB_MANAGED_PREFIX) {
                if let Err(e) = std::fs::remove_dir_all(&p) {
                    tracing::warn!("删旧 skill {} 失败: {}", p.display(), e);
                }
            }
        }
    }

    // 2. 复制每个 source skill 到 dest_root/homebase-<id>/
    let src_root = shared_skills_root();
    let mut copied = 0;
    for s in skills {
        if !s.has_skill_md {
            continue;
        }
        let src = src_root.join(&s.id);
        let dest = dest_root.join(format!("{HB_MANAGED_PREFIX}{}", s.id));
        if let Err(e) = copy_dir_recursive(&src, &dest) {
            return SkillsReconcileEntry {
                agent_id: agent_id.into(),
                path: dest_root.to_string_lossy().into_owned(),
                status: "error".into(),
                copied_count: copied,
                message: Some(format!("copy {} → {}: {e}", src.display(), dest.display())),
            };
        }
        copied += 1;
    }

    super::audit::record(
        "skills.agent_skills_applied",
        dest_root.to_string_lossy(),
        json!({
            "agent_id": agent_id,
            "copied_count": copied,
            "snapshot_id": snapshot.snapshot_id
        }),
    );
    SkillsReconcileEntry {
        agent_id: agent_id.into(),
        path: dest_root.to_string_lossy().into_owned(),
        status: "applied".into(),
        copied_count: copied,
        message: None,
    }
}

/// 递归复制目录（用 walkdir 已在 deps）
fn copy_dir_recursive(src: &PathBuf, dest: &PathBuf) -> Result<(), String> {
    std::fs::create_dir_all(dest).map_err(|e| format!("create_dir {dest:?}: {e}"))?;
    for entry in walkdir::WalkDir::new(src)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        let p = entry.path();
        let rel = p
            .strip_prefix(src)
            .map_err(|e| format!("strip_prefix: {e}"))?;
        let target = dest.join(rel);
        if p.is_dir() {
            std::fs::create_dir_all(&target).map_err(|e| format!("create_dir {target:?}: {e}"))?;
        } else if p.is_file() {
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("create_dir {parent:?}: {e}"))?;
            }
            std::fs::copy(p, &target)
                .map_err(|e| format!("copy {} → {target:?}: {e}", p.display()))?;
        }
    }
    Ok(())
}

// =====================================================================
// Tests
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn query_safety_skill_listing_does_not_create_missing_directories() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("missing-skills");
        assert!(list_skills_at(&missing).unwrap().is_empty());
        assert!(!missing.exists());
        std::fs::write(&missing, "synthetic non-directory").unwrap();
        assert!(list_skills_at(&missing).is_err());
        assert_eq!(std::fs::read_to_string(&missing).unwrap(), "synthetic non-directory");
    }
    use crate::adapters::shared_context::tests::TEST_ENV_LOCK;

    fn with_test_home<F: FnOnce()>(tmp: &tempfile::TempDir, f: F) {
        let _guard = TEST_ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let prev = std::env::var("HOMEBASE_TEST_HOME").ok();
        std::env::set_var("HOMEBASE_TEST_HOME", tmp.path());
        f();
        match prev {
            Some(v) => std::env::set_var("HOMEBASE_TEST_HOME", v),
            None => std::env::remove_var("HOMEBASE_TEST_HOME"),
        }
    }

    #[test]
    fn list_creates_dir_when_missing() {
        let tmp = tempfile::tempdir().unwrap();
        with_test_home(&tmp, || {
            let skills = list_skills().expect("list ok");
            assert!(skills.is_empty());
            assert!(shared_skills_root().exists());
        });
    }

    #[test]
    fn list_finds_skills_with_skill_md() {
        let tmp = tempfile::tempdir().unwrap();
        with_test_home(&tmp, || {
            let root = shared_skills_root();
            std::fs::create_dir_all(root.join("foo")).unwrap();
            std::fs::write(
                root.join("foo").join("SKILL.md"),
                "---\ndescription: x\n---\nbody",
            )
            .unwrap();
            std::fs::create_dir_all(root.join("bar")).unwrap(); // 没 SKILL.md
            let skills = list_skills().expect("list ok");
            assert_eq!(skills.len(), 2);
            let foo = skills.iter().find(|s| s.id == "foo").unwrap();
            assert!(foo.has_skill_md);
            assert!(foo.size_bytes > 0);
            let bar = skills.iter().find(|s| s.id == "bar").unwrap();
            assert!(!bar.has_skill_md);
        });
    }

    #[test]
    fn reconcile_skips_missing_agent_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        with_test_home(&tmp, || {
            let report = reconcile_all();
            assert_eq!(report.agents.len(), 2);
            for r in &report.agents {
                assert_eq!(r.status, "skipped", "{:?}", r);
            }
        });
    }

    #[test]
    fn reconcile_copies_homebase_prefix_only() {
        let tmp = tempfile::tempdir().unwrap();
        with_test_home(&tmp, || {
            // Source skill
            let src = shared_skills_root();
            std::fs::create_dir_all(src.join("test-skill")).unwrap();
            std::fs::write(src.join("test-skill").join("SKILL.md"), "test content").unwrap();

            // Mock claude dir + USER 自己加的 skill
            let claude = tmp.path().join(".claude");
            std::fs::create_dir_all(claude.join("skills").join("my-own")).unwrap();
            std::fs::write(
                claude.join("skills").join("my-own").join("SKILL.md"),
                "user",
            )
            .unwrap();

            // 旧 homebase- 前缀（应被删）
            std::fs::create_dir_all(claude.join("skills").join("homebase-stale")).unwrap();
            std::fs::write(
                claude
                    .join("skills")
                    .join("homebase-stale")
                    .join("SKILL.md"),
                "stale",
            )
            .unwrap();

            let report = reconcile_all();
            let claude_entry = report
                .agents
                .iter()
                .find(|a| a.agent_id == "claude-code")
                .unwrap();
            assert_eq!(claude_entry.status, "applied", "{:?}", claude_entry);
            assert_eq!(claude_entry.copied_count, 1);

            // USER 自加的还在
            assert!(claude.join("skills").join("my-own").exists());
            // 旧 homebase- 删了
            assert!(!claude.join("skills").join("homebase-stale").exists());
            // 新 homebase- 加了
            assert!(claude
                .join("skills")
                .join("homebase-test-skill")
                .join("SKILL.md")
                .exists());
        });
    }
}
