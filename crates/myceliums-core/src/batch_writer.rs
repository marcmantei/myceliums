//! Streaming batch writer for incremental storage during parsing.
//!
//! Instead of accumulating all symbols, files, and relationships in memory
//! before writing, [`BatchAccumulator`] buffers items and flushes them to the
//! [`Store`] in fixed-size batches via an async channel. The background
//! [`batch_writer_task`] drains the channel and persists each batch.

use std::sync::Arc;

use myceliums_storage::{CodeSymbol, FileNode, Relationship, Store};
use tokio::sync::mpsc;
use tracing::info;

// ── Messages ─────────────────────────────────────────────────────────

/// Messages that the batch writer can receive.
pub enum BatchMessage {
    Symbols(Vec<CodeSymbol>),
    Files(Vec<FileNode>),
    Relationships(Vec<Relationship>),
}

// ── Configuration ────────────────────────────────────────────────────

/// Configuration for the batch writer.
#[derive(Debug, Clone)]
pub struct BatchConfig {
    /// Number of items to buffer before flushing a batch.
    pub batch_size: usize,
    /// Capacity of the async channel between producers and the writer task.
    pub channel_buffer_size: usize,
}

impl Default for BatchConfig {
    fn default() -> Self {
        Self {
            batch_size: 500,
            channel_buffer_size: 8,
        }
    }
}

// ── Accumulator ──────────────────────────────────────────────────────

/// Accumulator that buffers items and flushes when `batch_size` is reached.
///
/// `F` is a closure that converts the accumulated `Vec<T>` into a
/// [`BatchMessage`] variant (e.g. `BatchMessage::Symbols`).
pub struct BatchAccumulator<T, F>
where
    F: Fn(Vec<T>) -> BatchMessage,
{
    items: Vec<T>,
    batch_size: usize,
    sender: mpsc::Sender<BatchMessage>,
    wrap: F,
}

impl<T, F> BatchAccumulator<T, F>
where
    F: Fn(Vec<T>) -> BatchMessage,
{
    /// Create a new accumulator.
    pub fn new(batch_size: usize, sender: mpsc::Sender<BatchMessage>, wrap: F) -> Self {
        Self {
            items: Vec::with_capacity(batch_size),
            batch_size,
            sender,
            wrap,
        }
    }

    /// Push items into the buffer, flushing whenever the batch size is reached.
    pub async fn push(
        &mut self,
        new_items: Vec<T>,
    ) -> Result<(), mpsc::error::SendError<BatchMessage>> {
        self.items.extend(new_items);
        while self.items.len() >= self.batch_size {
            let rest = self.items.split_off(self.batch_size);
            let batch = std::mem::replace(&mut self.items, rest);
            self.sender.send((self.wrap)(batch)).await?;
        }
        Ok(())
    }

    /// Flush any remaining items (call after all items have been pushed).
    pub async fn flush(mut self) -> Result<(), mpsc::error::SendError<BatchMessage>> {
        if !self.items.is_empty() {
            let batch = std::mem::take(&mut self.items);
            self.sender.send((self.wrap)(batch)).await?;
        }
        Ok(())
    }
}

// ── Background writer task ───────────────────────────────────────────

