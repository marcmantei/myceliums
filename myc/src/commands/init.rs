//! `myc init`.

use anyhow::Result;
use myceliums_core::config::{self, ProjectConfig};

pub(crate) async fn cmd_init() -> Result<()> {
    let cwd = std::env::current_dir()?;
    let config_path = cwd.join(config::CONFIG_FILENAME);

    if config_path.exists() {
        println!("{} already exists.", config::CONFIG_FILENAME);
        return Ok(());
    }

    let dir_name = cwd
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();

    let mut cfg = ProjectConfig::default();
    cfg.project.name = dir_name;

    cfg.save(&config_path)?;
    println!("Created {} with default settings.", config::CONFIG_FILENAME);
    println!("Edit it to customize analysis for this project.");
    Ok(())
}
