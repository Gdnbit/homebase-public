//! PowerShell 通用适配器（v3 §10.1 — Disks / PATH / 其他 Windows 原生操作）
//!
//! 用 `powershell.exe -NoProfile -Command` 执行脚本。
//! 自动把输出编码设为 UTF-8（Windows PowerShell 5.1 默认可能 UTF-16/gbk）。

use super::{run_checked, AdapterError, AdapterResult, CommandOutput, DEFAULT_TIMEOUT};
use serde::{de::DeserializeOwned, Deserialize, Serialize};

const PS_EXE: &str = "powershell.exe";

/// PowerShell 适配器
pub struct PowerShellAdapter;

impl PowerShellAdapter {
    pub fn new() -> Self {
        Self
    }

    /// 执行任意 PowerShell 脚本；返回完整 CommandOutput
    pub async fn run(&self, script: &str) -> AdapterResult<CommandOutput> {
        let wrapped = wrap_with_utf8(script);
        run_checked(
            PS_EXE,
            ["-NoProfile", "-NonInteractive", "-Command", &wrapped],
            None,
            DEFAULT_TIMEOUT,
        )
        .await
    }

    /// 执行脚本 + 解析 stdout 为 JSON 对象（脚本末尾应输出 `ConvertTo-Json`）
    pub async fn run_json<T: DeserializeOwned>(&self, script: &str) -> AdapterResult<T> {
        let out = self.run(script).await?;
        let trimmed = out.stdout.trim();
        serde_json::from_str(trimmed).map_err(|e| AdapterError::Io {
            program: PS_EXE.to_string(),
            source: std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "QUERY_INVALID: PowerShell JSON 结果无效（{:?}）",
                    e.classify()
                ),
            ),
        })
    }

    /// 列出所有 FileSystem PSDrive（v3 §10.1 Dashboard.disks 数据源）
    pub async fn get_psdrives(&self) -> AdapterResult<Vec<PsDrive>> {
        // ConvertTo-Json 在单对象时不会包装成数组；用 @() 强制数组
        let script = "$drives = @(Get-PSDrive -PSProvider FileSystem -ErrorAction Stop | Select-Object Name, Used, Free, Root); ConvertTo-Json -InputObject $drives -Compress -Depth 3";
        // 单元素时 @() + ConvertTo-Json 仍然可能出 object 或 array
        // 先尝试数组反序列化，失败再尝试单对象
        let out = self.run(script).await?;
        let trimmed = out.stdout.trim();
        if trimmed.is_empty() {
            return Err(AdapterError::Io {
                program: PS_EXE.into(),
                source: std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "QUERY_INVALID: 磁盘查询没有返回 JSON",
                ),
            });
        }
        serde_json::from_str::<Vec<PsDrive>>(trimmed)
            .or_else(|_| serde_json::from_str::<PsDrive>(trimmed).map(|d| vec![d]))
            .map_err(|e| AdapterError::Io {
                program: PS_EXE.to_string(),
                source: std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("QUERY_INVALID: 磁盘查询 JSON 无效（{:?}）", e.classify()),
                ),
            })
    }
}

impl Default for PowerShellAdapter {
    fn default() -> Self {
        Self::new()
    }
}

/// Dashboard.disks 数据结构（Bytes 单位；Used/Free 对非 Disk provider 可能为 null）
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PsDrive {
    #[serde(rename = "Name")]
    pub name: String,
    #[serde(rename = "Used", default)]
    pub used: Option<u64>,
    #[serde(rename = "Free", default)]
    pub free: Option<u64>,
    #[serde(rename = "Root")]
    pub root: String,
}

impl PsDrive {
    /// 总容量 (Used + Free)，如果任一缺失返回 None
    pub fn total(&self) -> Option<u64> {
        match (self.used, self.free) {
            (Some(u), Some(f)) => u.checked_add(f),
            _ => None,
        }
    }
}

/// 给脚本套一个 UTF-8 输出编码 preamble
fn wrap_with_utf8(script: &str) -> String {
    format!(
        "$ErrorActionPreference = 'Stop'; [Console]::OutputEncoding = [System.Text.Encoding]::UTF8; $OutputEncoding = [System.Text.Encoding]::UTF8; {}",
        script
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn query_safety_powershell_errors_and_malformed_json_are_explicit() {
        let ps = PowerShellAdapter::new();
        assert!(ps.run("Write-Error 'synthetic query failure'; '{}' ").await.is_err());
        let error = ps.run_json::<serde_json::Value>("'synthetic-private-invalid-json'").await.unwrap_err().to_string();
        assert!(error.contains("QUERY_INVALID"));
        assert!(!error.contains("synthetic-private-invalid-json"));
    }

    #[tokio::test]
    async fn run_get_date_succeeds() {
        let ps = PowerShellAdapter::new();
        let out = ps
            .run("Get-Date -Format 'yyyy-MM-dd'")
            .await
            .expect("Get-Date");
        assert!(out.is_success(), "Get-Date exit = {}", out.exit_code);
        // stdout 应该是 "2026-05-03" 样式 — 至少含 4 位年份
        assert!(
            out.stdout
                .trim()
                .chars()
                .take(4)
                .all(|c| c.is_ascii_digit()),
            "expected year prefix, got {:?}",
            out.stdout
        );
    }

    #[tokio::test]
    async fn run_json_works_for_simple_object() {
        #[derive(Deserialize)]
        struct Foo {
            bar: i32,
            baz: String,
        }
        let ps = PowerShellAdapter::new();
        let result: Foo = ps
            .run_json("[PSCustomObject]@{bar=42; baz='hello'} | ConvertTo-Json -Compress")
            .await
            .expect("run_json");
        assert_eq!(result.bar, 42);
        assert_eq!(result.baz, "hello");
    }

    #[tokio::test]
    async fn get_psdrives_returns_c_drive() {
        let ps = PowerShellAdapter::new();
        let drives = ps.get_psdrives().await.expect("get_psdrives");
        assert!(!drives.is_empty(), "should have at least C: drive");
        let has_c = drives.iter().any(|d| d.name.eq_ignore_ascii_case("C"));
        assert!(has_c, "C: drive should always exist; drives: {drives:?}");
        // C: 应该有 Used + Free
        let c_drive = drives
            .iter()
            .find(|d| d.name.eq_ignore_ascii_case("C"))
            .unwrap();
        assert!(c_drive.total().is_some(), "C: drive should have total size");
    }

    #[tokio::test]
    async fn nonzero_exit_surfaces_error() {
        let ps = PowerShellAdapter::new();
        // `exit 3` 返回 exit code 3
        let res = ps.run("exit 3").await;
        assert!(matches!(
            res,
            Err(AdapterError::NonZeroExit { code: 3, .. })
        ));
    }
}
