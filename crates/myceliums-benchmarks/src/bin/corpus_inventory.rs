//! Print a benchmark corpus inventory: every retrievable symbol, with the
//! stable `(file, qualified_name)` reference a label must use.
//!
//! Run this after changing a fixture to see what the parser actually produces,
//! or point it at a real-repository checkout to see what a real-repo label can
//! name:
//!
//! ```text
//! cargo run -p myceliums-benchmarks --bin corpus-inventory
//! cargo run -p myceliums-benchmarks --bin corpus-inventory -- --repo ~/.cache/myceliums-eval-repos/gin
//! ```
//!
//! The repository form prints `file::qualified_name<TAB>kind<TAB>start-end`.
//! It lists what the parser extracts — it does not search, so labelling from
//! it does not borrow the engine's own ranking.

use anyhow::{bail, Context, Result};
use myceliums_benchmarks::eval::corpus::{fixtures_root, symbol_ref, Corpus};
use std::path::PathBuf;

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let mut repo: Option<PathBuf> = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--repo" => repo = Some(PathBuf::from(args.next().context("--repo needs a path")?)),
            other => bail!("unknown argument '{other}' (expected --repo <checkout>)"),
        }
    }
    let corpus = match &repo {
        Some(root) => {
            let name = root
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "repo".to_string());
            Corpus::load_repository(root, &name)?
        }
        None => Corpus::load(&fixtures_root()?)?,
    };
    for symbol in corpus.symbols() {
        println!(
            "{}\t{:?}\t{}-{}",
            symbol_ref(symbol),
            symbol.kind,
            symbol.start_line,
            symbol.end_line
        );
    }
    for (file, reason) in corpus.skipped() {
        eprintln!("skipped {file}: {reason}");
    }
    eprintln!("{} symbols", corpus.len());
    Ok(())
}
