//! agent-env.ps1 适配器 — 启动 agent 子进程（v3 §10.3 启动契约）
//!
//! 命令模板：`powershell -NoProfile -File C:\HomeBase\Tools\bin\agent-env.ps1 <agent> [extra_args]`
//!
//! 与其他 adapter 不同：**不 timeout**（agent 是 long-running process），
//! 返回 `AgentHandle` 让 caller 持有 pid 和 stdout/stderr channel；
//! LogCollector worker（v3 §3.1）消费 channel 把输出转发给前端。

use super::{AdapterError, AdapterResult};
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::mpsc;

const PS_EXE: &str = "powershell.exe";
const DEFAULT_SCRIPT: &str = "C:\\HomeBase\\Tools\\bin\\agent-env.ps1";
/// stdout/stderr channel buffer 大小（超过则 writer 阻塞）
const CHANNEL_BUFFER: usize = 256;

/// agent-env.ps1 适配器
pub struct AgentEnvAdapter {
    script_path: String,
    ps_exe: String,
}

impl AgentEnvAdapter {
    pub fn new() -> Self {
        Self {
            script_path: DEFAULT_SCRIPT.to_string(),
            ps_exe: PS_EXE.to_string(),
        }
    }

    /// 自定义 script 路径（测试用 mock script）
    pub fn with_script(script_path: impl Into<String>) -> Self {
        Self {
            script_path: script_path.into(),
            ps_exe: PS_EXE.to_string(),
        }
    }

    /// 启动 agent 子进程；返回 AgentHandle（含 pid + stdout/stderr 行 channel + kill 能力）
    pub async fn launch(&self, agent: &str, extra_args: &[&str]) -> AdapterResult<AgentHandle> {
        self.launch_with_program(&self.ps_exe, agent, extra_args).await
    }

    /// 内部：用任意 program 启动（测试时可传 cmd.exe 等跳过 PowerShell）
    async fn launch_with_program(
        &self,
        program: &str,
        agent: &str,
        extra_args: &[&str],
    ) -> AdapterResult<AgentHandle> {
        let mut cmd = Command::new(program);
        cmd.arg("-NoProfile")
            .arg("-NonInteractive")
            .arg("-File")
            .arg(&self.script_path)
            .arg(agent);

        for a in extra_args {
            cmd.arg(a);
        }

        cmd.stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        // 隐藏 powershell.exe 黑窗
        #[cfg(windows)]
        {
            cmd.creation_flags(0x08000000);
        }

        let mut child = cmd.spawn().map_err(|e| AdapterError::SpawnFailed {
            program: program.to_string(),
            source: e,
        })?;

        let pid = child.id().unwrap_or(0);
        let stdout = child.stdout.take().expect("stdout piped");
        let stderr = child.stderr.take().expect("stderr piped");

        // stdout 行 → channel
        let (stdout_tx, stdout_rx) = mpsc::channel::<String>(CHANNEL_BUFFER);
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if stdout_tx.send(line).await.is_err() {
                    break; // receiver dropped
                }
            }
        });

        // stderr 行 → channel
        let (stderr_tx, stderr_rx) = mpsc::channel::<String>(CHANNEL_BUFFER);
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if stderr_tx.send(line).await.is_err() {
                    break;
                }
            }
        });

        Ok(AgentHandle {
            pid,
            stdout_rx,
            stderr_rx,
            child,
        })
    }
}

impl Default for AgentEnvAdapter {
    fn default() -> Self {
        Self::new()
    }
}

/// 一个已启动的 agent 子进程句柄
pub struct AgentHandle {
    /// OS 进程 ID（0 表示 spawn 后已退出，PID 不可用）
    pub pid: u32,
    /// stdout 按行切分的 channel
    pub stdout_rx: mpsc::Receiver<String>,
    /// stderr 按行切分的 channel
    pub stderr_rx: mpsc::Receiver<String>,
    child: Child,
}

impl AgentHandle {
    /// 发送 kill 信号 + 等子进程退出
    pub async fn kill(mut self) -> AdapterResult<()> {
        self.child.kill().await.map_err(|e| AdapterError::Io {
            program: "agent_env".to_string(),
            source: e,
        })?;
        Ok(())
    }

