use arrow_schema::{DataType, Field, Schema};
use std::sync::Arc;

pub const EMBEDDING_DIM: i32 = 384;

pub fn symbols_schema() -> Schema {
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
                EMBEDDING_DIM,
            ),
            true,
        ),
    ])
}

pub fn files_schema() -> Schema {
    Schema::new(vec![
        Field::new("uid", DataType::Utf8, false),
        Field::new("path", DataType::Utf8, false),
        Field::new("language", DataType::Utf8, false),
        Field::new("repo_id", DataType::Utf8, false),
        Field::new("num_symbols", DataType::UInt32, false),
    ])
}

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

pub fn _schema_arc(schema: Schema) -> Arc<Schema> {
    Arc::new(schema)
}
