use serde::{Deserialize, Serialize};
use std::str::FromStr;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum SymbolKind {
    Function,
    Method,
    Class,
    Interface,
    TypeAlias,
    Variable,
    Constant,
    Enum,
    Module,
    Import,
    /// A markdown/MDX heading and the prose beneath it
    Section,
    /// A whole content file (md, mdx, txt)
    Document,
    /// A design-rationale comment (NOTE:, HACK:, WHY:, TODO:, FIXME:, IMPORTANT:)
    Rationale,
    /// A single email message
    Email,
    /// A thread of emails
    Conversation,
    /// An entity extracted from From/To/CC fields
    Person,
    /// A file attached to an email
    Attachment,
}

impl std::fmt::Display for SymbolKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SymbolKind::Function => write!(f, "Function"),
            SymbolKind::Method => write!(f, "Method"),
            SymbolKind::Class => write!(f, "Class"),
            SymbolKind::Interface => write!(f, "Interface"),
            SymbolKind::TypeAlias => write!(f, "TypeAlias"),
            SymbolKind::Variable => write!(f, "Variable"),
            SymbolKind::Constant => write!(f, "Constant"),
            SymbolKind::Enum => write!(f, "Enum"),
            SymbolKind::Module => write!(f, "Module"),
            SymbolKind::Import => write!(f, "Import"),
            SymbolKind::Section => write!(f, "Section"),
            SymbolKind::Document => write!(f, "Document"),
            SymbolKind::Rationale => write!(f, "Rationale"),
            SymbolKind::Email => write!(f, "Email"),
            SymbolKind::Conversation => write!(f, "Conversation"),
            SymbolKind::Person => write!(f, "Person"),
            SymbolKind::Attachment => write!(f, "Attachment"),
        }
    }
}

impl FromStr for SymbolKind {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "Function" => SymbolKind::Function,
            "Method" => SymbolKind::Method,
            "Class" => SymbolKind::Class,
            "Interface" => SymbolKind::Interface,
            "TypeAlias" => SymbolKind::TypeAlias,
            "Variable" => SymbolKind::Variable,
            "Constant" => SymbolKind::Constant,
            "Enum" => SymbolKind::Enum,
            "Module" => SymbolKind::Module,
            "Import" => SymbolKind::Import,
            "Section" => SymbolKind::Section,
            "Document" => SymbolKind::Document,
            "Rationale" => SymbolKind::Rationale,
            "Email" => SymbolKind::Email,
            "Conversation" => SymbolKind::Conversation,
            "Person" => SymbolKind::Person,
            "Attachment" => SymbolKind::Attachment,
            _ => SymbolKind::Variable,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodeSymbol {
    pub uid: String,
    pub name: String,
    pub qualified_name: String,
    pub kind: SymbolKind,
    pub file_path: String,
    pub start_line: u32,
    pub end_line: u32,
    pub signature: String,
    pub content: String,
    pub repo_id: String,
    /// JSON-serialized [`SymbolMetadata`] (optional, for backward compatibility).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileNode {
    pub uid: String,
    pub path: String,
    pub language: String,
    pub repo_id: String,
    pub num_symbols: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum RelationshipKind {
    Calls,
    ContainedBy,
    MemberOf,
    StepInProcess,
    Imports,
    /// A document references another document or code file via a link
    References,
    /// A rationale comment explains a code symbol's design decision
    RationaleFor,
    /// Email replies to another email
    ReplyTo,
    /// Email was sent by a person
    SentBy,
    /// Email was received by a person
    ReceivedBy,
    /// Email has an attachment
    HasAttachment,
    /// Email belongs to a conversation thread
    PartOfConversation,
    /// Email body mentions a code symbol or person
    Mentions,
}

impl std::fmt::Display for RelationshipKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RelationshipKind::Calls => write!(f, "CALLS"),
            RelationshipKind::ContainedBy => write!(f, "CONTAINED_BY"),
            RelationshipKind::MemberOf => write!(f, "MEMBER_OF"),
            RelationshipKind::StepInProcess => write!(f, "STEP_IN_PROCESS"),
            RelationshipKind::Imports => write!(f, "IMPORTS"),
            RelationshipKind::References => write!(f, "REFERENCES"),
            RelationshipKind::RationaleFor => write!(f, "RATIONALE_FOR"),
            RelationshipKind::ReplyTo => write!(f, "REPLY_TO"),
            RelationshipKind::SentBy => write!(f, "SENT_BY"),
            RelationshipKind::ReceivedBy => write!(f, "RECEIVED_BY"),
            RelationshipKind::HasAttachment => write!(f, "HAS_ATTACHMENT"),
            RelationshipKind::PartOfConversation => write!(f, "PART_OF_CONVERSATION"),
            RelationshipKind::Mentions => write!(f, "MENTIONS"),
        }
    }
}

