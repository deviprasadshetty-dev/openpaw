use anyhow::Result;
use clap::ValueEnum;

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ServiceCommand {
    Install,
    Start,
    Stop,
    Restart,
    Status,
    Uninstall,
}

pub struct Service;

impl Service {
    pub fn handle_command(command: ServiceCommand, _config_path: &str) -> Result<()> {
        match command {
            ServiceCommand::Install => Self::install(),
            ServiceCommand::Start => Self::start(),
            ServiceCommand::Stop => Self::stop(),
            ServiceCommand::Restart => Self::restart(),
            ServiceCommand::Status => Self::status(),
            ServiceCommand::Uninstall => Self::uninstall(),
        }
    }

    #[cfg(target_os = "macos")]
    fn install() -> Result<()> {
        println!("Installing launchd service...");
        // Placeholder for launchctl load logic
        Ok(())
    }

    #[cfg(target_os = "linux")]
    fn install() -> Result<()> {
        println!("Installing systemd service...");
        // Placeholder for systemctl enable logic
        Ok(())
    }

    #[cfg(target_os = "windows")]
    fn install() -> Result<()> {
        println!("Installing Windows service...");
        // Placeholder for sc.exe create logic
        Ok(())
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    fn install() -> Result<()> {
        Err(anyhow!("Unsupported platform"))
    }

    fn start() -> Result<()> {
        // Platform specific start logic
        #[cfg(target_os = "macos")]
        {
            Command::new("launchctl")
                .arg("start")
                .arg("com.openpaw.daemon")
                .status()?;
        }
        #[cfg(target_os = "linux")]
        {
            Command::new("systemctl")
                .arg("--user")
                .arg("start")
                .arg("openpaw.service")
                .status()?;
        }
        Ok(())
    }

    fn stop() -> Result<()> {
        #[cfg(target_os = "macos")]
        {
            Command::new("launchctl")
                .arg("stop")
                .arg("com.openpaw.daemon")
                .status()?;
        }
        #[cfg(target_os = "linux")]
        {
            Command::new("systemctl")
                .arg("--user")
                .arg("stop")
                .arg("openpaw.service")
                .status()?;
        }
        Ok(())
    }

    fn restart() -> Result<()> {
        Self::stop()?;
        Self::start()
    }

    fn status() -> Result<()> {
        #[cfg(target_os = "macos")]
        {
            Command::new("launchctl")
                .arg("list")
                .arg("com.openpaw.daemon")
                .status()?;
        }
        #[cfg(target_os = "linux")]
        {
            Command::new("systemctl")
                .arg("--user")
                .arg("status")
                .arg("openpaw.service")
                .status()?;
        }
        Ok(())
    }

    fn uninstall() -> Result<()> {
        Self::stop()?;
        #[cfg(target_os = "macos")]
        {
            println!("Unloading launchd service...");
        }
        #[cfg(target_os = "linux")]
        {
            println!("Disabling systemd service...");
        }
        Ok(())
    }
}
