//! Entity type definitions and schema validation using YAML-based ontology.
//!
//! This module provides an ontology system for defining entity types (nodes) and
//! relationship types (edges) in the knowledge graph. Entity types define properties
//! that code symbols must have, enabling schema-driven validation and querying.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

/// Property definition for entity types.
///
/// Describes a single property that can be part of an entity type's schema.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Property {
    /// Property name (e.g., "name", "file_path", "return_type")
    pub name: String,

    /// Data type (e.g., "string", "integer", "boolean", "array")
    #[serde(rename = "type")]
    pub prop_type: String,

    /// Whether this property is required
    #[serde(default)]
    pub required: bool,

    /// Optional description of the property
    #[serde(default)]
    pub description: Option<String>,

    /// Optional constraints or validation rules
    #[serde(default)]
    pub constraints: Option<HashMap<String, String>>,
}

/// Entity type definition (node type in the knowledge graph).
///
/// Defines the structure and properties of a code symbol type.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntityType {
    /// Entity type name (e.g., "Function", "Class", "Method")
    pub name: String,

    /// Human-readable description
    pub description: String,

    /// List of properties that define this entity type's schema
    pub properties: Vec<Property>,

    /// Whether this entity type is abstract (cannot be instantiated directly)
    #[serde(default)]
    pub abstract_type: bool,

    /// Parent entity type (for inheritance)
    #[serde(default)]
    pub extends: Option<String>,

    /// Tags for categorization (e.g., ["callable", "definition"])
    #[serde(default)]
    pub tags: Vec<String>,
}

/// Relationship type definition (edge type in the knowledge graph).
///
/// Defines the structure and properties of a code relationship.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EdgeType {
    /// Relationship type name (e.g., "CALLS", "CONTAINS", "IMPORTS")
    pub name: String,

    /// Human-readable description
    pub description: String,

    /// Source node type(s) this edge can originate from
    pub from_types: Vec<String>,

    /// Target node type(s) this edge can point to
    pub to_types: Vec<String>,

    /// List of properties for this relationship type
    #[serde(default)]
    pub properties: Vec<Property>,

    /// Whether this relationship is directed
    #[serde(default = "default_directed")]
    pub directed: bool,

    /// Tags for categorization (e.g., ["definition", "static"])
    #[serde(default)]
    pub tags: Vec<String>,
}

fn default_directed() -> bool {
    true
}

/// Complete ontology containing all entity and relationship type definitions.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Ontology {
    /// Map of entity type name to EntityType definition
    #[serde(default)]
    pub entities: HashMap<String, EntityType>,

    /// Map of edge type name to EdgeType definition
    #[serde(default)]
    pub edges: HashMap<String, EdgeType>,

    /// Version of the ontology schema
    #[serde(default)]
    pub version: String,

    /// Namespace for the ontology (e.g., "mycelium.core")
    #[serde(default)]
    pub namespace: String,
}

impl Ontology {
    /// Create a new empty ontology.
    pub fn new() -> Self {
        Self::default()
    }

    /// Load ontology from a YAML file.
    pub fn load_from_file(path: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        let contents = std::fs::read_to_string(path)?;
        Self::load_from_yaml(&contents)
    }

    /// Load ontology from YAML string.
    pub fn load_from_yaml(yaml_str: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let ontology: Ontology = serde_yaml::from_str(yaml_str)?;
        Ok(ontology)
    }

    /// Load and merge multiple ontology files from a directory.
    pub fn load_from_directory(dir: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        let mut ontology = Ontology::default();

        // Load entity types from nodes directory
        let nodes_dir = dir.join("nodes");
        if nodes_dir.exists() {
            for entry in std::fs::read_dir(&nodes_dir)? {
                let entry = entry?;
                let path = entry.path();
                if path
                    .extension()
                    .is_some_and(|ext| ext == "yaml" || ext == "yml")
                {
                    let entity = Self::load_entity_from_file(&path)?;
                    ontology.entities.insert(entity.name.clone(), entity);
                }
            }
        }

        // Load edge types from edges directory
        let edges_dir = dir.join("edges");
        if edges_dir.exists() {
            for entry in std::fs::read_dir(&edges_dir)? {
                let entry = entry?;
                let path = entry.path();
                if path
                    .extension()
                    .is_some_and(|ext| ext == "yaml" || ext == "yml")
                {
                    let edge = Self::load_edge_from_file(&path)?;
                    ontology.edges.insert(edge.name.clone(), edge);
                }
            }
        }

        Ok(ontology)
    }