impl FromStr for RelationshipKind {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "CALLS" => RelationshipKind::Calls,
            "CONTAINED_BY" => RelationshipKind::ContainedBy,
            "MEMBER_OF" => RelationshipKind::MemberOf,
            "STEP_IN_PROCESS" => RelationshipKind::StepInProcess,
            "IMPORTS" => RelationshipKind::Imports,
            "REFERENCES" => RelationshipKind::References,
            "RATIONALE_FOR" => RelationshipKind::RationaleFor,
            "REPLY_TO" => RelationshipKind::ReplyTo,
            "SENT_BY" => RelationshipKind::SentBy,
            "RECEIVED_BY" => RelationshipKind::ReceivedBy,
            "HAS_ATTACHMENT" => RelationshipKind::HasAttachment,
            "PART_OF_CONVERSATION" => RelationshipKind::PartOfConversation,
            "MENTIONS" => RelationshipKind::Mentions,
            _ => RelationshipKind::Calls,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Relationship {
    pub uid: String,
    pub source_uid: String,
    pub target_uid: String,
    pub kind: RelationshipKind,
    pub repo_id: String,
    pub metadata: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Community {
    pub uid: String,
    pub label: String,
    pub repo_id: String,
    pub member_count: u32,
    pub top_symbols: String,
    pub summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Process {
    pub uid: String,
    pub name: String,
    pub repo_id: String,
    pub entry_point: String,
    pub step_count: u32,
    pub description: String,
}

// --- Team Models (Enterprise) ---

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum TeamRole {
    Owner,
    Admin,
    Member,
    Viewer,
}

impl std::fmt::Display for TeamRole {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TeamRole::Owner => write!(f, "owner"),
            TeamRole::Admin => write!(f, "admin"),
            TeamRole::Member => write!(f, "member"),
            TeamRole::Viewer => write!(f, "viewer"),
        }
    }
}

impl FromStr for TeamRole {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "owner" => TeamRole::Owner,
            "admin" => TeamRole::Admin,
            "member" => TeamRole::Member,
            "viewer" => TeamRole::Viewer,
            _ => TeamRole::Viewer,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Team {
    pub uid: String,
    pub name: String,
    pub owner_id: String,
    pub created_at: String,
    pub member_count: u32,
    pub repo_ids: String, // Comma-separated list of repo IDs
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeamMember {
    pub uid: String,
    pub team_id: String,
    pub user_id: String,
    pub email: String,
    pub role: TeamRole,
    pub joined_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoInfo {
    pub id: String,
    pub name: String,
    pub path: String,
    pub analyzed_at: String,
    pub symbol_count: u32,
    pub file_count: u32,
    /// Git commit hash at the time of analysis (for cache invalidation)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub analyzed_commit: Option<String>,
}

// --- Symbol Metadata ---

/// Git SDLC metadata for a symbol.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GitMetadataEntry {
    /// The name of the author who last modified the symbol.
    pub last_author: String,
    /// ISO 8601 date when the symbol was last modified.
    pub last_modified: String,
    /// Total number of commits touching this symbol's lines.
    pub commit_count: u32,
    /// Age of the symbol in days since last modification.
    pub age_days: u32,
    /// Optional commit hash of the last modification.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_commit_hash: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SymbolMetadata {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub decorators: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub return_type: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub superclasses: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub type_params: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visibility: Option<Visibility>,
    /// Git SDLC metadata (author, date, commit count, age)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git: Option<GitMetadataEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum Visibility {
    Public,
    Private,
    Protected,
    Internal,
}
