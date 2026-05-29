use crate::metrics::BenchmarkResults;
use anyhow::Result;
use std::fs;
use std::path::Path;

/// Generate JSON report from benchmark results
pub fn generate_json_report(results: &BenchmarkResults, output_path: &Path) -> Result<()> {
    let json = serde_json::to_string_pretty(&results)?;
    fs::write(output_path, json)?;
    Ok(())
}

/// Generate Markdown report from benchmark results
pub fn generate_markdown_report(results: &BenchmarkResults, output_path: &Path) -> Result<()> {
    let mut report = String::new();
    report.push_str("# Myceliums Benchmark Report\n\n");
    report.push_str(&format!("**Version:** {}\n", results.version));
    report.push_str(&format!("**Timestamp:** {}\n\n", results.timestamp));

    // Summary section
    if let Some(time_pct) = results.time_reduction_pct {
        report.push_str("## Performance Improvements\n\n");
        report.push_str(&format!("- ⏱️ **Time Reduction:** {:.2}%\n", time_pct));
    }
    if let Some(cost_pct) = results.cost_savings_pct {
        report.push_str(&format!("- 💰 **Cost Savings:** {:.2}%\n", cost_pct));
    }
    if let Some(calls_pct) = results.fewer_tool_calls_pct {
        report.push_str(&format!("- 🔧 **Fewer Tool Calls:** {:.2}%\n\n", calls_pct));
    }

    // Results by category
    report.push_str("## Results by Category\n\n");

    let mut categories: std::collections::BTreeMap<String, Vec<_>> =
        std::collections::BTreeMap::new();
    for result in &results.results {
        categories
            .entry(result.category.clone())
            .or_insert_with(Vec::new)
            .push(result);
    }

    for (category, cat_results) in categories {
        report.push_str(&format!("### {}\n\n", category));
        report.push_str("| Benchmark | Duration (ms) | Files | Symbols | Memory (MB) |\n");
        report.push_str("|-----------|---------------|-------|---------|-------------|\n");

        for result in cat_results {
            report.push_str(&format!(
                "| {} | {:.2} | {} | {} | {:.2} |\n",
                result.name,
                result.duration_ms,
                result.files_processed,
                result.symbols_found,
                result.memory_peak_mb
            ));
        }
        report.push('\n');
    }

    report.push_str("## Detailed Results\n\n");
    report.push_str("```json\n");
    report.push_str(&serde_json::to_string_pretty(&results)?);
    report.push_str("\n```\n");

    fs::write(output_path, report)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metrics::BenchmarkResult;
    use tempfile::TempDir;

    #[test]
    fn test_generate_json_report() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let output_path = temp_dir.path().join("report.json");

        let mut results = BenchmarkResults::new("0.1.0".to_string());
        results.add_result(BenchmarkResult {
            name: "test".to_string(),
            category: "indexing".to_string(),
            duration_ms: 100.0,
            files_processed: 10,
            symbols_found: 50,
            memory_peak_mb: 10.5,
            timestamp: chrono::Local::now().to_rfc3339(),
        });

        generate_json_report(&results, &output_path)?;
        assert!(output_path.exists());

        let content = fs::read_to_string(&output_path)?;
        assert!(content.contains("0.1.0"));

        Ok(())
    }

    #[test]
    fn test_generate_markdown_report() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let output_path = temp_dir.path().join("report.md");

        let mut results = BenchmarkResults::new("0.1.0".to_string());
        results.add_result(BenchmarkResult {
            name: "test".to_string(),
            category: "indexing".to_string(),
            duration_ms: 100.0,
            files_processed: 10,
            symbols_found: 50,
            memory_peak_mb: 10.5,
            timestamp: chrono::Local::now().to_rfc3339(),
        });
        results.set_improvements(15.5, 20.3, 10.1);

        generate_markdown_report(&results, &output_path)?;
        assert!(output_path.exists());

        let content = fs::read_to_string(&output_path)?;
        assert!(content.contains("Myceliums Benchmark Report"));
        assert!(content.contains("15.50%"));

        Ok(())
    }
}
