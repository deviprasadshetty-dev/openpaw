use anyhow::{Context, Result};
use std::collections::HashMap;
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};

pub struct RunResult {
    pub stdout: String,
    pub stderr: String,
    pub success: bool,
    pub exit_code: Option<i32>,
}

pub struct RunOptions<'a> {
    pub cwd: Option<&'a Path>,
    pub env_clear: bool,
    pub env_vars: Option<&'a HashMap<String, String>>,
    pub max_output_bytes: usize,
}

impl Default for RunOptions<'_> {
    fn default() -> Self {
        Self {
            cwd: None,
            env_clear: false,
            env_vars: None,
            max_output_bytes: 1_048_576,
        }
    }
}

pub fn run(args: &[&str], opts: RunOptions) -> Result<RunResult> {
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

    let mut stdout_bytes = Vec::new();
    let mut stderr_bytes = Vec::new();

    if let Some(mut stdout) = child.stdout.take() {
        // Read up to limit
        let mut handle = stdout.take(opts.max_output_bytes as u64);
        handle.read_to_end(&mut stdout_bytes)?;
    }

    if let Some(mut stderr) = child.stderr.take() {
        let mut handle = stderr.take(opts.max_output_bytes as u64);
        handle.read_to_end(&mut stderr_bytes)?;
    }

    let status = child.wait()?;
    let success = status.success();
    let exit_code = status.code();

    let stdout_str = String::from_utf8_lossy(&stdout_bytes).to_string();
    let stderr_str = String::from_utf8_lossy(&stderr_bytes).to_string();

    Ok(RunResult {
        stdout: stdout_str,
        stderr: stderr_str,
        success,
        exit_code,
    })
}
