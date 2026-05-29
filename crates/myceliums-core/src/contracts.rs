//! API contract awareness — detect and link spec files to code symbols.
//!
//! Discovers OpenAPI and Protobuf spec files in a repository and matches
//! their endpoints to handler symbols in the knowledge graph.

use myceliums_storage::CodeSymbol;
use regex::Regex;
use serde::Serialize;
use std::path::{Path, PathBuf};

/// A single API endpoint from a spec file.
#[derive(Debug, Clone, Serialize)]
pub struct ContractEndpoint {
    pub method: String,
    pub path: String,
    pub operation_id: Option<String>,
}

/// A link between an endpoint and a handler symbol.
#[derive(Debug, Clone, Serialize)]
pub struct HandlerLink {
    pub endpoint_path: String,
    pub handler_name: String,
    pub handler_file: String,
    pub confidence: f64,
}

/// A detected API contract (one per spec file).
#[derive(Debug, Clone, Serialize)]
pub struct ApiContract {
    pub spec_file: String,
    pub spec_type: String,
    pub endpoints: Vec<ContractEndpoint>,
    pub handler_links: Vec<HandlerLink>,
}

/// Summary of all detected contracts.
#[derive(Debug, Clone, Serialize)]
pub struct ContractsReport {
    pub contracts: Vec<ApiContract>,
    pub total_endpoints: usize,
    pub linked_count: usize,
    pub unlinked_endpoints: Vec<String>,
}

/// Detect spec files in a repository by filename conventions.
pub fn detect_spec_files(repo_path: &Path) -> Vec<(PathBuf, String)> {
    let mut results = Vec::new();
    let openapi_names = [
        "openapi.yaml",
        "openapi.yml",
        "openapi.json",
        "swagger.yaml",
        "swagger.yml",
        "swagger.json",
    ];

    let entries: Vec<walkdir::DirEntry> = walkdir::WalkDir::new(repo_path)
        .max_depth(5)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .collect();

    for entry in entries {
        let name = entry.file_name().to_string_lossy().to_string();
        let path_str = entry.path().to_string_lossy().to_string();

        // Skip common dependency directories
        if path_str.contains("node_modules")
            || path_str.contains(".git/")
            || path_str.contains("vendor/")
            || path_str.contains("target/")
        {
            continue;
        }

        if openapi_names.iter().any(|n| name == *n) {
            results.push((entry.path().to_path_buf(), "openapi".to_string()));
        } else if name.ends_with(".proto") {
            results.push((entry.path().to_path_buf(), "protobuf".to_string()));
        }
    }

    results
}

/// Parse an OpenAPI spec (YAML/JSON) and extract endpoints.
pub fn parse_openapi_endpoints(content: &str) -> Vec<ContractEndpoint> {
    let mut endpoints = Vec::new();

    let doc: serde_yaml::Value = match serde_yaml::from_str(content) {
        Ok(v) => v,
        Err(_) => return endpoints,
    };

    let paths = match doc.get("paths") {
        Some(serde_yaml::Value::Mapping(m)) => m,
        _ => return endpoints,
    };

    let http_methods = ["get", "post", "put", "delete", "patch", "head", "options"];

    for (path_key, methods) in paths {
        let path = match path_key {
            serde_yaml::Value::String(s) => s.clone(),
            _ => continue,
        };

        if let serde_yaml::Value::Mapping(method_map) = methods {
            for (method_key, method_value) in method_map {
                let method = match method_key {
                    serde_yaml::Value::String(s) => s.clone(),
                    _ => continue,
                };

                if !http_methods.contains(&method.as_str()) {
                    continue;
                }

                let operation_id = method_value
                    .get("operationId")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());

                endpoints.push(ContractEndpoint {
                    method: method.to_uppercase(),
                    path: path.clone(),
                    operation_id,
                });
            }
        }
    }

    endpoints
}

/// Parse a protobuf file and extract RPC method names.
pub fn parse_proto_endpoints(content: &str) -> Vec<ContractEndpoint> {
    let mut endpoints = Vec::new();

    let re = Regex::new(r"rpc\s+(\w+)\s*\(\s*(\w+)\s*\)\s*returns\s*\(\s*(\w+)\s*\)")
        .expect("valid regex");

    for cap in re.captures_iter(content) {
        endpoints.push(ContractEndpoint {
            method: "RPC".to_string(),
            path: cap[1].to_string(),
            operation_id: Some(cap[1].to_string()),
        });
    }

    endpoints
}

/// Match endpoints to handler symbols by name similarity.
pub fn link_endpoints_to_handlers(
    endpoints: &[ContractEndpoint],
    symbols: &[CodeSymbol],
) -> Vec<HandlerLink> {
    let mut links = Vec::new();

    for endpoint in endpoints {
        let search_terms = build_search_terms(endpoint);

        let mut best_match: Option<(&CodeSymbol, f64)> = None;

        for sym in symbols {
            let sym_lower = sym.name.to_lowercase();
            let sym_snake = to_snake_case(&sym.name).to_lowercase();

            for term in &search_terms {
                let term_lower = term.to_lowercase();

                let confidence = if sym_lower == term_lower || sym_snake == term_lower {
                    1.0
                } else if sym_lower.contains(&term_lower) || sym_snake.contains(&term_lower) {
                    0.7
                } else {
                    continue;
                };

                if best_match.is_none() || confidence > best_match.unwrap().1 {
                    best_match = Some((sym, confidence));
                }
            }
        }

        if let Some((sym, confidence)) = best_match {
            links.push(HandlerLink {
                endpoint_path: endpoint.path.clone(),
                handler_name: sym.name.clone(),
                handler_file: sym.file_path.clone(),
                confidence,
            });
        }
    }

    links
}

