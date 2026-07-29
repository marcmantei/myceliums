use serde::{Deserialize, Serialize};
use std::str::FromStr;

/// The kind of a symbol extracted from source or content files.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum SymbolKind {
    /// A free function.
    Function,
    /// A method attached to a type.
    Method,
    /// A class definition.
    Class,
    /// An interface or protocol definition.
    Interface,
    /// A type alias.
    TypeAlias,
    /// A variable binding.
    Variable,
    /// A constant binding.
    Constant,
    /// An enum definition.
    Enum,
    /// A module or namespace.
    Module,
    /// An import or `use` statement.
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

/// A symbol extracted from a source or content file, persisted in the store.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodeSymbol {
    /// Stable unique identifier for the symbol.
    pub uid: String,
    /// Short (unqualified) name.
    pub name: String,
    /// Fully qualified name including its container path.
    pub qualified_name: String,
    /// The kind of symbol (function, class, document, ...).
    pub kind: SymbolKind,
    /// Path of the file the symbol was extracted from.
    pub file_path: String,
    /// 1-based line where the symbol starts.
    pub start_line: u32,
    /// 1-based line where the symbol ends.
    pub end_line: u32,
    /// The symbol's signature (declaration line).
    pub signature: String,
    /// The symbol's body/content text.
    pub content: String,
    /// Identifier of the repository the symbol belongs to.
    pub repo_id: String,
    /// JSON-serialized [`SymbolMetadata`] (optional, for backward compatibility).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<String>,
}

/// A source or content file that was indexed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileNode {
    /// Stable unique identifier for the file.
    pub uid: String,
    /// Path of the file within the repository.
    pub path: String,
    /// Detected language (or content type) of the file.
    pub language: String,
    /// Identifier of the repository the file belongs to.
    pub repo_id: String,
    /// Number of symbols extracted from this file.
    pub num_symbols: u32,
}

/// The kind of a relationship (edge) between two nodes in the graph.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum RelationshipKind {
    /// One symbol calls another.
    Calls,
    /// A symbol is contained by another (e.g. method in class).
    ContainedBy,
    /// A symbol is a member of a container.
    MemberOf,
    /// A symbol is a step within a process.
    StepInProcess,
    /// A file or symbol imports another.
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

/// An edge between two nodes in the code graph.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Relationship {
    /// Stable unique identifier for the relationship.
    pub uid: String,
    /// UID of the source node.
    pub source_uid: String,
    /// UID of the target node.
    pub target_uid: String,
    /// The kind of relationship.
    pub kind: RelationshipKind,
    /// Identifier of the repository the relationship belongs to.
    pub repo_id: String,
    /// JSON-serialized, relationship-specific metadata.
    pub metadata: String,
}

/// A detected community (cluster) of related symbols.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Community {
    /// Stable unique identifier for the community.
    pub uid: String,
    /// Human-readable label for the community.
    pub label: String,
    /// Identifier of the repository the community belongs to.
    pub repo_id: String,
    /// Number of member symbols.
    pub member_count: u32,
    /// JSON-serialized list of the most central member symbols.
    pub top_symbols: String,
    /// Generated summary describing the community.
    pub summary: String,
}

/// A traced process (an ordered sequence of steps through the graph).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Process {
    /// Stable unique identifier for the process.
    pub uid: String,
    /// Human-readable name of the process.
    pub name: String,
    /// Identifier of the repository the process belongs to.
    pub repo_id: String,
    /// UID of the entry-point symbol.
    pub entry_point: String,
    /// Number of steps in the process.
    pub step_count: u32,
    /// Generated description of the process.
    pub description: String,
}

// --- Team Models (Enterprise) ---

/// A member's role within a team.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum TeamRole {
    /// Full control, including team deletion and ownership transfer.
    Owner,
    /// Administrative rights over members and settings.
    Admin,
    /// Standard contributing member.
    Member,
    /// Read-only access.
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

/// A team of users sharing access to repositories.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Team {
    /// Stable unique identifier for the team.
    pub uid: String,
    /// Display name of the team.
    pub name: String,
    /// User identifier of the team owner.
    pub owner_id: String,
    /// ISO 8601 creation timestamp.
    pub created_at: String,
    /// Number of members in the team.
    pub member_count: u32,
    /// Comma-separated list of repo IDs the team can access.
    pub repo_ids: String,
}

/// Membership record linking a user to a team with a role.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeamMember {
    /// Stable unique identifier for the membership.
    pub uid: String,
    /// UID of the team.
    pub team_id: String,
    /// Identifier of the member user.
    pub user_id: String,
    /// Email address of the member.
    pub email: String,
    /// The member's role within the team.
    pub role: TeamRole,
    /// ISO 8601 timestamp of when the user joined.
    pub joined_at: String,
}

/// Registry metadata describing an analyzed repository.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoInfo {
    /// Stable repository identifier.
    pub id: String,
    /// Human-readable repository name.
    pub name: String,
    /// Filesystem path to the repository.
    pub path: String,
    /// ISO 8601 timestamp of the last analysis.
    pub analyzed_at: String,
    /// Number of symbols indexed.
    pub symbol_count: u32,
    /// Number of files indexed.
    pub file_count: u32,
    /// Git commit hash at the time of analysis (for cache invalidation)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub analyzed_commit: Option<String>,
    /// Stored vector geometry version (see [`crate::schema::VECTOR_GEOMETRY_VERSION`]).
    /// `None`/`0` means a legacy index with unnormalized vectors that must be
    /// rebuilt before its vectors can be trusted for cosine ranking (issue #29).
    #[serde(default)]
    pub vector_geometry_version: u32,
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

/// Optional structured metadata attached to a [`CodeSymbol`].
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SymbolMetadata {
    /// Decorators or attributes applied to the symbol.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub decorators: Vec<String>,
    /// Declared return type, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub return_type: Option<String>,
    /// Names of superclasses or base types.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub superclasses: Vec<String>,
    /// Generic type parameters.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub type_params: Vec<String>,
    /// Declared visibility, if known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visibility: Option<Visibility>,
    /// Git SDLC metadata (author, date, commit count, age)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git: Option<GitMetadataEntry>,
}

/// Declared visibility of a symbol.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum Visibility {
    /// Publicly accessible.
    Public,
    /// Private to its defining scope.
    Private,
    /// Accessible to subclasses.
    Protected,
    /// Accessible within the same module/package.
    Internal,
}
