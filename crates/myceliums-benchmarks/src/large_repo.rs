use anyhow::Result;
use std::fs;
use std::path::PathBuf;

/// Generator for a synthetic large multi-language project (5,000+ files).
///
/// Creates a realistic directory structure with cross-file imports,
/// class hierarchies, and function call chains across 6 languages:
/// TypeScript, Python, Go, Rust, Java, and C#.
pub struct LargeRepoGenerator {
    root: PathBuf,
}

impl LargeRepoGenerator {
    /// Create a generator targeting the given directory.
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    /// Generate the full synthetic project.
    ///
    /// Returns the root path and a summary of what was created.
    pub fn generate(&self) -> Result<LargeRepoSummary> {
        fs::create_dir_all(&self.root)?;

        let mut total_files = 0u32;

        // --- TypeScript (1,200 files) ---
        total_files += self.generate_typescript(1_200)?;

        // --- Python (1,200 files) ---
        total_files += self.generate_python(1_200)?;

        // --- Go (800 files) ---
        total_files += self.generate_go(800)?;

        // --- Rust (800 files) ---
        total_files += self.generate_rust(800)?;

        // --- Java (600 files) ---
        total_files += self.generate_java(600)?;

        // --- C# (400 files) ---
        total_files += self.generate_csharp(400)?;

        Ok(LargeRepoSummary {
            root: self.root.clone(),
            total_files,
        })
    }

    // -------------------------------------------------------------------------
    // TypeScript
    // -------------------------------------------------------------------------
    fn generate_typescript(&self, count: u32) -> Result<u32> {
        let modules = [
            "api",
            "auth",
            "billing",
            "cache",
            "config",
            "controllers",
            "database",
            "email",
            "events",
            "graphql",
            "jobs",
            "logging",
            "middleware",
            "models",
            "notifications",
            "queue",
            "routes",
            "services",
            "utils",
            "validators",
        ];

        let base = self.root.join("packages/ts-core/src");
        for m in &modules {
            fs::create_dir_all(base.join(m))?;
        }

        // Shared types file that many modules import
        fs::write(
            base.join("types.ts"),
            r#"export interface User {
  id: string;
  name: string;
  email: string;
  role: UserRole;
}

export type UserRole = "admin" | "editor" | "viewer";

export interface Config {
  port: number;
  dbUrl: string;
  redisUrl: string;
  logLevel: "debug" | "info" | "warn" | "error";
}

export interface PaginatedResult<T> {
  items: T[];
  total: number;
  page: number;
  pageSize: number;
}

export interface ApiError {
  code: string;
  message: string;
  details?: Record<string, unknown>;
}
"#,
        )?;

        // Index barrel file
        fs::write(
            base.join("index.ts"),
            r#"export * from "./types";
export { createApp } from "./api/app_0";
"#,
        )?;

        let mut created = 2u32; // types.ts + index.ts

        for i in 0..count.saturating_sub(2) {
            let module = modules[i as usize % modules.len()];
            let file_idx = i / modules.len() as u32;
            let filename = format!("{module}_{file_idx}.ts");
            let prev_module = modules[(i as usize).wrapping_sub(1) % modules.len()];
            let prev_idx = if file_idx > 0 { file_idx - 1 } else { 0 };

            let content = format!(
                r#"import {{ User, Config }} from "../types";
import {{ func_{prev}_{pi}_process }} from "../{prev}/{prev}_{pi}";

export interface {Name}Input {{
  userId: string;
  payload: Record<string, unknown>;
}}

export interface {Name}Output {{
  success: boolean;
  data: unknown;
}}

export class {Name}Service {{
  private config: Config;

  constructor(config: Config) {{
    this.config = config;
  }}

  async process(input: {Name}Input): Promise<{Name}Output> {{
    const result = func_{mod}_{idx}_process(input.userId);
    return {{ success: true, data: result }};
  }}

  async validate(user: User): Promise<boolean> {{
    return user.role === "admin";
  }}
}}

export function func_{mod}_{idx}_process(id: string): string {{
  return `processed:${{id}}`;
}}

export function func_{mod}_{idx}_transform(data: unknown): unknown {{
  return data;
}}

export function func_{mod}_{idx}_validate(input: string): boolean {{
  return input.length > 0;
}}
"#,
                Name = capitalize(module),
                mod = module,
                idx = file_idx,
                prev = prev_module,
                pi = prev_idx,
            );

            fs::write(base.join(module).join(&filename), content)?;
            created += 1;
        }

