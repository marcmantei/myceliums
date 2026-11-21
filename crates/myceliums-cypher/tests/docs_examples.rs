//! Every Cypher example in `docs/guides/cypher-queries.md` runs (#118): the
//! guide once listed labels (`Function`, `Class`, `Community`) the executor
//! rejects.

mod support;

use support::small_graph::small_graph_executor;

/// The bodies of the ```` ```cypher ```` blocks of `markdown`.
fn cypher_blocks(markdown: &str) -> Vec<String> {
    let mut blocks = Vec::new();
    let mut current: Option<String> = None;
    for line in markdown.lines() {
        match (&mut current, line.trim()) {
            (None, "```cypher") => current = Some(String::new()),
            (Some(block), "```") => {
                blocks.push(std::mem::take(block));
                current = None;
            }
            (Some(block), _) => {
                block.push_str(line);
                block.push('\n');
            }
            (None, _) => {}
        }
    }
    blocks
}

#[test]
fn every_example_in_the_cypher_guide_executes() {
    let guide = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../docs/guides/cypher-queries.md"
    ))
    .expect("the guide exists");
    let blocks = cypher_blocks(&guide);
    assert!(blocks.len() >= 10, "found only {} examples", blocks.len());
    let executor = small_graph_executor();
    for query in blocks {
        if let Err(e) = executor.execute(&query) {
            panic!("guide example fails: {e}\n{query}");
        }
    }
}