    /// Load a single entity type from a YAML file.
    fn load_entity_from_file(path: &Path) -> Result<EntityType, Box<dyn std::error::Error>> {
        let contents = std::fs::read_to_string(path)?;
        let entity: EntityType = serde_yaml::from_str(&contents)?;
        Ok(entity)
    }

    /// Load a single edge type from a YAML file.
    fn load_edge_from_file(path: &Path) -> Result<EdgeType, Box<dyn std::error::Error>> {
        let contents = std::fs::read_to_string(path)?;
        let edge: EdgeType = serde_yaml::from_str(&contents)?;
        Ok(edge)
    }

    /// Get an entity type definition by name.
    pub fn get_entity(&self, name: &str) -> Option<&EntityType> {
        self.entities.get(name)
    }

    /// Get an edge type definition by name.
    pub fn get_edge(&self, name: &str) -> Option<&EdgeType> {
        self.edges.get(name)
    }

    /// Get all entity type definitions.
    pub fn get_entities(&self) -> &HashMap<String, EntityType> {
        &self.entities
    }

    /// Get all edge type definitions.
    pub fn get_edges(&self) -> &HashMap<String, EdgeType> {
        &self.edges
    }

    /// Validate that an entity type is defined in this ontology.
    pub fn validate_entity_type(&self, entity_type: &str) -> Result<(), String> {
        if self.entities.contains_key(entity_type) {
            Ok(())
        } else {
            Err(format!("Unknown entity type: {}", entity_type))
        }
    }

    /// Validate that an edge type is defined in this ontology.
    pub fn validate_edge_type(&self, edge_type: &str) -> Result<(), String> {
        if self.edges.contains_key(edge_type) {
            Ok(())
        } else {
            Err(format!("Unknown edge type: {}", edge_type))
        }
    }

    /// Get the schema (properties) for an entity type.
    pub fn get_entity_schema(&self, entity_type: &str) -> Result<Vec<&Property>, String> {
        self.get_entity(entity_type)
            .map(|e| e.properties.iter().collect())
            .ok_or_else(|| format!("Unknown entity type: {}", entity_type))
    }

    /// Get the schema (properties) for an edge type.
    pub fn get_edge_schema(&self, edge_type: &str) -> Result<Vec<&Property>, String> {
        self.get_edge(edge_type)
            .map(|e| e.properties.iter().collect())
            .ok_or_else(|| format!("Unknown edge type: {}", edge_type))
    }

    /// Convert the ontology to YAML string.
    pub fn to_yaml(&self) -> Result<String, serde_yaml::Error> {
        serde_yaml::to_string(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_entity_type_creation() {
        let entity = EntityType {
            name: "Function".to_string(),
            description: "A function symbol".to_string(),
            properties: vec![Property {
                name: "name".to_string(),
                prop_type: "string".to_string(),
                required: true,
                description: Some("Function name".to_string()),
                constraints: None,
            }],
            abstract_type: false,
            extends: None,
            tags: vec!["callable".to_string()],
        };

        assert_eq!(entity.name, "Function");
        assert!(!entity.properties.is_empty());
    }

    #[test]
    fn test_ontology_operations() {
        let mut ontology = Ontology::new();
        let entity = EntityType {
            name: "Function".to_string(),
            description: "A function symbol".to_string(),
            properties: vec![],
            abstract_type: false,
            extends: None,
            tags: vec![],
        };
        ontology.entities.insert("Function".to_string(), entity);

        assert!(ontology.get_entity("Function").is_some());
        assert!(ontology.validate_entity_type("Function").is_ok());
        assert!(ontology.validate_entity_type("Unknown").is_err());
    }

    #[test]
    fn test_yaml_deserialization() {
        let yaml = r#"
name: Function
description: A function symbol
properties:
  - name: name
    type: string
    required: true
abstract_type: false
tags:
  - callable
"#;

        let entity: EntityType = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(entity.name, "Function");
        assert_eq!(entity.properties.len(), 1);
        assert_eq!(entity.properties[0].name, "name");
    }
}
