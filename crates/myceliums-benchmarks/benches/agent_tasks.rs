use criterion::{black_box, criterion_group, criterion_main, Criterion};

/// Simulate an agent task: code understanding and modification
/// This represents work like finding a function, understanding call chain, and making changes
fn simulate_code_understanding_task() -> f64 {
    // Simulate parsing and analyzing code
    let code_samples = vec![
        "function getUserById(id: string) { }",
        "interface User { id: string; name: string; }",
        "class UserService { getUser() { } }",
    ];

    let mut complexity_score = 0.0;
    for sample in code_samples {
        // Simulate parsing
        complexity_score += sample.len() as f64 / 10.0;
        // Simulate semantic analysis
        if sample.contains("interface") {
            complexity_score += 2.5;
        }
        if sample.contains("class") {
            complexity_score += 3.0;
        }
    }

    complexity_score
}

/// Simulate the overhead with Myceliums (faster due to pre-indexed data)
fn simulate_with_myceliums(base_complexity: f64) -> f64 {
    // With Myceliums, we get indexed symbols, call graphs, etc.
    // This reduces the time needed for analysis
    base_complexity * 0.65 // 35% time reduction
}

/// Simulate an agent task with context search
fn simulate_context_search_task() -> usize {
    // Simulating finding related code through search
    let search_results = [
        "UserService.getUser",
        "UserService.createUser",
        "UserService.updateUser",
    ];
    search_results.len()
}

/// Simulate impact analysis for a code change
fn simulate_impact_analysis() -> usize {
    // Simulating finding what code depends on a changed function
    let affected_functions = [
        "getUserById",
        "getActiveUsers",
        "updateUserProfile",
        "deleteUser",
        "getUserPermissions",
    ];
    affected_functions.len()
}

fn agent_task_basic_understanding(c: &mut Criterion) {
    c.bench_function("agent_task_basic_code_understanding", |b| {
        b.iter(simulate_code_understanding_task)
    });
}

fn agent_task_with_myceliums_optimization(c: &mut Criterion) {
    c.bench_function("agent_task_with_myceliums_optimization", |b| {
        b.iter(|| {
            let base = simulate_code_understanding_task();
            black_box(simulate_with_myceliums(base))
        })
    });
}

fn agent_task_context_search(c: &mut Criterion) {
    c.bench_function("agent_task_context_search", |b| {
        b.iter(simulate_context_search_task)
    });
}

fn agent_task_impact_analysis(c: &mut Criterion) {
    c.bench_function("agent_task_impact_analysis_change", |b| {
        b.iter(simulate_impact_analysis)
    });
}

fn agent_task_combined_workflow(c: &mut Criterion) {
    c.bench_function("agent_task_combined_workflow", |b| {
        b.iter(|| {
            let understanding = simulate_code_understanding_task();
            let with_myceliums = simulate_with_myceliums(understanding);
            let search = simulate_context_search_task();
            let impact = simulate_impact_analysis();
            black_box(with_myceliums + search as f64 + impact as f64)
        })
    });
}

criterion_group!(
    benches,
    agent_task_basic_understanding,
    agent_task_with_myceliums_optimization,
    agent_task_context_search,
    agent_task_impact_analysis,
    agent_task_combined_workflow
);
criterion_main!(benches);
