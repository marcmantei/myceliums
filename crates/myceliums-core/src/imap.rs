//! IMAP live email connector for incremental mailbox synchronisation.
//!
//! Connects to an IMAP server, fetches emails newer than the last-seen UID
//! per folder, and returns them as [`ParsedEmail`] values (from the `email`
//! module). Sync state is persisted as JSON under
//! `<data_dir>/imap_state/<account_id>.json`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::email::{parse_eml, ParsedEmail};

// ── Configuration ────────────────────────────────────────────────────

/// Configuration for connecting to an IMAP mailbox.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImapConfig {
    /// IMAP server hostname (e.g. `imap.gmail.com`).
    pub host: String,
    /// IMAP server port. Defaults to `993` for TLS connections.
    pub port: u16,
    /// Login username (usually the full email address).
    pub username: String,
    /// Login password or OAuth token.
    pub password: String,
    /// Whether to use TLS (IMAPS). Defaults to `true`.
    pub use_tls: bool,
    /// Folders to synchronise. Defaults to `["INBOX"]`.
    pub folders: Vec<String>,
}

impl Default for ImapConfig {
    fn default() -> Self {
        Self {
            host: String::new(),
            port: 993,
            username: String::new(),
            password: String::new(),
            use_tls: true,
            folders: vec!["INBOX".to_string()],
        }
    }
}

// ── Sync state ───────────────────────────────────────────────────────

/// Persistent synchronisation state for an IMAP account.
///
/// Tracks the highest UID fetched per folder so that subsequent syncs
/// only retrieve new messages.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImapSyncState {
    /// Opaque identifier for the account (used as the filename stem).
    pub account_id: String,
    /// Highest UID successfully fetched, keyed by folder name.
    pub last_uid_per_folder: HashMap<String, u32>,
    /// ISO 8601 timestamp of the most recent sync.
    pub last_sync: String,
}

impl ImapSyncState {
    /// Create a new, empty sync state for the given account.
    pub fn new(account_id: impl Into<String>) -> Self {
        Self {
            account_id: account_id.into(),
            last_uid_per_folder: HashMap::new(),
            last_sync: chrono::Utc::now().to_rfc3339(),
        }
    }
}

// ── State persistence ────────────────────────────────────────────────

/// Return the path to the sync-state JSON file for an account.
fn state_file_path(data_dir: &Path, account_id: &str) -> PathBuf {
    data_dir
        .join("imap_state")
        .join(format!("{account_id}.json"))
}

/// Load persisted sync state from `<data_dir>/imap_state/<account_id>.json`.
///
/// Returns a fresh [`ImapSyncState`] if the file does not exist yet.
pub fn load_sync_state(data_dir: &Path, account_id: &str) -> Result<ImapSyncState> {
    let path = state_file_path(data_dir, account_id);
    if !path.exists() {
        return Ok(ImapSyncState::new(account_id));
    }
    let data = std::fs::read_to_string(&path)
        .with_context(|| format!("failed to read sync state from {}", path.display()))?;
    serde_json::from_str(&data)
        .with_context(|| format!("failed to parse sync state from {}", path.display()))
}

/// Persist sync state to `<data_dir>/imap_state/<account_id>.json`.
pub fn save_sync_state(data_dir: &Path, state: &ImapSyncState) -> Result<()> {
    let path = state_file_path(data_dir, &state.account_id);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create directory {}", parent.display()))?;
    }
    let json = serde_json::to_string_pretty(state)?;
    std::fs::write(&path, json)
        .with_context(|| format!("failed to write sync state to {}", path.display()))?;
    Ok(())
}

// ── IMAP session type alias ──────────────────────────────────────────

/// An authenticated IMAP session over a TLS stream.
pub type ImapSession = async_imap::Session<async_native_tls::TlsStream<tokio::net::TcpStream>>;

// ── Connection ───────────────────────────────────────────────────────

/// Establish an authenticated IMAP session using the provided config.
pub async fn connect(config: &ImapConfig) -> Result<ImapSession> {
    let tls = async_native_tls::TlsConnector::new();

    let addr = (config.host.as_str(), config.port);
    let tcp = tokio::net::TcpStream::connect(addr)
        .await
        .with_context(|| format!("failed to connect to {}:{}", config.host, config.port))?;

    let tls_stream = tls
        .connect(&config.host, tcp)
        .await
        .with_context(|| format!("TLS handshake failed with {}", config.host))?;

    let client = async_imap::Client::new(tls_stream);
    let session = client
        .login(&config.username, &config.password)
        .await
        .map_err(|(err, _client)| err)
        .with_context(|| {
            format!(
                "IMAP login failed for {} on {}",
                config.username, config.host
            )
        })?;

    Ok(session)
}

// ── Fetch new emails ─────────────────────────────────────────────────