        Ok(created)
    }

    // -------------------------------------------------------------------------
    // Python
    // -------------------------------------------------------------------------
    fn generate_python(&self, count: u32) -> Result<u32> {
        let packages = [
            "api",
            "auth",
            "billing",
            "cache",
            "config",
            "controllers",
            "database",
            "email",
            "events",
            "jobs",
            "logging_pkg",
            "middleware",
            "models",
            "notifications",
            "queue",
            "routes",
            "services",
            "utils",
            "validators",
            "workers",
        ];

        let base = self.root.join("packages/py-core/src");
        for pkg in &packages {
            let pkg_dir = base.join(pkg);
            fs::create_dir_all(&pkg_dir)?;
            fs::write(pkg_dir.join("__init__.py"), "")?;
        }
        fs::write(base.join("__init__.py"), "")?;

        // Shared base classes
        fs::write(
            base.join("base.py"),
            r#"from dataclasses import dataclass
from typing import Generic, TypeVar, List, Optional
from abc import ABC, abstractmethod

T = TypeVar("T")

@dataclass
class User:
    id: str
    name: str
    email: str
    role: str

@dataclass
class PaginatedResult(Generic[T]):
    items: List[T]
    total: int
    page: int
    page_size: int

class BaseService(ABC):
    @abstractmethod
    def process(self, user_id: str) -> str:
        ...

    @abstractmethod
    def validate(self, user: User) -> bool:
        ...

class BaseRepository(ABC):
    @abstractmethod
    def find_by_id(self, id: str) -> Optional[User]:
        ...

    @abstractmethod
    def save(self, user: User) -> None:
        ...
"#,
        )?;

        let mut created = packages.len() as u32 + 2; // __init__.py files + base.py + root __init__

        for i in 0..count.saturating_sub(created) {
            let pkg = packages[i as usize % packages.len()];
            let file_idx = i / packages.len() as u32;
            let filename = format!("module_{file_idx}.py");
            let prev_pkg = packages[(i as usize).wrapping_sub(1) % packages.len()];
            let prev_idx = if file_idx > 0 { file_idx - 1 } else { 0 };

            let content = format!(
                r#"from src.base import User, BaseService
from src.{prev_pkg}.module_{prev_idx} import func_{prev_pkg}_{prev_idx}_process


class {Name}Service_{idx}(BaseService):
    """Service for {pkg} module {idx}."""

    def __init__(self, config: dict):
        self.config = config

    def process(self, user_id: str) -> str:
        result = func_{pkg}_{idx}_process(user_id)
        return f"processed:{{result}}"

    def validate(self, user: User) -> bool:
        return user.role == "admin"


def func_{pkg}_{idx}_process(id: str) -> str:
    return f"result:{{id}}"


def func_{pkg}_{idx}_transform(data: dict) -> dict:
    return {{**data, "transformed": True}}


def func_{pkg}_{idx}_validate(value: str) -> bool:
    return len(value) > 0


class {Name}Handler_{idx}:
    def __init__(self, service: {Name}Service_{idx}):
        self.service = service

    def handle(self, request: dict) -> dict:
        user_id = request.get("user_id", "")
        result = self.service.process(user_id)
        return {{"status": "ok", "data": result}}
"#,
                Name = capitalize(pkg),
                pkg = pkg,
                idx = file_idx,
                prev_pkg = prev_pkg,
                prev_idx = prev_idx,
            );

            fs::write(base.join(pkg).join(&filename), content)?;
            created += 1;
        }

        Ok(created)
    }

    // -------------------------------------------------------------------------
    // Go
    // -------------------------------------------------------------------------
    fn generate_go(&self, count: u32) -> Result<u32> {
        let packages = [
            "api",
            "auth",
            "cache",
            "config",
            "db",
            "handlers",
            "middleware",
            "models",
            "services",
            "utils",
        ];

        let base = self.root.join("packages/go-core");
        fs::create_dir_all(&base)?;

        // go.mod
        fs::write(
            base.join("go.mod"),
            "module github.com/example/large-project\n\ngo 1.21\n",
        )?;

        for pkg in &packages {
            fs::create_dir_all(base.join(pkg))?;
        }

        let mut created = 1u32; // go.mod

        for i in 0..count.saturating_sub(1) {
            let pkg = packages[i as usize % packages.len()];
            let file_idx = i / packages.len() as u32;
            let filename = format!("{pkg}_{file_idx}.go");

            let content = format!(
                r#"package {pkg}

import "fmt"

// {Name}Service{idx} handles {pkg} operations.
type {Name}Service{idx} struct {{
	Config map[string]string
}}

// Process runs the main {pkg} logic for module {idx}.
func (s *{Name}Service{idx}) Process(userID string) (string, error) {{
	result := Func{Name}{idx}Process(userID)
	return fmt.Sprintf("processed:%s", result), nil
}}

// Validate checks if the input is valid.
func (s *{Name}Service{idx}) Validate(input string) bool {{
	return len(input) > 0
}}

// Func{Name}{idx}Process is a standalone processing function.
func Func{Name}{idx}Process(id string) string {{
	return fmt.Sprintf("result:%s", id)
}}

// Func{Name}{idx}Transform transforms the data.
func Func{Name}{idx}Transform(data map[string]interface{{}}) map[string]interface{{}} {{
	data["transformed"] = true
	return data
}}
"#,
                Name = capitalize(pkg),
                pkg = pkg,
                idx = file_idx,
            );

            fs::write(base.join(pkg).join(&filename), content)?;
            created += 1;
        }

        Ok(created)
    }

    // -------------------------------------------------------------------------
    // Rust
    // -------------------------------------------------------------------------
    fn generate_rust(&self, count: u32) -> Result<u32> {
        let modules = [
            "api",
            "auth",
            "cache",
            "config",
            "db",
            "handlers",
            "middleware",
            "models",
            "services",
            "utils",
        ];

        let base = self.root.join("packages/rs-core/src");
        for m in &modules {
            fs::create_dir_all(base.join(m))?;
        }

        // Cargo.toml
        fs::write(
            self.root.join("packages/rs-core/Cargo.toml"),
            r#"[package]
name = "large-project"
version = "0.1.0"
edition = "2021"
"#,
        )?;

        // lib.rs
        let mod_decls: String = modules.iter().map(|m| format!("pub mod {m};\n")).collect();
        fs::write(base.join("lib.rs"), mod_decls)?;

        let mut created = 2u32; // Cargo.toml + lib.rs

        for i in 0..count.saturating_sub(2) {
            let module = modules[i as usize % modules.len()];
            let file_idx = i / modules.len() as u32;
            let filename = format!("{module}_{file_idx}.rs");

            let content = format!(
                r#"//! Module {module} #{idx}

use std::collections::HashMap;

/// Service for {module} operations (instance {idx}).
pub struct {Name}Service{idx} {{
    config: HashMap<String, String>,
}}

impl {Name}Service{idx} {{
    pub fn new() -> Self {{
        Self {{
            config: HashMap::new(),
        }}
    }}

    pub fn process(&self, user_id: &str) -> String {{
        let result = func_{module}_{idx}_process(user_id);
        format!("processed:{{}}", result)
    }}

    pub fn validate(&self, input: &str) -> bool {{
        !input.is_empty()
    }}
}}

pub fn func_{module}_{idx}_process(id: &str) -> String {{
    format!("result:{{}}", id)
}}

pub fn func_{module}_{idx}_transform(data: HashMap<String, String>) -> HashMap<String, String> {{
    let mut out = data;
    out.insert("transformed".into(), "true".into());
    out
}}

#[cfg(test)]
mod tests {{
    use super::*;

    #[test]
    fn test_process() {{
        let svc = {Name}Service{idx}::new();
        assert_eq!(svc.process("42"), "processed:result:42");
    }}
}}
"#,
                Name = capitalize(module),
                module = module,
                idx = file_idx,
            );

            fs::write(base.join(module).join(&filename), content)?;
            created += 1;
        }

        // mod.rs files for each module directory
        for m in &modules {
            let dir = base.join(m);
            let mod_content: String = (0..count.saturating_sub(2) / modules.len() as u32)
                .map(|idx| format!("pub mod {m}_{idx};\n"))
                .collect();
            fs::write(dir.join("mod.rs"), mod_content)?;
            created += 1;
        }

        Ok(created)
    }

    // -------------------------------------------------------------------------
    // Java
    // -------------------------------------------------------------------------
    fn generate_java(&self, count: u32) -> Result<u32> {
        let packages = [
            "api",
            "auth",
            "cache",
            "config",
            "db",
            "handlers",
            "middleware",
            "models",
            "services",
            "utils",
        ];

        let base = self
            .root
            .join("packages/java-core/src/main/java/com/example");
        for pkg in &packages {
            fs::create_dir_all(base.join(pkg))?;
        }

        let mut created = 0u32;

        for i in 0..count {
            let pkg = packages[i as usize % packages.len()];
            let file_idx = i / packages.len() as u32;
            let class_name = format!("{}Service{}", capitalize(pkg), file_idx);
            let filename = format!("{class_name}.java");

            let content = format!(
                r#"package com.example.{pkg};

import java.util.Map;
import java.util.HashMap;

/**
 * Service for {pkg} operations (instance {idx}).
 */
public class {class_name} {{

    private Map<String, String> config;

    public {class_name}() {{
        this.config = new HashMap<>();
    }}

    public String process(String userId) {{
        String result = func{Name}{idx}Process(userId);
        return "processed:" + result;
    }}

    public boolean validate(String input) {{
        return input != null && !input.isEmpty();
    }}

    public static String func{Name}{idx}Process(String id) {{
        return "result:" + id;
    }}

    public static Map<String, Object> func{Name}{idx}Transform(Map<String, Object> data) {{
        data.put("transformed", true);
        return data;
    }}
}}
"#,
                class_name = class_name,
                Name = capitalize(pkg),
                pkg = pkg,
                idx = file_idx,
            );

            fs::write(base.join(pkg).join(&filename), content)?;
            created += 1;
        }

        Ok(created)
    }

    // -------------------------------------------------------------------------
    // C#
    // -------------------------------------------------------------------------
    fn generate_csharp(&self, count: u32) -> Result<u32> {
        let namespaces = [
            "Api",
            "Auth",
            "Cache",
            "Config",
            "Database",
            "Handlers",
            "Middleware",
            "Models",
            "Services",
            "Utils",
        ];

        let base = self.root.join("packages/cs-core/src");
        for ns in &namespaces {
            fs::create_dir_all(base.join(ns))?;
        }

        let mut created = 0u32;

        for i in 0..count {
            let ns = namespaces[i as usize % namespaces.len()];
            let file_idx = i / namespaces.len() as u32;
            let class_name = format!("{ns}Service{file_idx}");
            let filename = format!("{class_name}.cs");

            let content = format!(
                r#"using System;
using System.Collections.Generic;

namespace LargeProject.{ns}
{{
    /// <summary>
    /// Service for {ns} operations (instance {idx}).
    /// </summary>
    public class {class_name}
    {{
        private Dictionary<string, string> _config;

        public {class_name}()
        {{
            _config = new Dictionary<string, string>();
        }}

        public string Process(string userId)
        {{
            var result = Func{ns}{idx}Process(userId);
            return $"processed:{{result}}";
        }}

        public bool Validate(string input)
        {{
            return !string.IsNullOrEmpty(input);
        }}

        public static string Func{ns}{idx}Process(string id)
        {{
            return $"result:{{id}}";
        }}

        public static Dictionary<string, object> Func{ns}{idx}Transform(Dictionary<string, object> data)
        {{
            data["transformed"] = true;
            return data;
        }}
    }}
}}
"#,
                class_name = class_name,
                ns = ns,
                idx = file_idx,
            );

            fs::write(base.join(ns).join(&filename), content)?;
            created += 1;
        }

        Ok(created)
    }
}

/// Summary of the generated large repo.
pub struct LargeRepoSummary {
    pub root: PathBuf,
    pub total_files: u32,
}

/// Helper to capitalise the first letter of a string.
fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        None => String::new(),
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_generate_large_repo() -> Result<()> {
        let tmp = TempDir::new()?;
        let gen = LargeRepoGenerator::new(tmp.path().join("large-project"));
        let summary = gen.generate()?;
        assert!(
            summary.total_files >= 5_000,
            "Expected 5000+ files, got {}",
            summary.total_files
        );
        assert!(summary.root.exists());
        Ok(())
    }

    #[test]
    fn test_capitalize() {
        assert_eq!(capitalize("hello"), "Hello");
        assert_eq!(capitalize("api"), "Api");
        assert_eq!(capitalize(""), "");
    }
}
