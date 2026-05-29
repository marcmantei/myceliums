use anyhow::Result;
use std::fs;
use std::path::PathBuf;
use tempfile::TempDir;

/// Test fixture generator for creating synthetic projects
pub struct FixtureGenerator {
    temp_dir: TempDir,
}

impl FixtureGenerator {
    /// Create a new fixture generator
    pub fn new() -> Result<Self> {
        Ok(Self {
            temp_dir: TempDir::new()?,
        })
    }

    /// Get the root path of the temporary directory
    pub fn root(&self) -> PathBuf {
        self.temp_dir.path().to_path_buf()
    }

    /// Generate a small TypeScript project (10 files)
    pub fn generate_small_ts_project(&self) -> Result<PathBuf> {
        let project_dir = self.root().join("small-ts-project");
        fs::create_dir_all(&project_dir)?;

        // Create src directory structure
        let src_dir = project_dir.join("src");
        fs::create_dir_all(&src_dir)?;

        // utils.ts - 5 functions
        fs::write(
            src_dir.join("utils.ts"),
            r#"export function formatString(str: string): string {
  return str.trim().toLowerCase();
}

export function parseJSON(data: string): object {
  return JSON.parse(data);
}

export function cloneObject(obj: any): any {
  return JSON.parse(JSON.stringify(obj));
}

export function debounce(fn: Function, delay: number): Function {
  let timeout: NodeJS.Timeout;
  return function(...args: any[]) {
    clearTimeout(timeout);
    timeout = setTimeout(() => fn(...args), delay);
  };
}

export function throttle(fn: Function, limit: number): Function {
  let inThrottle: boolean;
  return function(...args: any[]) {
    if (!inThrottle) {
      fn(...args);
      inThrottle = true;
      setTimeout(() => inThrottle = false, limit);
    }
  };
}
"#,
        )?;

        // db.ts - 4 functions
        fs::write(
            src_dir.join("db.ts"),
            r#"export function connect(url: string): any {
  return { connected: true, url };
}

export function query(sql: string): Promise<any[]> {
  return Promise.resolve([]);
}

export function insert(table: string, data: any): Promise<void> {
  return Promise.resolve();
}

export function update(table: string, id: number, data: any): Promise<void> {
  return Promise.resolve();
}
"#,
        )?;

        // services/user.ts - 3 functions
        let services_dir = src_dir.join("services");
        fs::create_dir_all(&services_dir)?;
        fs::write(
            services_dir.join("user.ts"),
            r#"import { query, insert, update } from "../db";
import { formatString } from "../utils";

export async function getUser(id: number): Promise<any> {
  return await query(`SELECT * FROM users WHERE id = ${id}`);
}

export async function createUser(name: string, email: string): Promise<void> {
  const cleanName = formatString(name);
  await insert("users", { name: cleanName, email });
}

export async function updateUser(id: number, data: any): Promise<void> {
  await update("users", id, data);
}
"#,
        )?;

        // services/auth.ts - 2 functions
        fs::write(
            services_dir.join("auth.ts"),
            r#"import { getUser } from "./user";

export async function authenticate(email: string): Promise<boolean> {
  const user = await getUser(1);
  return user !== null;
}

export function hashPassword(password: string): string {
  return Buffer.from(password).toString("base64");
}
"#,
        )?;

        // index.ts - 1 function
        fs::write(
            src_dir.join("index.ts"),
            r#"import { createUser, getUser } from "./services/user";
import { authenticate } from "./services/auth";

export async function initializeApp(): Promise<void> {
  const user = await getUser(1);
  if (!user) {
    await createUser("admin", "admin@example.com");
  }
}

export { getUser, createUser, authenticate };
"#,
        )?;

        // Add 4 more simple files to reach 10 files
        fs::write(
            src_dir.join("constants.ts"),
            r#"export const APP_NAME = "MyApp";
export const VERSION = "1.0.0";
export const DEBUG = true;
"#,
        )?;

        fs::write(
            src_dir.join("types.ts"),
            r#"export interface User {
  id: number;
  name: string;
  email: string;
}