/// Full contract detection pipeline.
pub fn detect_contracts(repo_path: &Path, symbols: &[CodeSymbol]) -> ContractsReport {
    let spec_files = detect_spec_files(repo_path);
    let mut contracts = Vec::new();
    let mut total_endpoints = 0;
    let mut total_linked = 0;
    let mut unlinked = Vec::new();

    for (path, spec_type) in spec_files {
        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(_) => continue,
        };

        let endpoints = match spec_type.as_str() {
            "openapi" => parse_openapi_endpoints(&content),
            "protobuf" => parse_proto_endpoints(&content),
            _ => continue,
        };

        let handler_links = link_endpoints_to_handlers(&endpoints, symbols);
        total_endpoints += endpoints.len();
        total_linked += handler_links.len();

        // Find unlinked endpoints
        let linked_paths: std::collections::HashSet<&str> = handler_links
            .iter()
            .map(|l| l.endpoint_path.as_str())
            .collect();
        for ep in &endpoints {
            if !linked_paths.contains(ep.path.as_str()) {
                unlinked.push(format!("{} {}", ep.method, ep.path));
            }
        }

        contracts.push(ApiContract {
            spec_file: path.to_string_lossy().to_string(),
            spec_type,
            endpoints,
            handler_links,
        });
    }

    ContractsReport {
        contracts,
        total_endpoints,
        linked_count: total_linked,
        unlinked_endpoints: unlinked,
    }
}

fn build_search_terms(endpoint: &ContractEndpoint) -> Vec<String> {
    let mut terms = Vec::new();

    if let Some(op_id) = &endpoint.operation_id {
        terms.push(op_id.clone());
        terms.push(to_snake_case(op_id));
    }

    // Extract last path segment: /api/users/{id} → "users"
    let segments: Vec<&str> = endpoint
        .path
        .split('/')
        .filter(|s| !s.is_empty() && !s.starts_with('{'))
        .collect();
    if let Some(last) = segments.last() {
        terms.push(last.to_string());
        // Combine method + resource: POST /users → create_users
        let method_prefix = match endpoint.method.as_str() {
            "POST" => "create",
            "GET" => "get",
            "PUT" | "PATCH" => "update",
            "DELETE" => "delete",
            _ => "",
        };
        if !method_prefix.is_empty() {
            terms.push(format!("{}_{}", method_prefix, last));
        }
    }

    terms
}

fn to_snake_case(s: &str) -> String {
    let mut result = String::new();
    for (i, c) in s.chars().enumerate() {
        if c.is_uppercase() && i > 0 {
            result.push('_');
        }
        result.push(c.to_lowercase().next().unwrap_or(c));
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use myceliums_storage::SymbolKind;

    fn make_symbol(name: &str, file: &str) -> CodeSymbol {
        CodeSymbol {
            uid: name.to_string(),
            name: name.to_string(),
            qualified_name: name.to_string(),
            kind: SymbolKind::Function,
            file_path: file.to_string(),
            start_line: 1,
            end_line: 10,
            signature: String::new(),
            content: String::new(),
            repo_id: "test".to_string(),
            metadata: None,
        }
    }

    #[test]
    fn test_parse_openapi() {
        let spec = r#"
openapi: "3.0.0"
paths:
  /users:
    get:
      operationId: listUsers
    post:
      operationId: createUser
  /users/{id}:
    get:
      operationId: getUser
"#;
        let endpoints = parse_openapi_endpoints(spec);
        assert_eq!(endpoints.len(), 3);
        assert!(endpoints
            .iter()
            .any(|e| e.operation_id.as_deref() == Some("listUsers")));
        assert!(endpoints
            .iter()
            .any(|e| e.operation_id.as_deref() == Some("createUser")));
    }

    #[test]
    fn test_parse_proto() {
        let proto = r#"
syntax = "proto3";
service UserService {
    rpc GetUser(GetUserRequest) returns (GetUserResponse);
    rpc CreateUser(CreateUserRequest) returns (CreateUserResponse);
}
"#;
        let endpoints = parse_proto_endpoints(proto);
        assert_eq!(endpoints.len(), 2);
        assert!(endpoints.iter().any(|e| e.path == "GetUser"));
        assert!(endpoints.iter().any(|e| e.path == "CreateUser"));
    }

    #[test]
    fn test_link_handler() {
        let endpoints = vec![ContractEndpoint {
            method: "POST".to_string(),
            path: "/users".to_string(),
            operation_id: Some("createUser".to_string()),
        }];
        let symbols = vec![
            make_symbol("create_user", "src/handlers.rs"),
            make_symbol("unrelated_fn", "src/utils.rs"),
        ];

        let links = link_endpoints_to_handlers(&endpoints, &symbols);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].handler_name, "create_user");
        assert!(links[0].confidence >= 0.7);
    }

    #[test]
    fn test_no_specs() {
        let dir = tempfile::TempDir::new().unwrap();
        let symbols = vec![make_symbol("main", "src/main.rs")];
        let report = detect_contracts(dir.path(), &symbols);
        assert!(report.contracts.is_empty());
        assert_eq!(report.total_endpoints, 0);
    }

    #[test]
    fn test_unlinked_endpoints() {
        let endpoints = vec![ContractEndpoint {
            method: "GET".to_string(),
            path: "/health".to_string(),
            operation_id: None,
        }];
        let symbols = vec![make_symbol("create_user", "src/handlers.rs")];

        let links = link_endpoints_to_handlers(&endpoints, &symbols);
        assert!(links.is_empty()); // No match for /health
    }
}
