//! `myc configure`.

use crate::repo::data_dir;
use anyhow::Result;
use myceliums_core::GlobalConfig;

pub(crate) async fn cmd_configure(set: Option<&str>, reset: bool) -> Result<()> {
    let data = data_dir();
    let mut config = GlobalConfig::load(&data)?;

    if reset {
        config.reset();
        config.save()?;
        println!("Configuration reset to defaults.");
        println!();
        print!("{}", config.display());
        return Ok(());
    }

    if let Some(kv) = set {
        let (key, value) = kv
            .split_once('=')
            .ok_or_else(|| anyhow::anyhow!("Invalid format. Use: --set key=value"))?;
        if key.trim() == "mcp.toolsets" {
            value
                .parse::<myceliums_mcp::Toolsets>()
                .map_err(|e| anyhow::anyhow!("mcp.toolsets: {e}"))?;
        }
        config.set(key.trim(), value.trim())?;
        config.save()?;
        println!("Set {} = {}", key.trim(), value.trim());
        return Ok(());
    }

    // Show current config
    print!("{}", config.display());
    Ok(())
}
