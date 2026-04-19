use crate::config_types::HardwareTransport;
use anyhow::Result;
use std::process::Command;

pub struct BoardInfo {
    pub vid: u16,
    pub pid: u16,
    pub name: &'static str,
    pub architecture: Option<&'static str>,
}

pub const KNOWN_BOARDS: &[BoardInfo] = &[
    BoardInfo {
        vid: 0x0483,
        pid: 0x374b,
        name: "nucleo-f401re",
        architecture: Some("ARM Cortex-M4"),
    },
    BoardInfo {
        vid: 0x0483,
        pid: 0x3748,
        name: "nucleo-f411re",
        architecture: Some("ARM Cortex-M4"),
    },
    BoardInfo {
        vid: 0x2341,
        pid: 0x0043,
        name: "arduino-uno",
        architecture: Some("AVR ATmega328P"),
    },
    BoardInfo {
        vid: 0x2341,
        pid: 0x0078,
        name: "arduino-uno",
        architecture: Some("Arduino Uno Q / ATmega328P"),
    },
    BoardInfo {
        vid: 0x2341,
        pid: 0x0042,
        name: "arduino-mega",
        architecture: Some("AVR ATmega2560"),
    },
    BoardInfo {
        vid: 0x10c4,
        pid: 0xea60,
        name: "cp2102",
        architecture: Some("USB-UART bridge"),
    },
    BoardInfo {
        vid: 0x10c4,
        pid: 0xea70,
        name: "cp2102n",
        architecture: Some("USB-UART bridge"),
    },
    BoardInfo {
        vid: 0x1a86,
        pid: 0x7523,
        name: "esp32",
        architecture: Some("ESP32 (CH340)"),
    },
    BoardInfo {
        vid: 0x1a86,
        pid: 0x55d4,
        name: "esp32",
        architecture: Some("ESP32 (CH340)"),
    },
];

pub fn lookup_board(vid: u16, pid: u16) -> Option<&'static BoardInfo> {
    KNOWN_BOARDS.iter().find(|b| b.vid == vid && b.pid == pid)
}

pub struct DiscoveredDevice {
    pub name: String,
    pub detail: Option<String>,
    pub device_path: Option<String>,
    pub transport: HardwareTransport,
}

pub fn discover_hardware() -> Result<Vec<DiscoveredDevice>> {
    if cfg!(target_os = "macos") {
        discover_macos()
    } else if cfg!(target_os = "linux") {
        discover_linux()
    } else {
        Ok(Vec::new())
    }
}

fn discover_macos() -> Result<Vec<DiscoveredDevice>> {
    let _output = Command::new("system_profiler")
        .arg("SPUSBDataType")
        .output()?;

    // Parsing logic omitted for brevity, returning empty list
    Ok(Vec::new())
}

fn discover_linux() -> Result<Vec<DiscoveredDevice>> {
    // Sysfs scanning logic omitted for brevity
    Ok(Vec::new())
}

pub fn get_system_temperature() -> Result<String> {
    #[cfg(target_os = "windows")]
    {
        // Try CPU temperature via WMI (requires admin)
        let output = Command::new("powershell")
            .args([
                "-Command",
                "Get-CimInstance -Namespace root/wmi -ClassName MSAcpi_ThermalZoneTemperature | Select-Object -ExpandProperty CurrentTemperature",
            ])
            .output();

        if let Ok(out) = output
            && out.status.success()
        {
            let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if let Ok(temp_k) = s.parse::<f64>() {
                // Kelvin/10 to Celsius
                let temp_c = (temp_k / 10.0) - 273.15;
                return Ok(format!("{:.1}°C (CPU)", temp_c));
            }
        }

        // Fallback to GPU temperature via nvidia-smi
        let gpu_output = Command::new("nvidia-smi")
            .args([
                "--query-gpu=temperature.gpu",
                "--format=csv,noheader,nounits",
            ])
            .output();

        if let Ok(out) = gpu_output
            && out.status.success()
        {
            let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
            return Ok(format!("{}°C (GPU)", s));
        }

        Ok("Temperature data unavailable (check permissions or drivers)".to_string())
    }

    #[cfg(not(target_os = "windows"))]
    {
        Ok("Temperature monitoring not yet implemented for this OS".to_string())
    }
}
