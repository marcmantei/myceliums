//! `myc email`.

use crate::repo::{data_dir, registry_path};
use anyhow::{Context, Result};
use myceliums_core::analyzer::Analyzer;
use std::io::Write;
use std::path::PathBuf;

fn imap_config_dir() -> PathBuf {
    data_dir().join("imap_config")
}

pub(crate) async fn cmd_email_connect(host: &str, user: &str, port: u16) -> Result<()> {
    use myceliums_core::imap::{self, ImapConfig};

    // Prompt for password
    eprint!("Password for {}@{}: ", user, host);
    std::io::stderr().flush()?;
    let password = rpassword::read_password().context("Failed to read password")?;

    let config = ImapConfig {
        host: host.to_string(),
        port,
        username: user.to_string(),
        password: password.clone(),
        use_tls: true,
        folders: vec!["INBOX".to_string()],
    };

    // Test connection
    println!("Testing connection to {}:{}...", host, port);
    let mut session = imap::connect(&config).await?;
    println!("Connected successfully!");
    session
        .logout()
        .await
        .map_err(|e| anyhow::anyhow!("logout failed: {}", e))?;

    // Save config
    let account_id = user.replace('@', "_at_").replace('.', "_");
    let config_dir = imap_config_dir();
    std::fs::create_dir_all(&config_dir)?;
    let config_path = config_dir.join(format!("{}.json", account_id));
    let json = serde_json::to_string_pretty(&config)?;
    std::fs::write(&config_path, json)?;

    println!(
        "Saved IMAP config for account '{}' to {}",
        account_id,
        config_path.display()
    );
    println!(
        "Run `myc email sync --account {}` to fetch emails.",
        account_id
    );
    Ok(())
}

pub(crate) async fn cmd_email_sync(account: Option<&str>) -> Result<()> {
    use myceliums_core::imap::{self, ImapConfig};

    let config_dir = imap_config_dir();
    if !config_dir.exists() {
        anyhow::bail!("No email accounts configured. Run `myc email connect` first.");
    }

    // Collect account configs to sync
    let configs: Vec<(String, ImapConfig)> = if let Some(acct) = account {
        let path = config_dir.join(format!("{}.json", acct));
        if !path.exists() {
            anyhow::bail!(
                "Account '{}' not found. Run `myc email connect` first.",
                acct
            );
        }
        let data = std::fs::read_to_string(&path)?;
        let cfg: ImapConfig = serde_json::from_str(&data)?;
        vec![(acct.to_string(), cfg)]
    } else {
        let mut all = Vec::new();
        for entry in std::fs::read_dir(&config_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "json") {
                let acct_id = path
                    .file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string();
                let data = std::fs::read_to_string(&path)?;
                let cfg: ImapConfig = serde_json::from_str(&data)?;
                all.push((acct_id, cfg));
            }
        }
        if all.is_empty() {
            anyhow::bail!("No email accounts configured. Run `myc email connect` first.");
        }
        all
    };

    let state_dir = data_dir();
    for (acct_id, config) in &configs {
        println!("Syncing account '{}'...", acct_id);
        let mut session = imap::connect(config).await?;
        let mut state = imap::load_sync_state(&state_dir, acct_id)?;

        let mut total_emails = 0usize;
        for folder in &config.folders {
            let emails = imap::fetch_new_emails(&mut session, &mut state, folder).await?;
            println!("  {} — {} new emails", folder, emails.len());
            total_emails += emails.len();

            // Run emails through the analyzer pipeline
            if !emails.is_empty() {
                let repo_id = format!("email-{}", acct_id);
                let db_path = myceliums_storage::RepoRegistry::repo_db_path(&data_dir(), &repo_id);
                let store = myceliums_storage::Store::open(&db_path, &repo_id).await?;

                // Write each email to a temp file and analyze
                let temp_dir = std::env::temp_dir().join("myceliums-email-sync");
                std::fs::create_dir_all(&temp_dir)?;
                let mut paths = Vec::new();
                for (i, email) in emails.iter().enumerate() {
                    let eml_path = temp_dir.join(format!("{}_{}.eml", acct_id, i));
                    // Reconstruct a minimal EML for the analyzer
                    let eml_content = format!(
                        "From: {}\r\nTo: {}\r\nSubject: {}\r\nMessage-ID: <{}>\r\n{}{}\r\n\r\n{}",
                        email.from,
                        email.to.join(", "),
                        email.subject,
                        email.message_id,
                        email
                            .in_reply_to
                            .as_ref()
                            .map(|r| format!("In-Reply-To: <{}>\r\n", r))
                            .unwrap_or_default(),
                        email
                            .date
                            .as_ref()
                            .map(|d| format!("Date: {}\r\n", d))
                            .unwrap_or_default(),
                        email.body,
                    );
                    std::fs::write(&eml_path, eml_content)?;
                    paths.push(eml_path);
                }

                let analyzer = Analyzer::new(store, temp_dir.clone()).set_skip_embeddings(true);
                let result = analyzer.analyze_files(&paths).await?;
                println!(
                    "  Indexed: {} symbols, {} relationships",
                    result.symbol_count, result.relationship_count
                );

                // Register in repo registry
                let mut registry = myceliums_storage::RepoRegistry::load(&registry_path())?;
                registry.register(myceliums_storage::RepoInfo {
                    id: repo_id.clone(),
                    name: format!("Email: {}", acct_id),
                    path: temp_dir.to_string_lossy().to_string(),
                    analyzed_at: chrono::Utc::now().to_rfc3339(),
                    symbol_count: result.symbol_count as u32,
                    file_count: result.file_count as u32,
                    analyzed_commit: None,
                    vector_geometry_version: myceliums_storage::schema::VECTOR_GEOMETRY_VERSION,
                    classification_version:
                        myceliums_storage::schema::SYMBOL_CLASSIFICATION_VERSION,
                });
                registry.save()?;

                // Clean up temp files
                // best-effort: temp dir removal failing does not affect user data.
                let _ = std::fs::remove_dir_all(&temp_dir);
            }
        }

        imap::save_sync_state(&state_dir, &state)?;
        session
            .logout()
            .await
            .map_err(|e| anyhow::anyhow!("logout failed: {}", e))?;
        println!(
            "Done — {} total new emails synced for '{}'",
            total_emails, acct_id
        );
    }

    Ok(())
}

pub(crate) async fn cmd_email_disconnect(account: &str) -> Result<()> {
    let config_path = imap_config_dir().join(format!("{}.json", account));
    if !config_path.exists() {
        anyhow::bail!("Account '{}' not found.", account);
    }
    std::fs::remove_file(&config_path)?;
    println!("Removed IMAP config for account '{}'.", account);

    // Also remove sync state if it exists
    let state_path = data_dir()
        .join("imap_state")
        .join(format!("{}.json", account));
    if state_path.exists() {
        std::fs::remove_file(&state_path)?;
        println!("Removed sync state for account '{}'.", account);
    }

    Ok(())
}
