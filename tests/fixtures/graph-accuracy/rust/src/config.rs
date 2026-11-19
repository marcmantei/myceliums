use std::collections::HashMap;

pub fn parse(text: &str) -> HashMap<String, String> {
    let pairs = text.split(';').filter_map(|pair| pair.split_once('='));
    normalize(pairs.map(|(k, v)| (k.to_string(), v.to_string())).collect())
}

fn normalize(data: HashMap<String, String>) -> HashMap<String, String> {
    data.into_iter()
        .map(|(k, v)| (k.to_lowercase(), v))
        .collect()
}

pub enum Theme {
    Light,
    Dark,
}

impl Theme {
    pub fn parse(name: &str) -> Theme {
        if name == "dark" {
            Theme::Dark
        } else {
            Theme::Light
        }
    }
}

pub struct Config {
    values: HashMap<String, String>,
}

impl Config {
    pub fn new(values: HashMap<String, String>) -> Self {
        Config { values }
    }

    pub fn load(text: &str) -> Self {
        Self::new(parse(text))
    }

    pub fn get(&self, key: &str, fallback: &str) -> String {
        self.values
            .get(key)
            .cloned()
            .unwrap_or_else(|| fallback.to_string())
    }

    pub fn theme(&self) -> Theme {
        Theme::parse(&self.get("theme", "light"))
    }
}
