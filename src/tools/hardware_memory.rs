use super::{Tool, ToolContext, ToolResult};
use crate::tools::process_util::{self, RunOptions};
use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;

const NUCLEO_RAM_BASE: u64 = 0x2000_0000;
const MAX_PROBE_OUTPUT: usize = 65_536;

pub struct HardwareMemoryTool {
    pub boards: Vec<String>,
}

#[async_trait]
impl Tool for HardwareMemoryTool {
    fn name(&self) -> &str {
        "hardware_memory"
    }

    fn description(&self) -> &str {
        "Read/write hardware memory maps via probe-rs or serial. Use for: 'read memory', 'read register', 'dump memory', 'write memory'. Params: action (read/write), address (hex), length (bytes), value (for write)."
    }

    fn parameters_json(&self) -> String {
        r#"{"type":"object","properties":{"action":{"type":"string","enum":["read","write"],"description":"read or write memory"},"address":{"type":"string","description":"Memory address in hex (e.g. 0x20000000)"},"length":{"type":"integer","description":"Bytes to read (default 128, max 256)"},"value":{"type":"string","description":"Hex value to write (for write action)"},"board":{"type":"string","description":"Board name (optional if only one configured)"}},"required":["action"]}"#.to_string()
    }

    async fn execute(&self, args: Value, _context: &ToolContext) -> Result<ToolResult> {
        if self.boards.is_empty() {
            return Ok(ToolResult::fail(
                "No peripherals configured. Add boards to config.toml [peripherals.boards].",
            ));
        }

        let action = match args.get("action").and_then(|v| v.as_str()) {
            Some(a) => a,
            None => {
                return Ok(ToolResult::fail(
                    "Missing 'action' parameter (read or write)",
                ));
            }
        };

        let board =
            args.get("board")
                .and_then(|v| v.as_str())
                .unwrap_or(if !self.boards.is_empty() {
                    &self.boards[0]
                } else {
                    "unknown"
                });

        let chip = match chip_for_board(board) {
            Some(c) => c,
            None => {
                return Ok(ToolResult::fail(format!(
                    "Memory operations only support nucleo-f401re, nucleo-f411re. Got: {}",
                    board
                )));
            }
        };

        let address_str = args
            .get("address")
            .and_then(|v| v.as_str())
            .unwrap_or("0x20000000");
        let address = parse_hex_address(address_str).unwrap_or(NUCLEO_RAM_BASE);

        if action == "read" {
            let length_raw = args.get("length").and_then(|v| v.as_i64()).unwrap_or(128);
            let length = length_raw.clamp(1, 256) as usize;
            return probe_read(chip, address, length).await;
        } else if action == "write" {
            let value = match args.get("value").and_then(|v| v.as_str()) {
                Some(v) => v,
                None => {
                    return Ok(ToolResult::fail(
                        "Missing 'value' parameter for write action",
                    ));
                }
            };
            return probe_write(chip, address, value).await;
        } else {
            return Ok(ToolResult::fail(format!(
                "Unknown action '{}'. Use 'read' or 'write'.",
                action
            )));
        }
    }
}

async fn probe_rs_available() -> bool {
    let opts = RunOptions {
        max_output_bytes: 4096,
        ..Default::default()
    };
    if let Ok(result) = process_util::run(&["probe-rs", "--version"], opts).await {
        result.success
    } else {
        false
    }
}

async fn probe_read(chip: &str, address: u64, length: usize) -> Result<ToolResult> {
    if !probe_rs_available().await {
        return Ok(ToolResult::fail(
            "probe-rs not found. Install with: cargo install probe-rs-tools",
        ));
    }

    let addr_str = format!("0x{:0>8X}", address);
    let len_str = length.to_string();

    let opts = RunOptions {
        max_output_bytes: MAX_PROBE_OUTPUT,
        ..Default::default()
    };

    let result = match process_util::run(
        &["probe-rs", "read", "--chip", chip, &addr_str, &len_str],
        opts,
    )
    .await
    {
        Ok(res) => res,
        Err(_) => return Ok(ToolResult::fail("Failed to spawn probe-rs read command")),
    };

    if result.success {
        if !result.stdout.is_empty() {
            return Ok(ToolResult::ok(result.stdout));
        }
        return Ok(ToolResult::ok("(no output from probe-rs)"));
    }

    if let Some(code) = result.exit_code {
        let err_msg = format!(
            "probe-rs read failed (exit {}): {}",
            code,
            if !result.stderr.is_empty() {
                &result.stderr
            } else {
                "unknown error"
            }
        );
        return Ok(ToolResult::fail(err_msg));
    }
    Ok(ToolResult::fail("probe-rs read terminated by signal"))
}

async fn probe_write(chip: &str, address: u64, value: &str) -> Result<ToolResult> {
    if !probe_rs_available().await {
        return Ok(ToolResult::fail(
            "probe-rs not found. Install with: cargo install probe-rs-tools",
        ));
    }

    let addr_str = format!("0x{:0>8X}", address);

    let opts = RunOptions {
        max_output_bytes: MAX_PROBE_OUTPUT,
        ..Default::default()
    };

    let result = match process_util::run(
        &["probe-rs", "write", "--chip", chip, &addr_str, value],
        opts,
    )
    .await
    {
        Ok(res) => res,
        Err(_) => return Ok(ToolResult::fail("Failed to spawn probe-rs write command")),
    };

    if result.success {
        let out = format!(
            "Write OK: 0x{:0>8X} <- {} ({}){}",
            address, value, chip, result.stdout
        );
        return Ok(ToolResult::ok(out));
    }

    if let Some(code) = result.exit_code {
        let err_msg = format!(
            "probe-rs write failed (exit {}): {}",
            code,
            if !result.stderr.is_empty() {
                &result.stderr
            } else {
                "unknown error"
            }
        );
        return Ok(ToolResult::fail(err_msg));
    }
    Ok(ToolResult::fail("probe-rs write terminated by signal"))
}

fn chip_for_board(board: &str) -> Option<&'static str> {
    match board {
        "nucleo-f401re" => Some("STM32F401RETx"),
        "nucleo-f411re" => Some("STM32F411RETx"),
        _ => None,
    }
}

fn parse_hex_address(s: &str) -> Option<u64> {
    let trimmed = if s.starts_with("0x") || s.starts_with("0X") {
        &s[2..]
    } else {
        s
    };
    u64::from_str_radix(trimmed, 16).ok()
}