/// Background task that consumes [`BatchMessage`]s from the channel and
/// writes them to the [`Store`].
///
/// Returns `(symbol_count, file_count, relationship_count)` on completion.
pub async fn batch_writer_task(
    mut rx: mpsc::Receiver<BatchMessage>,
    store: Arc<Store>,
) -> (usize, usize, usize) {
    let mut symbol_count = 0usize;
    let mut file_count = 0usize;
    let mut rel_count = 0usize;

    while let Some(msg) = rx.recv().await {
        match msg {
            BatchMessage::Symbols(syms) => {
                let n = syms.len();
                if let Err(e) = store.store_symbols(&syms).await {
                    tracing::warn!("Batch write error (symbols): {e}");
                } else {
                    symbol_count += n;
                    info!("Wrote batch of {n} symbols (total: {symbol_count})");
                }
            }
            BatchMessage::Files(files) => {
                let n = files.len();
                if let Err(e) = store.store_files(&files).await {
                    tracing::warn!("Batch write error (files): {e}");
                } else {
                    file_count += n;
                    info!("Wrote batch of {n} files (total: {file_count})");
                }
            }
            BatchMessage::Relationships(rels) => {
                let n = rels.len();
                if let Err(e) = store.store_relationships(&rels).await {
                    tracing::warn!("Batch write error (relationships): {e}");
                } else {
                    rel_count += n;
                    info!("Wrote batch of {n} relationships (total: {rel_count})");
                }
            }
        }
    }

    info!(
        "Batch writer finished: {symbol_count} symbols, {file_count} files, {rel_count} relationships"
    );
    (symbol_count, file_count, rel_count)
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_batch_config_defaults() {
        let cfg = BatchConfig::default();
        assert_eq!(cfg.batch_size, 500);
        assert_eq!(cfg.channel_buffer_size, 8);
    }

    #[tokio::test]
    async fn test_accumulator_flushes_at_batch_size() {
        let (tx, mut rx) = mpsc::channel::<BatchMessage>(16);

        let mut acc = BatchAccumulator::new(3, tx, BatchMessage::Symbols);

        // Push 2 items — no flush yet
        let syms: Vec<CodeSymbol> = (0..2).map(|i| make_symbol(&format!("sym{i}"))).collect();
        acc.push(syms).await.unwrap();
        // Channel should be empty
        assert!(rx.try_recv().is_err());

        // Push 2 more — total 4, should flush one batch of 3
        let syms: Vec<CodeSymbol> = (2..4).map(|i| make_symbol(&format!("sym{i}"))).collect();
        acc.push(syms).await.unwrap();
        let msg = rx.try_recv().expect("should have received a batch");
        match msg {
            BatchMessage::Symbols(s) => assert_eq!(s.len(), 3),
            _ => panic!("expected Symbols variant"),
        }

        // Flush remainder (1 item)
        acc.flush().await.unwrap();
        let msg = rx.try_recv().expect("should have received remainder");
        match msg {
            BatchMessage::Symbols(s) => assert_eq!(s.len(), 1),
            _ => panic!("expected Symbols variant"),
        }
    }

    #[tokio::test]
    async fn test_channel_round_trip() {
        let (tx, mut rx) = mpsc::channel::<BatchMessage>(4);

        tx.send(BatchMessage::Symbols(vec![make_symbol("a")]))
            .await
            .unwrap();
        tx.send(BatchMessage::Files(vec![make_file("b.rs")]))
            .await
            .unwrap();
        tx.send(BatchMessage::Relationships(vec![make_rel("a", "b")]))
            .await
            .unwrap();
        drop(tx);

        let mut sym_count = 0;
        let mut file_count = 0;
        let mut rel_count = 0;
        while let Some(msg) = rx.recv().await {
            match msg {
                BatchMessage::Symbols(s) => sym_count += s.len(),
                BatchMessage::Files(f) => file_count += f.len(),
                BatchMessage::Relationships(r) => rel_count += r.len(),
            }
        }
        assert_eq!(sym_count, 1);
        assert_eq!(file_count, 1);
        assert_eq!(rel_count, 1);
    }

    #[tokio::test]
    async fn test_accumulator_flush_empty_is_noop() {
        let (tx, mut rx) = mpsc::channel::<BatchMessage>(4);
        let acc: BatchAccumulator<CodeSymbol, _> =
            BatchAccumulator::new(10, tx, BatchMessage::Symbols);
        acc.flush().await.unwrap();
        // Nothing should have been sent
        assert!(rx.try_recv().is_err());
    }

    // ── Test helpers ─────────────────────────────────────────────────

    fn make_symbol(name: &str) -> CodeSymbol {
        CodeSymbol {
            uid: format!("uid-{name}"),
            name: name.to_string(),
            qualified_name: name.to_string(),
            kind: myceliums_storage::SymbolKind::Function,
            file_path: "test.rs".to_string(),
            start_line: 1,
            end_line: 10,
            signature: String::new(),
            content: String::new(),
            repo_id: "test-repo".to_string(),
            metadata: None,
        }
    }

    fn make_file(path: &str) -> FileNode {
        FileNode {
            uid: format!("uid-{path}"),
            path: path.to_string(),
            language: "rust".to_string(),
            repo_id: "test-repo".to_string(),
            num_symbols: 0,
        }
    }

    fn make_rel(from: &str, to: &str) -> Relationship {
        Relationship {
            uid: format!("uid-rel-{from}-{to}"),
            source_uid: format!("uid-{from}"),
            target_uid: format!("uid-{to}"),
            kind: myceliums_storage::RelationshipKind::Calls,
            repo_id: "test-repo".to_string(),
            metadata: String::new(),
        }
    }
}
