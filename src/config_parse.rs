use crate::config::Config;
use anyhow::Result;

pub fn parse_config(
    json_content: &str,
    config_path: &str,
    enable_encryption: bool,
) -> Result<Config> {
    let mut config: Config = serde_json::from_str(json_content)?;
    config.config_path = config_path.to_string();

    // Initialize secret store and decrypt secrets
    config.init_secret_store(enable_encryption)?;
    config.decrypt_secrets()?;

    Ok(config)
}
