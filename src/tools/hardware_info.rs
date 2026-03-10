use super::{Tool, ToolContext, ToolResult};
use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;

pub struct HardwareInfoTool {}

#[async_trait]
impl Tool for HardwareInfoTool {
    fn name(&self) -> &str {
        "hardware_info"
    }

    fn description(&self) -> &str {
        "Get information about the system's hardware (CPU, Hostname, etc.)"
    }

    fn parameters_json(&self) -> String {
        r#"{"type":"object","properties":{}}"#.to_string()
    }

    async fn execute(&self, _args: Value, _context: &ToolContext) -> Result<ToolResult> {
        use sysinfo::System;
        let mut sys = System::new_all();
        sys.refresh_all();

        let mut output = String::new();
        output.push_str(&format!("System Name: {:?}\n", System::name()));
        output.push_str(&format!("OS Version: {:?}\n", System::os_version()));
        output.push_str(&format!("Host Name: {:?}\n", System::host_name()));
        output.push_str(&format!("CPU Count: {}\n", sys.cpus().len()));
        output.push_str(&format!("Total Memory: {} KB\n", sys.total_memory()));
        output.push_str(&format!("Used Memory: {} KB\n", sys.used_memory()));

        Ok(ToolResult::ok(output))
    }
}
