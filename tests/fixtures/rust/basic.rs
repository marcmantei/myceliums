use std::collections::HashMap;

const MAX_RETRIES: u32 = 3;
static GLOBAL_COUNT: u32 = 0;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

fn process(input: &str) -> Result<String> {
    Ok(input.to_uppercase())
}

fn transform(data: Vec<u8>) -> Vec<u8> {
    data.into_iter().map(|b| b + 1).collect()
}

struct Config {
    host: String,
    port: u16,
    max_connections: usize,
}

enum Status {
    Active,
    Inactive,
    Error(String),
}

mod utils {
    pub fn helper() -> bool {
        true
    }
}
