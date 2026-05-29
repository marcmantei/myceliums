use std::path::Path;
use walkdir::WalkDir;

/// Benchmark infrastructure module
pub mod baseline;
pub mod fixtures;
pub mod large_repo;
pub mod metrics;
pub mod report;
pub mod scenarios;

pub use baseline::*;
pub use fixtures::*;
pub use large_repo::*;
pub use metrics::*;
pub use report::*;
pub use scenarios::*;

/// Count the number of files in a directory recursively
pub fn count_files(path: &Path) -> usize {
    WalkDir::new(path)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file())
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_count_files() {
        // Basic test that the function works
        let count = count_files(Path::new("."));
        assert!(count > 0);
    }
}
