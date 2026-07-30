//! Print the benchmark corpus inventory: every retrievable symbol, with the
//! stable `(file, qualified_name)` reference a golden label must use.
//!
//! Run this after changing a fixture to see what the parser actually produces:
//! `cargo run -p myceliums-benchmarks --bin corpus-inventory`

use anyhow::Result;
use myceliums_benchmarks::eval::corpus::{fixtures_root, symbol_ref, Corpus};

fn main() -> Result<()> {
    let corpus = Corpus::load(&fixtures_root()?)?;
    for symbol in corpus.symbols() {
        println!("{}\t{:?}", symbol_ref(symbol), symbol.kind);
    }
    eprintln!("{} symbols", corpus.len());
    Ok(())
}
