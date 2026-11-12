//! Expose the locked fastembed version to the crate.
//!
//! The retrieval benchmark keys the CI model-weights cache on it (see
//! `eval::models::ModelSetup::cache_key`): fastembed's model list decides which
//! repository and files a registry entry downloads, so a fastembed upgrade can
//! change the weights even when the model ids stay the same.

fn main() {
    let lock = std::path::Path::new(&std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default())
        .join("../../Cargo.lock");
    println!("cargo:rerun-if-changed={}", lock.display());
    let version = std::fs::read_to_string(&lock)
        .ok()
        .and_then(|text| locked_version(&text, "fastembed"))
        .unwrap_or_else(|| "unknown".to_string());
    println!("cargo:rustc-env=MYC_FASTEMBED_VERSION={version}");
}

/// The `version` of the `[[package]]` named `name` in a Cargo.lock.
fn locked_version(lock: &str, name: &str) -> Option<String> {
    let wanted = format!("name = \"{name}\"");
    let mut lines = lock.lines();
    while let Some(line) = lines.next() {
        if line.trim() == wanted {
            let version = lines.next()?.trim();
            return version
                .strip_prefix("version = \"")
                .and_then(|rest| rest.strip_suffix('"'))
                .map(str::to_string);
        }
    }
    None
}
