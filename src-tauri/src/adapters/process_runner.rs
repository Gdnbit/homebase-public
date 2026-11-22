use std::ffi::OsStr;
use std::time::{Duration, Instant};
use thiserror::Error;
use tokio::process::Command;
use tokio::time::timeout;

/// Adapter 层统一错误类型（跨 sops/git/powershell/agent_env）
#[derive(Error, Debug)]
pub enum AdapterError {
    #[error("failed to spawn `{program}`: {source}")]
    SpawnFailed {
        program: String,
        #[source]
        source: std::io::Error,
    },

    #[error("`{program}` exited with code {code}: {stderr}")]
    NonZeroExit {
        program: String,
        code: i32,
        stderr: String,
    },

    #[error("`{program}` timed out after {timeout_secs}s")]
    Timeout { program: String, timeout_secs: u64 },

    #[error("stdout from `{program}` is not valid UTF-8")]
    Utf8Decode {
        program: String,
        #[source]
        source: std::string::FromUtf8Error,
    },

    #[error("io error on `{program}`: {source}")]
    Io {
        program: String,
        #[source]
        source: std::io::Error,
    },
}

pub type AdapterResult<T> = Result<T, AdapterError>;

/// 结构化的子进程输出（所有 adapter 共用）
#[derive(Debug, Clone)]
pub struct CommandOutput {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
    pub duration: Duration,
}

impl CommandOutput {
    /// 判断是否成功（exit code = 0）
    pub fn is_success(&self) -> bool {
        self.exit_code == 0
    }

    /// 把 stdout 按行切分（trim CR/LF）
    pub fn stdout_lines(&self) -> impl Iterator<Item = &str> {
        self.stdout.lines()
    }
}

/// 默认子进程 timeout（30 秒）— 适配 sops/git/powershell 一次性调用
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// 启动子进程并等待输出；超时则 kill + 返回 `AdapterError::Timeout`
///
/// 参数：
/// - `program`: 可执行文件路径（如 `"git"` 或 `"C:\\HomeBase\\Tools\\sops\\bin\\sops.exe"`）
/// - `args`: 命令行参数
/// - `cwd`: 可选的工作目录
/// - `timeout_dur`: 超时时长
pub async fn spawn_with_timeout<S, I>(
    program: &str,
    args: I,
    cwd: Option<&std::path::Path>,
    timeout_dur: Duration,
) -> AdapterResult<CommandOutput>
where
    S: AsRef<OsStr>,
    I: IntoIterator<Item = S>,
{
    spawn_with_timeout_env(program, args, cwd, &[], timeout_dur).await
}

/// `spawn_with_timeout` with explicit child-only environment overrides.
///
/// This avoids mutating the long-running HomeBase process environment when a
/// CLI needs a path discovered after the app was started.
pub async fn spawn_with_timeout_env<S, I>(
    program: &str,
    args: I,
    cwd: Option<&std::path::Path>,
    env_overrides: &[(String, String)],
    timeout_dur: Duration,
) -> AdapterResult<CommandOutput>
where
    S: AsRef<OsStr>,
    I: IntoIterator<Item = S>,
{
    spawn_with_timeout_env_input(program, args, cwd, env_overrides, None, timeout_dur).await
}

pub async fn spawn_with_timeout_env_input<S, I>(
    program: &str,
    args: I,
    cwd: Option<&std::path::Path>,
    env_overrides: &[(String, String)],
    input: Option<&[u8]>,
    timeout_dur: Duration,
) -> AdapterResult<CommandOutput>
where
    S: AsRef<OsStr>,
    I: IntoIterator<Item = S>,
{
    spawn_with_timeout_env_input_remove(program, args, cwd, env_overrides, &[], false, input, timeout_dur).await
}

pub async fn spawn_with_timeout_env_input_remove<S, I>(
    program: &str,
    args: I,
    cwd: Option<&std::path::Path>,
    env_overrides: &[(String, String)],
    env_remove: &[&str],
    clear_env: bool,
    input: Option<&[u8]>,
    timeout_dur: Duration,
) -> AdapterResult<CommandOutput>
where
    S: AsRef<OsStr>,
    I: IntoIterator<Item = S>,
{
    use tokio::io::AsyncWriteExt;
    let mut cmd = Command::new(program);
    if clear_env { cmd.env_clear(); }
    cmd.args(args);
    cmd.envs(env_overrides.iter().map(|(key, value)| (key, value)));
    for name in env_remove { cmd.env_remove(name); }
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    // 不继承父进程 stdin（防 hang），stdout/stderr piped 捕获
    cmd.stdin(if input.is_some() { std::process::Stdio::piped() } else { std::process::Stdio::null() })
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    // Windows 下隐藏 console 窗口（CREATE_NO_WINDOW = 0x08000000）—
    // 否则 release GUI app 每次 spawn powershell.exe / sops.exe 子进程都会闪窗
    #[cfg(windows)]
    {
        cmd.creation_flags(0x08000000);
    }

    let start = Instant::now();
    let mut child = cmd.spawn().map_err(|e| AdapterError::SpawnFailed {
        program: program.to_string(),
        source: e,
    })?;

    #[cfg(windows)]
    let child_id = child.id();
    // Keep the child alive until timeout cleanup can terminate shell descendants.
    let stdin = child.stdin.take();
    let mut wait = Box::pin(async move {
        let write = async move {
            if let (Some(mut stdin), Some(bytes)) = (stdin, input) {
                stdin.write_all(bytes).await?;
                stdin.shutdown().await?;
            }
            Ok::<(), std::io::Error>(())
        };
        let (write_result, output) = tokio::join!(write, child.wait_with_output());
        write_result?;
        output
    });
    let output = match timeout(timeout_dur, &mut wait).await {
        Ok(Ok(o)) => o,
        Ok(Err(e)) => {
            return Err(AdapterError::Io {
                program: program.to_string(),
                source: e,
            });
        }
        Err(_elapsed) => {
            // Killing only cmd/powershell can leave descendants holding stdout open.
            #[cfg(windows)]
            if let Some(pid) = child_id {
                let mut cleanup = Command::new("taskkill.exe");
                cleanup
                    .args(["/PID", &pid.to_string(), "/T", "/F"])
                    .creation_flags(0x08000000)
                    .kill_on_drop(true);
                let _ = timeout(Duration::from_secs(3), cleanup.output()).await;
            }
            // Drop still terminates the direct child if tree cleanup failed.
            return Err(AdapterError::Timeout {
                program: program.to_string(),
                timeout_secs: timeout_dur.as_secs(),
            });
        }
    };

    let duration = start.elapsed();
    let stdout = String::from_utf8(output.stdout).map_err(|e| AdapterError::Utf8Decode {
        program: program.to_string(),
        source: e,
    })?;
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    let exit_code = output.status.code().unwrap_or(-1);

    Ok(CommandOutput {
        stdout,
        stderr,
        exit_code,
        duration,
    })
}

