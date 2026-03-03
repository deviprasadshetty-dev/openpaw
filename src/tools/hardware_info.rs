use super::{Tool, ToolResult};
use anyhow::Result;
use serde_json::Value;

struct BoardInfo {
    id: &'static str,
    chip: &'static str,
    desc: &'static str,
}

const BOARD_DB: &[BoardInfo] = &[
    BoardInfo {
        id: "nucleo-f401re",
        chip: "STM32F401RET6",
        desc: "ARM Cortex-M4, 84 MHz. Flash: 512 KB, RAM: 128 KB. User LED on PA5 (pin 13).",
    },
    BoardInfo {
        id: "nucleo-f411re",
        chip: "STM32F411RET6",
        desc: "ARM Cortex-M4, 100 MHz. Flash: 512 KB, RAM: 128 KB. User LED on PA5 (pin 13).",
    },
    BoardInfo {
        id: "arduino-uno",
        chip: "ATmega328P",
        desc: "8-bit AVR, 16 MHz. Flash: 16 KB, SRAM: 2 KB. Built-in LED on pin 13.",
    },
    BoardInfo {
        id: "arduino-uno-q",
        chip: "STM32U585 + Qualcomm",
        desc: "Dual-core: STM32 (MCU) + Linux (aarch64). GPIO via Bridge app on port 9999.",
    },
    BoardInfo {
        id: "esp32",
        chip: "ESP32",
        desc: "Dual-core Xtensa LX6, 240 MHz. Flash: 4 MB typical. Built-in LED on GPIO 2.",
    },
    BoardInfo {
        id: "rpi-gpio",
        chip: "Raspberry Pi",
        desc: "ARM Linux. Native GPIO via sysfs/rppal. No fixed LED pin.",
    },
];

pub struct HardwareBoardInfoTool {
    pub boards: Vec<String>,
}

impl Tool for HardwareBoardInfoTool {
    fn name(&self) -> &str {
        "hardware_board_info"
    }

    fn description(&self) -> &str {
        "Return board info (chip, architecture, memory map) for connected hardware. Use for: 'board info', 'what board', 'connected hardware', 'chip info', 'memory map'."
    }

    fn parameters_json(&self) -> String {
        r#"{"type":"object","properties":{"board":{"type":"string","description":"Board name (e.g. nucleo-f401re). If omitted, returns info for first configured board."}}}"#.to_string()
    }

    fn execute(&self, args: Value) -> Result<ToolResult> {
        if self.boards.is_empty() {
            return Ok(ToolResult::fail(
                "No peripherals configured. Add boards to config.toml [peripherals.boards].",
            ));
        }

        let board =
            args.get("board")
                .and_then(|v| v.as_str())
                .unwrap_or(if !self.boards.is_empty() {
                    &self.boards[0]
                } else {
                    "unknown"
                });

        for entry in BOARD_DB {
            if entry.id == board {
                let mut output = format!(
                    "**Board:** {}\n**Chip:** {}\n**Description:** {}",
                    board, entry.chip, entry.desc
                );

                if let Some(mem) = memory_map_static(board) {
                    output.push_str("\n\n**Memory map:**\n");
                    output.push_str(mem);
                }

                return Ok(ToolResult::ok(output));
            }
        }

        let msg = format!("Board '{}' configured. No static info available.", board);
        Ok(ToolResult::ok(msg))
    }
}

fn memory_map_static(board: &str) -> Option<&'static str> {
    if board == "nucleo-f401re" || board == "nucleo-f411re" {
        return Some(
            "Flash: 0x0800_0000 - 0x0807_FFFF (512 KB)\nRAM: 0x2000_0000 - 0x2001_FFFF (128 KB)",
        );
    }
    if board == "arduino-uno" {
        return Some("Flash: 16 KB, SRAM: 2 KB, EEPROM: 1 KB");
    }
    if board == "esp32" {
        return Some("Flash: 4 MB, IRAM/DRAM per ESP-IDF layout");
    }
    None
}
