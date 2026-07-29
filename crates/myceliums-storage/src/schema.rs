use arrow_schema::{DataType, Field, Schema};
use std::sync::Arc;

/// Dimension of the legacy default embedding model (all-MiniLM-L6-v2).
/// Used when no embedding configuration or index metadata is available.
pub const DEFAULT_EMBEDDING_DIM: i32 = 384;

/// Version of the stored vector geometry.
///
/// Bumped to `2` when embeddings became L2-normalized (cosine geometry) and the
/// ANN index was activated (issue #29). An index written under an older version
/// stores raw (unnormalized) vectors whose L2 ordering differs from cosine, so
/// [`RepoInfo::vector_geometry_version`] lets callers detect a stale index and
/// trigger a rebuild rather than silently mixing geometries.
pub const VECTOR_GEOMETRY_VERSION: u32 = 2;

/// Arrow schema for the `symbols` table, with a fixed-size vector column
/// sized to `embedding_dim`.
pub fn symbols_schema(embedding_dim: i32) -> Schema {
    Schema::new(vec![
        Field::new("uid", DataType::Utf8, false),
        Field::new("name", DataType::Utf8, false),
        Field::new("qualified_name", DataType::Utf8, false),
        Field::new("kind", DataType::Utf8, false),
        Field::new("file_path", DataType::Utf8, false),
        Field::new("start_line", DataType::UInt32, false),
        Field::new("end_line", DataType::UInt32, false),
        Field::new("signature", DataType::Utf8, false),
        Field::new("content", DataType::Utf8, false),
        Field::new("repo_id", DataType::Utf8, false),
        Field::new("metadata", DataType::Utf8, true),
        Field::new(
            "vector",
            DataType::FixedSizeList(
                Arc::new(Field::new("item", DataType::Float32, true)),
                embedding_dim,
            ),
            true,
        ),
    ])
}

/// Key-value metadata about the index itself (e.g. which embedding model
/// built the vectors). Kept separate from symbol data so it survives
/// incremental updates and can be read without scanning symbols.
pub fn index_meta_schema() -> Schema {
    Schema::new(vec![
        Field::new("key", DataType::Utf8, false),
        Field::new("value", DataType::Utf8, false),
    ])
}

/// Arrow schema for the `files` table.
pub fn files_schema() -> Schema {
    Schema::new(vec![
        Field::new("uid", DataType::Utf8, false),
        Field::new("path", DataType::Utf8, false),
        Field::new("language", DataType::Utf8, false),
        Field::new("repo_id", DataType::Utf8, false),
        Field::new("num_symbols", DataType::UInt32, false),
    ])
}

/// Arrow schema for the `relationships` (edges) table.
pub fn relationships_schema() -> Schema {
    Schema::new(vec![
        Field::new("uid", DataType::Utf8, false),
        Field::new("source_uid", DataType::Utf8, false),
        Field::new("target_uid", DataType::Utf8, false),
        Field::new("kind", DataType::Utf8, false),
        Field::new("repo_id", DataType::Utf8, false),
        Field::new("metadata", DataType::Utf8, false),
    ])
}

/// Arrow schema for the `communities` table.
pub fn communities_schema() -> Schema {
    Schema::new(vec![
        Field::new("uid", DataType::Utf8, false),
        Field::new("label", DataType::Utf8, false),
        Field::new("repo_id", DataType::Utf8, false),
        Field::new("member_count", DataType::UInt32, false),
        Field::new("top_symbols", DataType::Utf8, false),
        Field::new("summary", DataType::Utf8, true),
    ])
}

/// Arrow schema for the `processes` table.
pub fn processes_schema() -> Schema {
    Schema::new(vec![
        Field::new("uid", DataType::Utf8, false),
        Field::new("name", DataType::Utf8, false),
        Field::new("repo_id", DataType::Utf8, false),
        Field::new("entry_point", DataType::Utf8, false),
        Field::new("step_count", DataType::UInt32, false),
        Field::new("description", DataType::Utf8, false),
    ])
}

/// Arrow schema for the `teams` table.
pub fn teams_schema() -> Schema {
    Schema::new(vec![
        Field::new("uid", DataType::Utf8, false),
        Field::new("name", DataType::Utf8, false),
        Field::new("owner_id", DataType::Utf8, false),
        Field::new("created_at", DataType::Utf8, false),
        Field::new("member_count", DataType::UInt32, false),
        Field::new("repo_ids", DataType::Utf8, false),
    ])
}

/// Arrow schema for the `team_members` table.
pub fn team_members_schema() -> Schema {
    Schema::new(vec![
        Field::new("uid", DataType::Utf8, false),
        Field::new("team_id", DataType::Utf8, false),
        Field::new("user_id", DataType::Utf8, false),
        Field::new("email", DataType::Utf8, false),
        Field::new("role", DataType::Utf8, false),
        Field::new("joined_at", DataType::Utf8, false),
    ])
}

/// Wraps a [`Schema`] in an [`Arc`] for sharing across LanceDB calls.
pub fn _schema_arc(schema: Schema) -> Arc<Schema> {
    Arc::new(schema)
}
