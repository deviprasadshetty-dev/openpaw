use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::Path;
use std::process::Stdio;
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::time::{Duration, timeout};

#[derive(Debug, Clone)]
pub struct RunResult {
    pub stdout: String,
    pub stderr: String,
    pub success: bool,
    pub exit_code: Option<i32>,
    pub timed_out: bool,
}

#[derive(Debug, Clone)]
pub struct RunOptions<'a> {
    pub cwd: Option<&'a Path>,
    pub env_clear: bool,
    pub env_vars: Option<&'a HashMap<String, String>>,
    pub max_output_bytes: usize,
    pub timeout_ms: u64,
}

impl Default for RunOptions<'_> {
    fn default() -> Self {
        Self {
            cwd: None,
            env_clear: false,
            env_vars: None,
            max_output_bytes: 1_048_576, // 1MB
            timeout_ms: 30_000,          // 30s default
        }
    }
}

pub async fn run(args: &[&str], opts: RunOptions<'_>) -> Result<RunResult> {
    if args.is_empty() {
        return Err(anyhow::anyhow!("No command provided"));
    }

    let mut cmd = Command::new(args[0]);
    cmd.args(&args[1..]);
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    if let Some(cwd) = opts.cwd {
        cmd.current_dir(cwd);
    }

    if opts.env_clear {
        cmd.env_clear();
    }

    if let Some(vars) = opts.env_vars {
        for (k, v) in vars {
            cmd.env(k, v);
        }
    }

    let mut child = cmd.spawn().context("Failed to spawn process")?;

    let stdout = child.stdout.take().context("Failed to take stdout")?;
    let stderr = child.stderr.take().context("Failed to take stderr")?;

    let max_output = opts.max_output_bytes;

    let stdout_handle = tokio::spawn(async move {
        let mut buf = Vec::new();
        let _ = stdout.take(max_output as u64).read_to_end(&mut buf).await;
        buf
    });

    let stderr_handle = tokio::spawn(async move {
        let mut buf = Vec::new();
        let _ = stderr.take(max_output as u64).read_to_end(&mut buf).await;
        buf
    });

    let exec_timeout = Duration::from_millis(opts.timeout_ms);
    let wait_res = timeout(exec_timeout, child.wait()).await;

    match wait_res {
        Ok(status_res) => {
            let status = status_res?;
            let stdout_bytes = stdout_handle.await.unwrap_or_default();
            let stderr_bytes = stderr_handle.await.unwrap_or_default();

            Ok(RunResult {
                stdout: String::from_utf8_lossy(&stdout_bytes).to_string(),
                stderr: String::from_utf8_lossy(&stderr_bytes).to_string(),
                success: status.success(),
                exit_code: status.code(),
                timed_out: false,
            })
        }
        Err(_) => {
            // Kill the process if it timed out
            let _ = child.kill().await;

            // Still try to get what we can from stdout/stderr, non-blocking if possible
            // but since they were moved to tasks, let's just abort or join with small timeout
            let stdout_bytes = stdout_handle.await.unwrap_or_default();
            let stderr_bytes = stderr_handle.await.unwrap_or_default();

            Ok(RunResult {
                stdout: String::from_utf8_lossy(&stdout_bytes).to_string(),
                stderr: String::from_utf8_lossy(&stderr_bytes).to_string(),
                success: false,
                exit_code: None,
                timed_out: true,
            })
        }
    }
}