    /// 等子进程自然退出（返回 exit code）
    pub async fn wait(mut self) -> AdapterResult<i32> {
        let status = self.child.wait().await.map_err(|e| AdapterError::Io {
            program: "agent_env".to_string(),
            source: e,
        })?;
        Ok(status.code().unwrap_or(-1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::time::Duration;
    use tokio::time::timeout;

    /// 写一个临时 PowerShell 脚本当 mock agent-env.ps1
    /// `unique_id` 必须每个 test 不同，否则并行测试会覆盖同名文件导致 flaky
    fn write_mock_script(unique_id: &str, content: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir();
        let path = dir.join(format!(
            "homebase-mock-{}-{}.ps1",
            std::process::id(),
            unique_id
        ));
        let mut f = std::fs::File::create(&path).expect("create mock script");
        f.write_all(content.as_bytes()).expect("write mock script");
        path
    }

    #[tokio::test]
    async fn launch_captures_stdout_lines() {
        // Mock script 输出 3 行 + 正常退出
        let script_content = r#"
param([string]$Agent)
Write-Output "line-1 from $Agent"
Write-Output "line-2"
Write-Output "line-3"
"#;
        let script_path = write_mock_script("stdout", script_content);
        let adapter = AgentEnvAdapter::with_script(script_path.to_string_lossy().to_string());

        let mut handle = adapter.launch("test-agent", &[]).await.expect("launch");
        assert!(handle.pid > 0, "pid should be nonzero");

        // 收 3 行 stdout（2 秒内）
        let mut lines = Vec::new();
        let collect = async {
            while let Some(line) = handle.stdout_rx.recv().await {
                lines.push(line);
                if lines.len() >= 3 {
                    break;
                }
            }
        };
        timeout(Duration::from_secs(10), collect)
            .await
            .expect("collect stdout within 10s");

        assert!(lines.len() >= 3, "expected >=3 lines, got {}: {:?}", lines.len(), lines);
        assert!(lines[0].contains("test-agent"), "first line should contain agent name: {:?}", lines[0]);
        assert!(lines.iter().any(|l| l.contains("line-2")), "should see line-2: {:?}", lines);

        let _ = std::fs::remove_file(&script_path);
    }

    #[tokio::test]
    async fn launch_captures_stderr() {
        let script_content = r#"
param([string]$Agent)
[Console]::Error.WriteLine("error-msg-1")
[Console]::Error.WriteLine("error-msg-2")
"#;
        let script_path = write_mock_script("stderr", script_content);
        let adapter = AgentEnvAdapter::with_script(script_path.to_string_lossy().to_string());

        let mut handle = adapter.launch("any", &[]).await.expect("launch");

        let mut errors = Vec::new();
        let collect = async {
            while let Some(line) = handle.stderr_rx.recv().await {
                errors.push(line);
                if errors.len() >= 2 {
                    break;
                }
            }
        };
        timeout(Duration::from_secs(10), collect)
            .await
            .expect("collect stderr within 10s");

        assert!(errors.iter().any(|e| e.contains("error-msg-1")), "stderr: {errors:?}");
        let _ = std::fs::remove_file(&script_path);
    }

    #[tokio::test]
    async fn kill_terminates_long_running() {
        // Sleep 60 秒的 script — 测 kill
        let script_content = r#"Start-Sleep -Seconds 60"#;
        let script_path = write_mock_script("kill", script_content);
        let adapter = AgentEnvAdapter::with_script(script_path.to_string_lossy().to_string());

        let handle = adapter.launch("sleeper", &[]).await.expect("launch");
        let pid = handle.pid;
        assert!(pid > 0);

        // kill 应该立即返回（不等 60 秒）
        let kill_result = timeout(Duration::from_secs(5), handle.kill()).await;
        assert!(kill_result.is_ok(), "kill should return within 5s");
        assert!(kill_result.unwrap().is_ok(), "kill should succeed");

        let _ = std::fs::remove_file(&script_path);
    }

    #[tokio::test]
    async fn spawn_fail_on_missing_script_surfaces_error() {
        // 不存在的 script 会导致 powershell 返回 non-zero 但 spawn 本身不会 fail
        // 所以这个测试改为验证 spawn + child exits with non-zero
        let adapter = AgentEnvAdapter::with_script(r"C:\definitely-does-not-exist-xyz.ps1");
        let handle = adapter.launch("none", &[]).await.expect("spawn ok");
        let exit = timeout(Duration::from_secs(10), handle.wait())
            .await
            .expect("wait within 10s")
            .expect("wait ok");
        assert_ne!(exit, 0, "missing script should cause nonzero exit, got {exit}");
    }
}
