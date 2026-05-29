use criterion::{criterion_group, criterion_main, Criterion};
use myceliums_benchmarks::{
    metrics::EnvironmentInfo, scenarios::run_all_scenarios, VerifiedMetrics,
};

/// Get environment information for the benchmark
fn get_environment_info() -> EnvironmentInfo {
    let os = if cfg!(target_os = "linux") {
        "linux"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(target_os = "windows") {
        "windows"
    } else {
        "unknown"
    };

    let memory_gb = {
        #[cfg(target_os = "linux")]
        {
            std::fs::read_to_string("/proc/meminfo")
                .ok()
                .and_then(|meminfo| {
                    meminfo
                        .lines()
                        .next()
                        .and_then(|line| line.split_whitespace().nth(1))
                        .and_then(|s| s.parse::<u64>().ok())
                        .map(|kb| ((kb / 1024 / 1024) as u32).max(1))
                })
                .unwrap_or(16)
        }
        #[cfg(not(target_os = "linux"))]
        {
            16u32
        }
    };

    let rust_version = std::process::Command::new("rustc")
        .arg("--version")
        .output()
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .unwrap_or_else(|_| "unknown".to_string());

    EnvironmentInfo {
        os: os.to_string(),
        cpu_count: num_cpus::get() as u32,
        memory_gb,
        rust_version,
    }
}

/// Get the package version
fn get_package_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

/// Generate verified metrics report
fn generate_verified_metrics(_c: &mut Criterion) {
    // Run all scenarios
    let scenarios = run_all_scenarios().expect("Failed to run scenarios");

    // Get environment and version information
    let env = get_environment_info();
    let version = get_package_version();

    // Create verified metrics
    let metrics = VerifiedMetrics::new(version.clone(), env, scenarios);

    // Print the JSON
    let json = metrics
        .to_json_string()
        .expect("Failed to serialize metrics");
    println!("\n=== VERIFIED METRICS ===\n{}\n", json);

    // Save to file if in release build
    #[cfg(not(debug_assertions))]
    {
        let metrics_dir = PathBuf::from("benchmarks/metrics");
        let metrics_file = metrics_dir.join(format!("v{}.json", version));
        if let Err(e) = metrics.save_to_file(&metrics_file) {
            eprintln!("Warning: Failed to save metrics to file: {}", e);
        } else {
            println!("Metrics saved to: {}", metrics_file.display());
        }

        // Also save as latest.json for website consumption
        let latest_file = PathBuf::from("benchmarks/latest.json");
        if let Err(e) = metrics.save_to_file(&latest_file) {
            eprintln!("Warning: Failed to save latest.json: {}", e);
        } else {
            println!("Latest metrics saved to: {}", latest_file.display());
        }
    }
}

criterion_group!(benches, generate_verified_metrics);
criterion_main!(benches);
