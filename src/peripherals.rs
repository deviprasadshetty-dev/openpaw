use anyhow::{Result, anyhow};

#[derive(Debug, Clone, Default)]
pub struct PeripheralCapabilities {
    pub board_name: String,
    pub board_type: String,
    pub gpio_pins: String,
    pub flash_size_kb: u32,
    pub has_serial: bool,
    pub has_gpio: bool,
    pub has_flash: bool,
    pub has_adc: bool,
}

pub trait Peripheral: Send + Sync {
    fn name(&self) -> String;
    fn board_type(&self) -> String;
    fn health_check(&self) -> bool;
    fn init_peripheral(&mut self) -> Result<()>;
    fn read(&mut self, addr: u32) -> Result<u8>;
    fn write_byte(&mut self, addr: u32, data: u8) -> Result<()>;
    fn flash_firmware(&mut self, firmware_path: &str) -> Result<()>;
    fn get_capabilities(&self) -> PeripheralCapabilities;
}

pub struct SerialPeripheral {
    peripheral_name: String,
    board_type_str: String,
    port_path: String,
    _baud_rate: u32,
    connected: bool,
    // serial_port: Option<Box<dyn serialport::SerialPort>>, // Dependency needed
    _msg_id: u32,
}

impl SerialPeripheral {
    pub fn new(port_path: &str, board: &str, baud: u32) -> Result<Self> {
        if !Self::is_serial_path_allowed(port_path) {
            return Err(anyhow!("Permission denied: {}", port_path));
        }
        Ok(Self {
            peripheral_name: board.to_string(),
            board_type_str: board.to_string(),
            port_path: port_path.to_string(),
            _baud_rate: baud,
            connected: false,
            _msg_id: 0,
        })
    }

    fn is_serial_path_allowed(path: &str) -> bool {
        let allowed_prefixes = [
            "/dev/ttyACM",
            "/dev/ttyUSB",
            "/dev/tty.usbmodem",
            "/dev/cu.usbmodem",
            "/dev/tty.usbserial",
            "/dev/cu.usbserial",
        ];
        allowed_prefixes.iter().any(|p| path.starts_with(p))
    }
}

impl Peripheral for SerialPeripheral {
    fn name(&self) -> String {
        self.peripheral_name.clone()
    }

    fn board_type(&self) -> String {
        self.board_type_str.clone()
    }

    fn health_check(&self) -> bool {
        self.connected
    }

    fn init_peripheral(&mut self) -> Result<()> {
        if !Self::is_serial_path_allowed(&self.port_path) {
            return Err(anyhow!("Permission denied"));
        }
        // Stub implementation - actual serial open would go here
        // self.serial_port = Some(serialport::new(&self.port_path, self.baud_rate).open()?);
        self.connected = true;
        Ok(())
    }

    fn read(&mut self, _addr: u32) -> Result<u8> {
        if !self.connected {
            return Err(anyhow!("Not connected"));
        }
        // Stub
        Ok(0)
    }

    fn write_byte(&mut self, _addr: u32, _data: u8) -> Result<()> {
        if !self.connected {
            return Err(anyhow!("Not connected"));
        }
        // Stub
        Ok(())
    }

    fn flash_firmware(&mut self, _firmware_path: &str) -> Result<()> {
        Err(anyhow!("Flash not implemented"))
    }

    fn get_capabilities(&self) -> PeripheralCapabilities {
        PeripheralCapabilities {
            board_name: self.peripheral_name.clone(),
            board_type: self.board_type_str.clone(),
            has_serial: true,
            ..Default::default()
        }
    }
}