/// 便捷包装：spawn_with_timeout + 非零 exit code 自动转 `AdapterError::NonZeroExit`
pub async fn run_checked<S, I>(
    program: &str,
    args: I,
    cwd: Option<&std::path::Path>,
    timeout_dur: Duration,
) -> AdapterResult<CommandOutput>
where
    S: AsRef<OsStr>,
    I: IntoIterator<Item = S>,
{
    let out = spawn_with_timeout(program, args, cwd, timeout_dur).await?;
    if !out.is_success() {
        return Err(AdapterError::NonZeroExit {
            program: program.to_string(),
            code: out.exit_code,
            stderr: out.stderr,
        });
    }
    Ok(out)
}

/// `run_checked` with environment overrides applied only to the child process.
pub async fn run_checked_with_env<S, I>(
    program: &str,
    args: I,
    cwd: Option<&std::path::Path>,
    env_overrides: &[(String, String)],
    timeout_dur: Duration,
) -> AdapterResult<CommandOutput>
where
    S: AsRef<OsStr>,
    I: IntoIterator<Item = S>,
{
    let out = spawn_with_timeout_env(program, args, cwd, env_overrides, timeout_dur).await?;
    if !out.is_success() {
        return Err(AdapterError::NonZeroExit {
            program: program.to_string(),
            code: out.exit_code,
            stderr: out.stderr,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn spawn_captures_stdout() {
        // cmd /c echo hello — 简单到不能再简单的子进程测试
        let out = spawn_with_timeout("cmd", ["/c", "echo hello"], None, Duration::from_secs(5))
            .await
            .expect("cmd echo should spawn");
        assert!(out.is_success(), "exit 0 expected, got {}", out.exit_code);
        assert!(
            out.stdout.contains("hello"),
            "stdout should contain 'hello': {:?}",
            out.stdout
        );
    }

    #[tokio::test]
    async fn spawn_fail_on_missing_program() {
        let res = spawn_with_timeout(
            "definitely-does-not-exist-xyz-12345",
            ["--help"],
            None,
            Duration::from_secs(2),
        )
        .await;
        assert!(matches!(res, Err(AdapterError::SpawnFailed { .. })));
    }

    #[tokio::test]
    async fn run_checked_fails_on_nonzero_exit() {
        // cmd /c exit 1
        let res = run_checked("cmd", ["/c", "exit 1"], None, Duration::from_secs(5)).await;
        assert!(matches!(
            res,
            Err(AdapterError::NonZeroExit { code: 1, .. })
        ));
    }

    #[tokio::test]
    async fn child_environment_override_is_scoped_to_command() {
        let out = spawn_with_timeout_env(
            "cmd",
            ["/c", "echo", "%HOMEBASE_CHILD_ENV_TEST%"],
            None,
            &[(
                "HOMEBASE_CHILD_ENV_TEST".to_string(),
                "isolated".to_string(),
            )],
            Duration::from_secs(5),
        )
        .await
        .expect("cmd echo should receive override");
        assert!(out.stdout.contains("isolated"));
        assert!(std::env::var("HOMEBASE_CHILD_ENV_TEST").is_err());
    }

    #[tokio::test]
    async fn spawn_times_out() {
        // ping 自己 100 次 — 足够触发 2 秒 timeout
        let res = spawn_with_timeout(
            "cmd",
            ["/c", "ping", "127.0.0.1", "-n", "100"],
            None,
            Duration::from_millis(500),
        )
        .await;
        assert!(matches!(res, Err(AdapterError::Timeout { .. })));
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn timeout_terminates_child_before_it_can_write() {
        let temp = tempfile::tempdir().unwrap();
        let marker = temp.path().join("should-not-exist.txt");
        let res = spawn_with_timeout_env(
            "powershell.exe",
            ["-NoProfile", "-NonInteractive", "-Command",
                "Start-Sleep -Seconds 4; [IO.File]::WriteAllText($env:HOMEBASE_TIMEOUT_MARKER, 'leaked')"],
            None,
            &[("HOMEBASE_TIMEOUT_MARKER".to_string(), marker.to_string_lossy().into_owned())],
            Duration::from_secs(2),
        ).await;
        assert!(matches!(res, Err(AdapterError::Timeout { .. })));
        tokio::time::sleep(Duration::from_secs(4)).await;
        assert!(
            !marker.exists(),
            "timed-out child must not continue executing"
        );
    }
}