/// Fetch emails from `folder` whose UID is greater than the last-seen UID
/// recorded in `state`.
///
/// Returns a `Vec<ParsedEmail>` and updates `state.last_uid_per_folder` in
/// place. The caller is responsible for persisting the state afterwards via
/// [`save_sync_state`].
pub async fn fetch_new_emails(
    session: &mut ImapSession,
    state: &mut ImapSyncState,
    folder: &str,
) -> Result<Vec<ParsedEmail>> {
    use futures::StreamExt;

    session
        .select(folder)
        .await
        .with_context(|| format!("failed to SELECT folder {folder}"))?;

    let last_uid = state.last_uid_per_folder.get(folder).copied().unwrap_or(0);

    // IMAP UID ranges: "last+1:*"
    let range = format!("{}:*", last_uid + 1);
    let mut fetch_stream = session
        .uid_fetch(&range, "(UID RFC822)")
        .await
        .with_context(|| format!("UID FETCH failed for range {range} in {folder}"))?;

    let mut emails = Vec::new();
    let mut max_uid = last_uid;

    while let Some(result) = fetch_stream.next().await {
        let msg = result.with_context(|| "error reading IMAP fetch stream")?;
        let uid = msg.uid.unwrap_or(0);
        if uid <= last_uid {
            // IMAP may return the boundary UID; skip it.
            continue;
        }
        if let Some(body) = msg.body() {
            match parse_eml(body) {
                Ok(parsed) => {
                    emails.push(parsed);
                    if uid > max_uid {
                        max_uid = uid;
                    }
                }
                Err(e) => {
                    tracing::warn!(uid, folder, "skipping unparseable email: {e:#}");
                }
            }
        }
    }
    // Drop the stream before mutating session again.
    drop(fetch_stream);

    if max_uid > last_uid {
        state
            .last_uid_per_folder
            .insert(folder.to_string(), max_uid);
        state.last_sync = chrono::Utc::now().to_rfc3339();
    }

    Ok(emails)
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_sync_state_save_load_roundtrip() {
        let tmp = TempDir::new().unwrap();
        let data_dir = tmp.path();

        let mut state = ImapSyncState::new("test-account");
        state.last_uid_per_folder.insert("INBOX".into(), 42);
        state.last_uid_per_folder.insert("Sent".into(), 7);
        state.last_sync = "2026-04-19T12:00:00Z".to_string();

        save_sync_state(data_dir, &state).unwrap();
        let loaded = load_sync_state(data_dir, "test-account").unwrap();

        assert_eq!(loaded.account_id, "test-account");
        assert_eq!(loaded.last_uid_per_folder.get("INBOX"), Some(&42));
        assert_eq!(loaded.last_uid_per_folder.get("Sent"), Some(&7));
        assert_eq!(loaded.last_sync, "2026-04-19T12:00:00Z");
    }

    #[test]
    fn test_load_sync_state_missing_file_returns_fresh() {
        let tmp = TempDir::new().unwrap();
        let state = load_sync_state(tmp.path(), "nonexistent").unwrap();
        assert_eq!(state.account_id, "nonexistent");
        assert!(state.last_uid_per_folder.is_empty());
    }

    #[test]
    fn test_config_serialization_roundtrip() {
        let config = ImapConfig {
            host: "imap.example.com".into(),
            port: 993,
            username: "user@example.com".into(),
            password: "secret".into(),
            use_tls: true,
            folders: vec!["INBOX".into(), "Archive".into()],
        };

        let json = serde_json::to_string(&config).unwrap();
        let deserialized: ImapConfig = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.host, "imap.example.com");
        assert_eq!(deserialized.port, 993);
        assert_eq!(deserialized.folders.len(), 2);
    }

    #[test]
    fn test_config_default() {
        let config = ImapConfig::default();
        assert_eq!(config.port, 993);
        assert!(config.use_tls);
        assert_eq!(config.folders, vec!["INBOX".to_string()]);
    }

    #[test]
    fn test_state_file_path() {
        let path = state_file_path(Path::new("/home/user/.myceliums"), "my-acct");
        assert_eq!(
            path,
            PathBuf::from("/home/user/.myceliums/imap_state/my-acct.json")
        );
    }

    #[tokio::test]
    #[ignore]
    async fn test_connect_live() {
        // Requires a real IMAP server. Set env vars to run:
        //   IMAP_HOST, IMAP_USER, IMAP_PASS
        let config = ImapConfig {
            host: std::env::var("IMAP_HOST").unwrap(),
            port: 993,
            username: std::env::var("IMAP_USER").unwrap(),
            password: std::env::var("IMAP_PASS").unwrap(),
            use_tls: true,
            folders: vec!["INBOX".into()],
        };
        let mut session = connect(&config).await.unwrap();
        session.logout().await.unwrap();
    }

    #[tokio::test]
    #[ignore]
    async fn test_fetch_new_emails_live() {
        let config = ImapConfig {
            host: std::env::var("IMAP_HOST").unwrap(),
            port: 993,
            username: std::env::var("IMAP_USER").unwrap(),
            password: std::env::var("IMAP_PASS").unwrap(),
            use_tls: true,
            folders: vec!["INBOX".into()],
        };
        let mut session = connect(&config).await.unwrap();
        let mut state = ImapSyncState::new("live-test");
        let emails = fetch_new_emails(&mut session, &mut state, "INBOX")
            .await
            .unwrap();
        println!("Fetched {} emails", emails.len());
        session.logout().await.unwrap();
    }
}
