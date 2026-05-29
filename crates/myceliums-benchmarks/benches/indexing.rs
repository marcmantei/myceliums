use criterion::{black_box, criterion_group, criterion_main, Criterion};
use myceliums_benchmarks::fixtures::FixtureGenerator;
use std::path::Path;

fn count_project_files(path: &Path) -> usize {
    walkdir::WalkDir::new(path)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file())
        .count()
}

fn indexing_small_ts(c: &mut Criterion) {
    c.bench_function("indexing_small_ts_10_files", |b| {
        b.iter_batched(
            || {
                let gen = FixtureGenerator::new().expect("Failed to create generator");
                gen.generate_small_ts_project()
                    .expect("Failed to generate project")
            },
            |project_path| black_box(count_project_files(&project_path)),
            criterion::BatchSize::SmallInput,
        )
    });
}

fn indexing_medium_ts(c: &mut Criterion) {
    c.bench_function("indexing_medium_ts_100_files", |b| {
        b.iter_batched(
            || {
                let gen = FixtureGenerator::new().expect("Failed to create generator");
                gen.generate_medium_ts_project()
                    .expect("Failed to generate project")
            },
            |project_path| black_box(count_project_files(&project_path)),
            criterion::BatchSize::SmallInput,
        )
    });
}

fn indexing_small_py(c: &mut Criterion) {
    c.bench_function("indexing_small_py_10_files", |b| {
        b.iter_batched(
            || {
                let gen = FixtureGenerator::new().expect("Failed to create generator");
                gen.generate_small_py_project()
                    .expect("Failed to generate project")
            },
            |project_path| black_box(count_project_files(&project_path)),
            criterion::BatchSize::SmallInput,
        )
    });
}

fn indexing_large_py(c: &mut Criterion) {
    c.bench_function("indexing_large_py_500_files", |b| {
        b.iter_batched(
            || {
                let gen = FixtureGenerator::new().expect("Failed to create generator");
                gen.generate_large_py_project()
                    .expect("Failed to generate project")
            },
            |project_path| black_box(count_project_files(&project_path)),
            criterion::BatchSize::SmallInput,
        )
    });
}

criterion_group!(
    benches,
    indexing_small_ts,
    indexing_medium_ts,
    indexing_small_py,
    indexing_large_py
);
criterion_main!(benches);
