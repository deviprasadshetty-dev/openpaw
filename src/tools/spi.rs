use super::{Tool, ToolContext, ToolResult};
use anyhow::Result;
use serde_json::Value;

pub struct SpiTool {}

impl Tool for SpiTool {
    fn name(&self) -> &str {
        "spi"
    }

    fn description(&self) -> &str {
        "Interact with SPI hardware devices. Supports listing available SPI devices, full-duplex data transfer, and read-only mode. Linux only — uses /dev/spidevX.Y via ioctl."
    }

    fn parameters_json(&self) -> String {
        r#"{"type":"object","properties":{"action":{"type":"string","description":"Action: list, transfer, or read"},"device":{"type":"string","description":"SPI device path (default /dev/spidev0.0)"},"data":{"type":"string","description":"Hex bytes to send, e.g. 'FF 0A 3B'"},"speed_hz":{"type":"integer","description":"SPI clock speed in Hz (default 1000000)"},"mode":{"type":"integer","description":"SPI mode 0-3 (default 0)"},"bits_per_word":{"type":"integer","description":"Bits per word (default 8)"}},"required":["action"]}"#.to_string()
    }

    fn execute(&self, args: Value, _context: &ToolContext) -> Result<ToolResult> {
        let action = match args.get("action").and_then(|v| v.as_str()) {
            Some(a) => a,
            None => return Ok(ToolResult::fail("Missing 'action' parameter")),
        };

        if action == "list" || action == "transfer" || action == "read" {
            #[cfg(target_os = "linux")]
            {
                return Ok(ToolResult::fail(
                    "SPI logic not fully implemented in Rust port yet",
                ));
            }
            #[cfg(not(target_os = "linux"))]
            {
                return Ok(ToolResult::fail("SPI not supported on this platform"));
            }
        }

        Ok(ToolResult::fail(
            "Unknown action. Use 'list', 'transfer', or 'read'",
        ))
    }
}
