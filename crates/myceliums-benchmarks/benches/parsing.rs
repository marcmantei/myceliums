use criterion::{black_box, criterion_group, criterion_main, Criterion};
use myceliums_benchmarks::fixtures::FixtureGenerator;
use myceliums_core::parser::{SourceLanguage, SourceParser};
use std::fs;
use walkdir::WalkDir;

/// Collect all source files from a directory with their language and content.
fn collect_source_files(dir: &std::path::Path) -> Vec<(SourceLanguage, String)> {
    WalkDir::new(dir)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file())
        .filter_map(|e| {
            let ext = e.path().extension()?.to_str()?;
            let lang = SourceLanguage::from_extension(ext)?;
            if lang.is_content() {
                return None;
            }
            let content = fs::read_to_string(e.path()).ok()?;
            Some((lang, content))
        })
        .collect()
}

fn bench_parse_handcoded(c: &mut Criterion) {
    let gen = FixtureGenerator::new().expect("fixture gen");
    let project = gen.generate_small_ts_project().expect("generate small ts");

    let files = collect_source_files(&project);

    c.bench_function("parse_handcoded_small_ts", |b| {
        b.iter(|| {
            for (lang, source) in &files {
                let mut parser = SourceParser::new(*lang).unwrap();
                let result = parser.parse(black_box(source)).unwrap();
                black_box(&result);
            }
        })
    });
}

fn bench_parse_dsl(c: &mut Criterion) {
    let gen = FixtureGenerator::new().expect("fixture gen");
    let project = gen.generate_small_ts_project().expect("generate small ts");

    let files = collect_source_files(&project);

    c.bench_function("parse_dsl_small_ts", |b| {
        b.iter(|| {
            for (lang, source) in &files {
                let mut parser = SourceParser::new(*lang).unwrap();
                let result = parser.parse_with_dsl(black_box(source)).unwrap();
                black_box(&result);
            }
        })
    });
}

fn bench_parse_python_handcoded(c: &mut Criterion) {
    let source = r#"
import os
from typing import Optional, List
from dataclasses import dataclass

@dataclass
class UserService:
    db: object
    cache: object

    def get_user(self, user_id: int) -> Optional[dict]:
        cached = self.cache.get(user_id)
        if cached:
            return cached
        user = self.db.query("SELECT * FROM users WHERE id = ?", user_id)
        self.cache.set(user_id, user)
        return user

    def list_users(self, page: int = 1) -> List[dict]:
        return self.db.query("SELECT * FROM users LIMIT ? OFFSET ?", 20, (page - 1) * 20)

    def delete_user(self, user_id: int) -> bool:
        self.cache.invalidate(user_id)
        return self.db.execute("DELETE FROM users WHERE id = ?", user_id)

class AuthService:
    def __init__(self, user_service: UserService):
        self.user_service = user_service

    def authenticate(self, username: str, password: str) -> Optional[dict]:
        user = self.user_service.get_user(username)
        if user and verify_password(password, user['password_hash']):
            return create_token(user)
        return None

    def refresh_token(self, token: str) -> Optional[str]:
        payload = decode_token(token)
        if payload:
            return create_token(payload)
        return None

def verify_password(password: str, hash: str) -> bool:
    return hash == hashlib.sha256(password.encode()).hexdigest()

def create_token(user: dict) -> str:
    return jwt.encode(user, os.environ['SECRET_KEY'])

def decode_token(token: str) -> Optional[dict]:
    try:
        return jwt.decode(token, os.environ['SECRET_KEY'])
    except Exception:
        return None
"#;

    c.bench_function("parse_python_handcoded", |b| {
        b.iter(|| {
            let mut parser = SourceParser::new(SourceLanguage::Python).unwrap();
            let result = parser.parse(black_box(source)).unwrap();
            black_box(&result);
        })
    });
}

fn bench_parse_python_dsl(c: &mut Criterion) {
    let source = r#"
import os
from typing import Optional, List
from dataclasses import dataclass

@dataclass
class UserService:
    db: object
    cache: object

    def get_user(self, user_id: int) -> Optional[dict]:
        cached = self.cache.get(user_id)
        if cached:
            return cached
        user = self.db.query("SELECT * FROM users WHERE id = ?", user_id)
        self.cache.set(user_id, user)
        return user

    def list_users(self, page: int = 1) -> List[dict]:
        return self.db.query("SELECT * FROM users LIMIT ? OFFSET ?", 20, (page - 1) * 20)

    def delete_user(self, user_id: int) -> bool:
        self.cache.invalidate(user_id)
        return self.db.execute("DELETE FROM users WHERE id = ?", user_id)

class AuthService:
    def __init__(self, user_service: UserService):
        self.user_service = user_service

    def authenticate(self, username: str, password: str) -> Optional[dict]:
        user = self.user_service.get_user(username)
        if user and verify_password(password, user['password_hash']):
            return create_token(user)
        return None

def verify_password(password: str, hash: str) -> bool:
    return hash == hashlib.sha256(password.encode()).hexdigest()

def create_token(user: dict) -> str:
    return jwt.encode(user, os.environ['SECRET_KEY'])
"#;

    c.bench_function("parse_python_dsl", |b| {
        b.iter(|| {
            let mut parser = SourceParser::new(SourceLanguage::Python).unwrap();
            let result = parser.parse_with_dsl(black_box(source)).unwrap();
            black_box(&result);
        })
    });
}

criterion_group!(
    benches,
    bench_parse_handcoded,
    bench_parse_dsl,
    bench_parse_python_handcoded,
    bench_parse_python_dsl,
);
criterion_main!(benches);
