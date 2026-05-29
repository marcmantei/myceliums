use criterion::{black_box, criterion_group, criterion_main, Criterion};

/// Simulate a simple symbol lookup by finding occurrences of a string in source files
fn simple_symbol_lookup(content: &str, symbol: &str) -> usize {
    content.matches(symbol).count()
}

/// Simulate a complex query by counting relationships between symbols
fn complex_call_graph_query(content: &str) -> usize {
    content.lines().filter(|line| line.contains("->")).count()
}

/// Simulate graph traversal
fn graph_traversal_simulation(nodes: usize) -> usize {
    // Simulating BFS traversal
    let mut visited = 0;
    let mut queue = vec![0];
    let mut visited_set = std::collections::HashSet::new();

    while !queue.is_empty() {
        let node = queue.remove(0);
        if visited_set.insert(node) {
            visited += 1;
            // Simulate discovering neighbors
            for neighbor in 0..std::cmp::min(3, nodes) {
                if !visited_set.contains(&neighbor) {
                    queue.push((node + neighbor + 1) % nodes);
                }
            }
        }
    }
    visited
}

fn query_simple_lookup(c: &mut Criterion) {
    let sample_code = r#"
        function getUser() { }
        function findUser() { }
        function updateUser() { }
        class User { }
        interface IUser { }
        const user = null;
        const userName = "";
    "#
    .repeat(10);

    c.bench_function("query_simple_symbol_lookup", |b| {
        b.iter(|| simple_symbol_lookup(black_box(&sample_code), black_box("User")))
    });
}

fn query_complex_call_graph(c: &mut Criterion) {
    let sample_code = r#"
        getUser() -> findUser()
        findUser() -> User.getName()
        User.getName() -> formatString()
        formatString() -> trim()
        updateUser() -> Database.update()
        Database.update() -> Query.execute()
    "#
    .repeat(20);

    c.bench_function("query_complex_call_graph", |b| {
        b.iter(|| complex_call_graph_query(black_box(&sample_code)))
    });
}

fn query_graph_traversal(c: &mut Criterion) {
    c.bench_function("query_graph_traversal_1000_nodes", |b| {
        b.iter(|| graph_traversal_simulation(black_box(1000)))
    });
}

fn query_graph_traversal_large(c: &mut Criterion) {
    c.bench_function("query_graph_traversal_10000_nodes", |b| {
        b.iter(|| graph_traversal_simulation(black_box(10000)))
    });
}

criterion_group!(
    benches,
    query_simple_lookup,
    query_complex_call_graph,
    query_graph_traversal,
    query_graph_traversal_large
);
criterion_main!(benches);
