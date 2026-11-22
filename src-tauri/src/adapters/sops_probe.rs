//! Credential-free offline readiness probe with bounded cold-start recovery.
use super::{spawn_with_timeout_env_input_remove, AdapterError};
use std::{
    path::Path,
    time::{Duration, Instant},
};

#[derive(Clone, Copy)]
pub struct ProbePolicy {
    pub first: Duration,
    pub retry: Duration,
}
impl Default for ProbePolicy {
    fn default() -> Self {
        Self {
            first: Duration::from_secs(5),
            retry: Duration::from_secs(15),
        }
    }
}
pub struct ProbeSuccess {
    pub version: String,
    pub attempts: usize,
    pub elapsed_ms: u128,
}
pub enum ProbeFailure {
    Missing,
    Setup,
    Spawn,
    Exit(i32),
    InvalidOutput,
    Timeout { attempts: usize, elapsed_ms: u128 },
}
impl ProbeFailure {
    pub fn retryable(&self) -> bool {
        matches!(self, Self::Timeout { .. })
    }
    pub fn message(&self) -> String {
        match self {
            Self::Missing => "SOPS 可执行文件不存在或不是普通文件，请检查工具路径".into(),
            Self::Setup => "无法准备隔离的 SOPS 启动检查目录，请检查临时目录权限".into(),
            Self::Spawn => "SOPS 无法启动，请检查文件权限或重新准备项目运行时工具".into(),
            Self::Exit(code) => format!("SOPS 版本探测退出码为 {code}，请检查工具是否完整；敏感输出未回显"),
            Self::InvalidOutput => "工具退出但没有返回有效的 SOPS 版本号，请检查是否配置了正确的 sops.exe".into(),
            Self::Timeout {attempts,elapsed_ms} => format!("SOPS 启动检查连续 {attempts} 次超时（约 {:.1} 秒）；尚未确认工具可用，不代表密钥丢失。系统将有限次自动复检，也可在资源空闲后手动重新检查", *elapsed_ms as f64 / 1000.0),
        }
    }
}
pub fn parse_version(stdout: &str) -> Option<String> {
    let text = stdout.trim();
    let version = text.strip_prefix("sops ")?;
    if version.len() > 100
        || !version
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b".-+".contains(&c))
    {
        return None;
    }
    let core = version.split(['-', '+']).next()?;
    let parts: Vec<_> = core.split('.').collect();
    if parts.len() != 3
        || parts
            .iter()
            .any(|part| part.is_empty() || !part.bytes().all(|c| c.is_ascii_digit()))
    {
        return None;
    }
    Some(version.into())
}
pub async fn probe(executable: &Path, policy: ProbePolicy) -> Result<ProbeSuccess, ProbeFailure> {
    if !executable.is_file() {
        return Err(ProbeFailure::Missing);
    }
    let executable = executable.canonicalize().map_err(|_| ProbeFailure::Spawn)?;
    let directory = tempfile::Builder::new()
        .prefix("homebase-sops-probe-")
        .tempdir()
        .map_err(|_| ProbeFailure::Setup)?;
    let root = directory.path().to_string_lossy().into_owned();
    let mut environment = Vec::new();
    for name in ["SystemRoot", "WINDIR", "PATH"] {
        if let Some(value) = std::env::var_os(name) {
            environment.push((name.into(), value.to_string_lossy().into_owned()));
        }
    }
    for name in [
        "HOME",
        "USERPROFILE",
        "APPDATA",
        "LOCALAPPDATA",
        "XDG_CONFIG_HOME",
        "TEMP",
        "TMP",
    ] {
        environment.push((name.into(), root.clone()));
    }
    environment.extend([
        ("SOPS_DISABLE_VERSION_CHECK".into(), "true".into()),
        ("HTTP_PROXY".into(), "http://127.0.0.1:1".into()),
        ("HTTPS_PROXY".into(), "http://127.0.0.1:1".into()),
    ]);
    let start = Instant::now();
    for (index, timeout) in [policy.first, policy.retry].into_iter().enumerate() {
        let result = spawn_with_timeout_env_input_remove(
            &executable.to_string_lossy(),
            ["--disable-version-check", "--version"],
            Some(directory.path()),
            &environment,
            &[],
            true,
            None,
            timeout,
        )
        .await;
        match result {
            Ok(output) if !output.is_success() => return Err(ProbeFailure::Exit(output.exit_code)),
            Ok(output) => {
                return parse_version(&output.stdout)
                    .map(|version| ProbeSuccess {
                        version,
                        attempts: index + 1,
                        elapsed_ms: start.elapsed().as_millis(),
                    })
                    .ok_or(ProbeFailure::InvalidOutput)
            }
            Err(AdapterError::Timeout { .. }) if index == 0 => continue,
            Err(AdapterError::Timeout { .. }) => {
                return Err(ProbeFailure::Timeout {
                    attempts: index + 1,
                    elapsed_ms: start.elapsed().as_millis(),
                })
            }
            Err(_) => return Err(ProbeFailure::Spawn),
        }
    }
    unreachable!()
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn accepts_actual_version_and_rejects_empty_or_unrelated_success_output() {
        assert_eq!(parse_version("sops 3.12.2\n").as_deref(), Some("3.12.2"));
        assert!(parse_version("sops 3.12.2-test+build").is_some());
        for output in [
            "",
            "hello",
            "sops 3",
            "sops 3.12",
            "sops x.1.2",
            "sops 3.12.2\nnot-a-version",
            "sops 1.2.3 script",
        ] {
            assert!(parse_version(output).is_none());
        }
    }
    #[tokio::test]
    async fn missing_file_fails_without_running_a_process() {
        let dir = tempfile::tempdir().unwrap();
        assert!(matches!(
            probe(&dir.path().join("absent.exe"), ProbePolicy::default()).await,
            Err(ProbeFailure::Missing)
        ));
        assert!(matches!(
            probe(dir.path(), ProbePolicy::default()).await,
            Err(ProbeFailure::Missing)
        ));
    }
}
