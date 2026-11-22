//! git 命令适配器（Projects 模块用 — v3 §10.1 Projects.git）
//!
//! 所有方法异步，默认 30s timeout。返回结构化数据而不是 raw stdout。

use super::{run_checked, AdapterResult, CommandOutput, DEFAULT_TIMEOUT};
use serde::{Deserialize, Serialize};
use std::path::Path;

const GIT_EXE: &str = "git";

/// git 适配器
pub struct GitAdapter;

impl GitAdapter {
    pub fn new() -> Self {
        Self
    }

    /// `git status --porcelain=v1` 的结构化结果
    pub async fn status(&self, repo: &Path) -> AdapterResult<Vec<FileStatus>> {
        let out: CommandOutput = run_checked(
            GIT_EXE,
            ["status", "--porcelain=v1", "--untracked-files=all"],
            Some(repo),
            DEFAULT_TIMEOUT,
        )
        .await?;
        Ok(parse_porcelain(&out.stdout))
    }

    /// 当前分支名（detached HEAD 返回 "HEAD"）
    pub async fn current_branch(&self, repo: &Path) -> AdapterResult<String> {
        let out = run_checked(
            GIT_EXE,
            ["rev-parse", "--abbrev-ref", "HEAD"],
            Some(repo),
            DEFAULT_TIMEOUT,
        )
        .await?;
        Ok(out.stdout.trim().to_string())
    }

    /// 最近 N 条 commits（hash/author/summary/iso-date）
    pub async fn log_recent(&self, repo: &Path, n: usize) -> AdapterResult<Vec<CommitInfo>> {
        // 用 \x1f 作字段分隔，\x1e 作记录分隔（避免 summary 里有 tab/逗号误切）
        let fmt = "--pretty=format:%H%x1f%an%x1f%aI%x1f%s%x1e";
        let n_arg = format!("-{}", n);
        let out = run_checked(
            GIT_EXE,
            ["log", &n_arg, fmt],
            Some(repo),
            DEFAULT_TIMEOUT,
        )
        .await?;
        Ok(parse_log(&out.stdout))
    }

    /// 工作树是否干净（无 modified/staged/untracked）
    pub async fn is_clean(&self, repo: &Path) -> AdapterResult<bool> {
        Ok(self.status(repo).await?.is_empty())
    }
}

impl Default for GitAdapter {
    fn default() -> Self {
        Self::new()
    }
}

/// 单个文件的 git 状态（简化版 porcelain v1）
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FileStatus {
    pub path: String,
    pub code: GitStatusCode,
}

/// v3 §10.1 Projects.git 用这些分类
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum GitStatusCode {
    Untracked,
    Modified,
    Added,
    Deleted,
    Renamed,
    Copied,
    Unmerged,
    /// 未识别的状态码（罕见，如子模块 change）
    Other,
}

impl GitStatusCode {
    fn from_porcelain(code: &str) -> Self {
        // porcelain v1 code 是两字节：index | worktree；我们看 combined
        match code {
            "??" => Self::Untracked,
            c if c.contains('M') => Self::Modified,
            c if c.contains('A') => Self::Added,
            c if c.contains('D') => Self::Deleted,
            c if c.contains('R') => Self::Renamed,
            c if c.contains('C') => Self::Copied,
            c if c.contains('U') => Self::Unmerged,
            _ => Self::Other,
        }
    }
}

fn parse_porcelain(stdout: &str) -> Vec<FileStatus> {
    stdout
        .lines()
        .filter_map(|line| {
            // 格式: "XY <path>"（X=index, Y=worktree），rename 时 "R  old -> new"
            if line.len() < 4 {
                return None;
            }
            let (code, rest) = line.split_at(2);
            let path = rest.trim().to_string();
            if path.is_empty() {
                return None;
            }
            Some(FileStatus {
                path,
                code: GitStatusCode::from_porcelain(code),
            })
        })
        .collect()
}

/// 单个 commit 记录
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CommitInfo {
    pub hash: String,
    pub author: String,
    /// ISO 8601 (chrono 解析不强制 — 保留原字符串给前端)
    pub date: String,
    pub summary: String,
}

fn parse_log(stdout: &str) -> Vec<CommitInfo> {
    stdout
        .split('\x1e')
        .filter_map(|record| {
            let trimmed = record.trim();
            if trimmed.is_empty() {
                return None;
            }
            let mut parts = trimmed.splitn(4, '\x1f');
            Some(CommitInfo {
                hash: parts.next()?.to_string(),
                author: parts.next()?.to_string(),
                date: parts.next()?.to_string(),
                summary: parts.next().unwrap_or("").to_string(),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// HomeBase 项目根（测试跑在此仓库里 — Day 1-2 git init 过）
    fn homebase_root() -> PathBuf {
        // CARGO_MANIFEST_DIR = src-tauri/，上一级是项目根
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("CARGO_MANIFEST_DIR has a parent")
            .to_path_buf()
    }

    #[tokio::test]
    async fn current_branch_is_master_or_main() {
        let adapter = GitAdapter::new();
        let branch = adapter.current_branch(&homebase_root()).await.expect("branch");
        assert!(
            branch == "master" || branch == "main",
            "expected master or main, got {:?}",
            branch
        );
    }

    #[tokio::test]
    async fn log_has_commits() {
        let adapter = GitAdapter::new();
        let commits = adapter
            .log_recent(&homebase_root(), 5)
            .await
            .expect("log");
        assert!(!commits.is_empty(), "HomeBase should have commits");
        // 第一个 commit 应该是 scaffold 或之后的
        let first_msg = &commits[0].summary;
        assert!(
            !first_msg.is_empty(),
            "first commit summary should not be empty"
        );
        // hash 是 40 字符 SHA-1
        assert_eq!(commits[0].hash.len(), 40, "git hash should be 40 chars");
    }

    #[tokio::test]
    async fn status_runs_without_error() {
        // 不 assert 内容（工作树状态会变），只验证命令能跑成功
        let adapter = GitAdapter::new();
        let _status = adapter.status(&homebase_root()).await.expect("status");
    }

    #[test]
    fn parse_porcelain_handles_multiple_kinds() {
        let input = " M src/lib.rs\n?? tauri-dev.log\nA  new.md\n M .gitignore\n";
        let result = parse_porcelain(input);
        assert_eq!(result.len(), 4);
        assert_eq!(result[0].code, GitStatusCode::Modified);
        assert_eq!(result[0].path, "src/lib.rs");
        assert_eq!(result[1].code, GitStatusCode::Untracked);
        assert_eq!(result[1].path, "tauri-dev.log");
        assert_eq!(result[2].code, GitStatusCode::Added);
        assert_eq!(result[2].path, "new.md");
    }

    #[test]
    fn parse_log_handles_field_separators() {
        // 模拟真实 git log 输出
        let input = "abc123\x1fAlice\x1f2026-05-03T10:00:00+08:00\x1ffirst commit\x1e\
                     def456\x1fBob\x1f2026-05-03T11:00:00+08:00\x1fsecond\x1e";
        let result = parse_log(input);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].hash, "abc123");
        assert_eq!(result[0].author, "Alice");
        assert_eq!(result[0].summary, "first commit");
        assert_eq!(result[1].author, "Bob");
    }
}
