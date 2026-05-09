use super::{Tool, ToolContext, ToolResult};
use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;

pub struct I2cTool {}

#[async_trait]
impl Tool for I2cTool {
    fn name(&self) -> &str {
        "i2c"
    }

    fn description(&self) -> &str {
        "I2C hardware tool. Actions: detect (list buses), scan (find devices on bus), read (read register bytes), write (write register byte). Linux only — requires i2c-tools (i2cdetect, i2cget, i2cset)."
    }

    fn parameters_json(&self) -> String {
        r#"{"type":"object","properties":{"action":{"type":"string","description":"Action: detect, scan, read, write"},"bus":{"type":"integer","description":"I2C bus number (e.g. 1 for /dev/i2c-1)"},"address":{"type":"string","description":"Device address in hex (e.g. 0x48)"},"register":{"type":"integer","description":"Register number to read/write"},"value":{"type":"integer","description":"Byte value to write (0-255)"},"length":{"type":"integer","description":"Number of bytes to read (default 1)"}},"required":["action"]}"#.to_string()
    }

    async fn execute(&self, args: Value, _context: &ToolContext) -> Result<ToolResult> {
        #[cfg(not(target_os = "linux"))]
        {
            let _ = args;
            return Ok(ToolResult::fail("I2C is only supported on Linux."));
        }

        #[cfg(target_os = "linux")]
        execute_linux(args).await
    }
}

#[cfg(target_os = "linux")]
async fn execute_linux(args: Value) -> Result<ToolResult> {
    use tokio::process::Command;

    let action = match args.get("action").and_then(|v| v.as_str()) {
        Some(a) => a.to_lowercase(),
        None => return Ok(ToolResult::fail("Missing 'action' parameter")),
    };

    match action.as_str() {
        "detect" => {
            // List all available I2C buses
            let out = run("i2cdetect", &["-l"]).await?;
            Ok(ToolResult::ok(out))
        }

        "scan" => {
            let bus = match args.get("bus").and_then(|v| v.as_u64()) {
                Some(b) => b.to_string(),
                None => return Ok(ToolResult::fail("Missing 'bus' parameter for scan")),
            };
            // -y suppresses the interactive prompt
            let out = run("i2cdetect", &["-y", &bus]).await?;
            Ok(ToolResult::ok(out))
        }

        "read" => {
            let bus = match args.get("bus").and_then(|v| v.as_u64()) {
                Some(b) => b.to_string(),
                None => return Ok(ToolResult::fail("Missing 'bus' parameter")),
            };
            let address = match args.get("address").and_then(|v| v.as_str()) {
                Some(a) => a.to_string(),
                None => return Ok(ToolResult::fail("Missing 'address' parameter")),
            };
            let length = args.get("length").and_then(|v| v.as_u64()).unwrap_or(1);

            if let Some(reg) = args.get("register").and_then(|v| v.as_u64()) {
                // Read a specific register, potentially multiple bytes
                if length == 1 {
                    let reg_str = format!("0x{:02x}", reg);
                    let out = run("i2cget", &["-y", &bus, &address, &reg_str]).await?;
                    Ok(ToolResult::ok(out))
                } else {
                    // Use i2cdump for multi-byte reads from a base register
                    let reg_str = format!("0x{:02x}", reg);
                    let out = run("i2cdump", &["-y", &bus, &address, "b"]).await?;
                    // Filter output to just the relevant range for clarity
                    let filtered = filter_i2cdump(&out, reg as u8, length as u8);
                    Ok(ToolResult::ok(filtered))
                }
            } else {
                // No register: dump the whole device
                let out = run("i2cdump", &["-y", &bus, &address, "b"]).await?;
                Ok(ToolResult::ok(out))
            }
        }

        "write" => {
            let bus = match args.get("bus").and_then(|v| v.as_u64()) {
                Some(b) => b.to_string(),
                None => return Ok(ToolResult::fail("Missing 'bus' parameter")),
            };
            let address = match args.get("address").and_then(|v| v.as_str()) {
                Some(a) => a.to_string(),
                None => return Ok(ToolResult::fail("Missing 'address' parameter")),
            };
            let register = match args.get("register").and_then(|v| v.as_u64()) {
                Some(r) => format!("0x{:02x}", r),
                None => return Ok(ToolResult::fail("Missing 'register' parameter")),
            };
            let value = match args.get("value").and_then(|v| v.as_u64()) {
                Some(v) if v <= 255 => format!("0x{:02x}", v),
                Some(_) => return Ok(ToolResult::fail("'value' must be 0-255")),
                None => return Ok(ToolResult::fail("Missing 'value' parameter")),
            };
            let out = run("i2cset", &["-y", &bus, &address, &register, &value]).await?;
            Ok(ToolResult::ok(if out.is_empty() {
                format!(
                    "Written {} to register {} on device {} (bus {})",
                    value, register, address, bus
                )
            } else {
                out
            }))
        }

        _ => Ok(ToolResult::fail(
            "Unknown action. Use: detect, scan, read, write",
        )),
    }
}

#[cfg(target_os = "linux")]
async fn run(bin: &str, args: &[&str]) -> Result<String> {
    use tokio::process::Command;
    let out = Command::new(bin).args(args).output().await.map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            anyhow::anyhow!(
                "'{}' not found. Install i2c-tools: sudo apt install i2c-tools",
                bin
            )
        } else {
            anyhow::anyhow!("Failed to run {}: {}", bin, e)
        }
    })?;

    let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();

    if out.status.success() {
        Ok(stdout)
    } else {
        Err(anyhow::anyhow!(
            "{} failed (exit {}): {}",
            bin,
            out.status.code().unwrap_or(-1),
            if stderr.is_empty() { stdout } else { stderr }
        ))
    }
}

/// Extract only the rows covering [start_reg, start_reg+length) from i2cdump output.
#[cfg(target_os = "linux")]
fn filter_i2cdump(dump: &str, start_reg: u8, length: u8) -> String {
    let end_reg = start_reg.saturating_add(length).saturating_sub(1);
    let start_row = start_reg & 0xF0;
    let end_row = end_reg & 0xF0;

    let mut out = String::new();
    for line in dump.lines() {
        // i2cdump rows look like: "00: 00 01 02 03 ..."
        if let Some(colon) = line.find(':') {
            if let Ok(row_addr) = u8::from_str_radix(line[..colon].trim(), 16) {
                if row_addr >= start_row && row_addr <= end_row {
                    out.push_str(line);
                    out.push('\n');
                }
            }
        }
    }
    if out.is_empty() {
        dump.to_string()
    } else {
        out
    }
}
