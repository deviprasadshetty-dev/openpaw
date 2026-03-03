use crate::config::Config;
use anyhow::Result;

pub fn parse_config(json_content: &str) -> Result<Config> {
    // In the future, we might need custom logic to handle legacy formats
    // (e.g. single object vs array for channels), but for now we rely on serde.
    let config: Config = serde_json::from_str(json_content)?;
    Ok(config)
}