export interface Config {
  debug: boolean;
  port: number;
}
"#,
        )?;

        fs::write(
            src_dir.join("logger.ts"),
            r#"export function log(message: string): void {
  console.log(`[LOG] ${message}`);
}

export function error(message: string): void {
  console.error(`[ERROR] ${message}`);
}
"#,
        )?;

        fs::write(
            src_dir.join("config.ts"),
            r#"import { Config } from "./types";

export function loadConfig(): Config {
  return {
    debug: process.env.DEBUG === "true",
    port: parseInt(process.env.PORT || "3000"),
  };
}
"#,
        )?;

        Ok(project_dir)
    }

    /// Generate a medium TypeScript project (100 files)
    pub fn generate_medium_ts_project(&self) -> Result<PathBuf> {
        let project_dir = self.root().join("medium-ts-project");
        fs::create_dir_all(&project_dir)?;
        let src_dir = project_dir.join("src");
        fs::create_dir_all(&src_dir)?;

        // Create modular structure
        let dirs = vec![
            "services",
            "controllers",
            "models",
            "middleware",
            "utils",
            "validators",
            "decorators",
            "errors",
            "types",
            "config",
        ];

        for dir in &dirs {
            fs::create_dir_all(src_dir.join(dir))?;
        }

        // Generate 100 TypeScript files with varying complexity
        for i in 0..100 {
            let module = dirs[i % dirs.len()];
            let filename = format!("module_{}.ts", i);

            let content = format!(
                r#"// Module {}
export function func_{}_a(): string {{
  return "result_{}";
}}

export function func_{}_b(param: string): void {{
  console.log(param);
}}

export interface Type_{} {{
  id: number;
  name: string;
}}

export class Handler_{} {{
  handle(event: any): void {{
    func_{}_a();
  }}
}}
"#,
                i, i, i, i, i, i, i
            );

            fs::write(src_dir.join(module).join(&filename), content)?;
        }

        Ok(project_dir)
    }

    /// Generate a small Python project (10 files)
    pub fn generate_small_py_project(&self) -> Result<PathBuf> {
        let project_dir = self.root().join("small-py-project");
        fs::create_dir_all(&project_dir)?;

        // Create package structure
        fs::create_dir_all(project_dir.join("app"))?;
        fs::create_dir_all(project_dir.join("app/services"))?;
        fs::create_dir_all(project_dir.join("app/models"))?;

        // __init__.py files
        fs::write(project_dir.join("app/__init__.py"), "")?;
        fs::write(project_dir.join("app/services/__init__.py"), "")?;
        fs::write(project_dir.join("app/models/__init__.py"), "")?;

        // utils.py
        fs::write(
            project_dir.join("app/utils.py"),
            r#"def format_string(s: str) -> str:
    return s.strip().lower()

def parse_json(data: str) -> dict:
    import json
    return json.loads(data)

def clone_dict(d: dict) -> dict:
    return dict(d)

def retry(max_attempts: int = 3):
    def decorator(func):
        def wrapper(*args, **kwargs):
            for _ in range(max_attempts):
                try:
                    return func(*args, **kwargs)
                except Exception:
                    pass
        return wrapper
    return decorator
"#,
        )?;

        // db.py
        fs::write(
            project_dir.join("app/db.py"),
            r#"class Database:
    def __init__(self, url: str):
        self.url = url
        self.connected = False

    def connect(self):
        self.connected = True

    def query(self, sql: str):
        return []

    def insert(self, table: str, data: dict):
        pass

    def update(self, table: str, id: int, data: dict):
        pass
"#,
        )?;

        // models/user.py
        fs::write(
            project_dir.join("app/models/user.py"),
            r#"from dataclasses import dataclass

@dataclass
class User:
    id: int
    name: str
    email: str

def create_user(name: str, email: str) -> User:
    return User(id=1, name=name, email=email)
"#,
        )?;

        // services/user_service.py
        fs::write(
            project_dir.join("app/services/user_service.py"),
            r#"from app.models.user import User, create_user
from app.db import Database
from app.utils import format_string

class UserService:
    def __init__(self, db: Database):
        self.db = db

    def get_user(self, user_id: int) -> User:
        result = self.db.query(f"SELECT * FROM users WHERE id = {user_id}")
        return result[0] if result else None

    def create_user_with_validation(self, name: str, email: str):
        clean_name = format_string(name)
        user = create_user(clean_name, email)
        self.db.insert("users", {"name": user.name, "email": user.email})
        return user
"#,
        )?;

        // Add more simple files
        fs::write(
            project_dir.join("app/constants.py"),
            r#"APP_NAME = "MyApp"
VERSION = "1.0.0"
DEBUG = True
"#,
        )?;

        fs::write(
            project_dir.join("app/logger.py"),
            r#"def log(message: str):
    print(f"[LOG] {message}")

def error(message: str):
    print(f"[ERROR] {message}")
"#,
        )?;

        fs::write(
            project_dir.join("app/config.py"),
            r#"import os
from typing import Dict

def load_config() -> Dict:
    return {
        "debug": os.getenv("DEBUG", "false") == "true",
        "port": int(os.getenv("PORT", "3000")),
    }
"#,
        )?;

        fs::write(
            project_dir.join("main.py"),
            r#"from app.db import Database
from app.services.user_service import UserService

def main():
    db = Database("sqlite:///app.db")
    db.connect()
    service = UserService(db)
    user = service.create_user_with_validation("admin", "admin@example.com")
    print(f"Created user: {user}")

if __name__ == "__main__":
    main()
"#,
        )?;

        fs::write(project_dir.join("requirements.txt"), "")?;

        Ok(project_dir)
    }

    /// Generate a large Python project (500 files)
    pub fn generate_large_py_project(&self) -> Result<PathBuf> {
        let project_dir = self.root().join("large-py-project");
        fs::create_dir_all(&project_dir)?;

        let packages = vec![
            "services",
            "models",
            "controllers",
            "middleware",
            "validators",
            "utils",
            "decorators",
            "errors",
            "types",
        ];

        for pkg in &packages {
            fs::create_dir_all(project_dir.join("app").join(pkg))?;
            fs::write(project_dir.join("app").join(pkg).join("__init__.py"), "")?;
        }

        // Generate 500 Python files
        for i in 0..500 {
            let pkg = packages[i % packages.len()];
            let filename = format!("module_{}.py", i);

            let content = format!(
                r#"# Module {}
def func_{}_a():
    return "result_{}"

def func_{}_b(param):
    print(param)

class Handler_{}:
    def handle(self, event):
        func_{}_a()

def config_{}():
    return {{"id": {}}}
"#,
                i, i, i, i, i, i, i, i
            );

            fs::write(project_dir.join("app").join(pkg).join(&filename), content)?;
        }

        fs::write(
            project_dir.join("main.py"),
            "# Entry point\nif __name__ == \"__main__\":\n    print(\"Large project\")\n",
        )?;

        Ok(project_dir)
    }
}

impl Default for FixtureGenerator {
    fn default() -> Self {
        Self::new().expect("Failed to create fixture generator")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_small_ts_project() -> Result<()> {
        let gen = FixtureGenerator::new()?;
        let path = gen.generate_small_ts_project()?;
        assert!(path.exists());
        Ok(())
    }

    #[test]
    fn test_generate_small_py_project() -> Result<()> {
        let gen = FixtureGenerator::new()?;
        let path = gen.generate_small_py_project()?;
        assert!(path.exists());
        Ok(())
    }
}
