use crate::lexer::Token;
use thiserror::Error;

/// Error produced while parsing a Cypher query into an AST.
#[derive(Error, Debug)]
pub enum ParseError {
    /// A token appeared where the grammar did not allow it.
    #[error("Unexpected token: {0}")]
    UnexpectedToken(String),
    /// A specific token was expected but a different one was found.
    #[error("Expected {expected}, found {found}")]
    Expected {
        /// Description of the token or construct the parser expected.
        expected: String,
        /// Description of the token that was actually found.
        found: String,
    },
    /// A write/mutating clause was encountered; this parser is read-only.
    #[error("Write operations are blocked: {0}")]
    BlockedOperation(String),
    /// Input ended before a complete query could be parsed.
    #[error("Unexpected end of input")]
    UnexpectedEnd,
}

/// A parsed, read-only Cypher query.
#[derive(Debug, Clone)]
pub struct Query {
    /// Optional `MATCH` clause describing the graph pattern to find.
    pub match_clause: Option<MatchClause>,
    /// Optional `WHERE` clause filtering matched rows.
    pub where_clause: Option<WhereClause>,
    /// Optional `WITH` clause projecting intermediate results.
    pub with_clause: Option<WithClause>,
    /// The `RETURN` clause describing the query's output columns.
    pub return_clause: ReturnClause,
    /// Optional `ORDER BY` clause.
    pub order_by: Option<OrderByClause>,
    /// Optional `SKIP` count.
    pub skip: Option<i64>,
    /// Optional `LIMIT` count.
    pub limit: Option<i64>,
}

/// A `MATCH` clause: one or more graph patterns plus path-variable bindings.
#[derive(Debug, Clone)]
pub struct MatchClause {
    /// Node and relationship patterns to match against the graph.
    pub patterns: Vec<Pattern>,
    /// Named path variables bound via path functions (e.g. `shortestPath`).
    pub path_vars: Vec<PathVariableBinding>,
}

/// Binds a variable name to the result of a path function.
#[derive(Debug, Clone)]
pub struct PathVariableBinding {
    /// The variable name the path result is bound to.
    pub variable: String,
    /// The path function that produces the bound value.
    pub path_fn: PathFunction,
}

/// A path-finding function invoked in a pattern.
#[derive(Debug, Clone)]
pub enum PathFunction {
    /// `shortestPath(...)` — the shortest path between two nodes.
    ShortestPath(PathFunctionArgs),
    /// `allPaths(...)` — every path between two nodes (bounded by depth).
    AllPaths(PathFunctionArgs),
    /// `anyPath(...)` — any single path between two nodes.
    AnyPath(PathFunctionArgs),
}

/// Arguments to a [`PathFunction`].
#[derive(Debug, Clone)]
pub struct PathFunctionArgs {
    /// Optional start node variable.
    pub start: Option<String>,
    /// Optional end node variable.
    pub end: Option<String>,
    /// Optional maximum traversal depth.
    pub max_depth: Option<i64>,
    /// Relationship types the path may traverse.
    pub rel_types: Vec<String>,
}

/// A graph pattern element within a `MATCH` clause.
#[derive(Debug, Clone)]
pub enum Pattern {
    /// A single node pattern.
    Node(NodePattern),
    /// A relationship connecting two node patterns: `(a)-[r]->(b)`.
    Relationship(NodePattern, RelPattern, NodePattern),
}

/// A node pattern such as `(n:Function {name: "foo"})`.
#[derive(Debug, Clone)]
pub struct NodePattern {
    /// Optional bound variable for the node.
    pub variable: Option<String>,
    /// Optional node label (entity type).
    pub label: Option<String>,
    /// Inline property equality constraints as `(key, value)` pairs.
    pub properties: Vec<(String, Expr)>,
}

/// A relationship pattern such as `-[r:CALLS]->`.
#[derive(Debug, Clone)]
pub struct RelPattern {
    /// Optional bound variable for the relationship.
    pub variable: Option<String>,
    /// Optional relationship type (edge type).
    pub rel_type: Option<String>,
    /// Traversal direction of the relationship.
    pub direction: Direction,
}

/// Direction of a relationship pattern.
#[derive(Debug, Clone, PartialEq)]
pub enum Direction {
    /// Left-to-right: `-->`.
    Right,
    /// Right-to-left: `<--`.
    Left,
    /// Undirected: `--`.
    Both,
}

/// A `WHERE` clause holding a boolean filter expression.
#[derive(Debug, Clone)]
pub struct WhereClause {
    /// The predicate expression evaluated per row.
    pub expr: Expr,
}

/// A `WITH` clause projecting intermediate result columns.
#[derive(Debug, Clone)]
pub struct WithClause {
    /// The projected items carried forward.
    pub items: Vec<ReturnItem>,
}

/// A `RETURN` clause describing the query's output.
#[derive(Debug, Clone)]
pub struct ReturnClause {
    /// Whether `DISTINCT` was requested.
    pub distinct: bool,
    /// The returned items (expressions with optional aliases).
    pub items: Vec<ReturnItem>,
}

/// A single returned item: an expression with an optional alias.
#[derive(Debug, Clone)]
pub struct ReturnItem {
    /// The expression producing the column value.
    pub expr: Expr,
    /// Optional column alias (`AS name`).
    pub alias: Option<String>,
}

/// An `ORDER BY` clause.
#[derive(Debug, Clone)]
pub struct OrderByClause {
    /// Sort keys as `(expression, ascending)` pairs, where `true` sorts
    /// ascending (`ASC`, the default) and `false` sorts descending (`DESC`).
    pub items: Vec<(Expr, bool)>,
}

/// An expression node in the query AST.
#[derive(Debug, Clone)]
pub enum Expr {
    /// A bare identifier (variable reference).
    Ident(String),
    /// A property access `variable.property`.
    Property(String, String),
    /// A string literal.
    StringLit(String),
    /// An integer literal.
    IntLit(i64),
    /// A floating-point literal.
    FloatLit(f64),
    /// A boolean literal.
    BoolLit(bool),
    /// The `NULL` literal.
    Null,
    /// A binary operation `lhs op rhs`.
    BinOp(Box<Expr>, BinOp, Box<Expr>),
    /// Logical negation `NOT expr`.
    Not(Box<Expr>),
    /// `lhs CONTAINS rhs` substring test.
    Contains(Box<Expr>, Box<Expr>),
    /// `expr IS NULL` test.
    IsNull(Box<Expr>),
    /// `expr IS NOT NULL` test.
    IsNotNull(Box<Expr>),
    /// A scalar function call `name(args...)`.
    FunctionCall(String, Vec<Expr>),
    /// An aggregation over an expression, e.g. `count(x)`.
    Aggregation(AggregationFunc, Box<Expr>),
}

/// A supported aggregation function.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AggregationFunc {
    /// `count(...)`.
    Count,
    /// `sum(...)`.
    Sum,
    /// `avg(...)`.
    Avg,
    /// `min(...)`.
    Min,
    /// `max(...)`.
    Max,
    /// `collect(...)` — gather values into a list.
    Collect,
}

/// A binary operator.
#[derive(Debug, Clone)]
pub enum BinOp {
    /// Equality `=`.
    Eq,
    /// Inequality `<>`.
    Neq,
    /// Less-than `<`.
    Lt,
    /// Greater-than `>`.
    Gt,
    /// Less-than-or-equal `<=`.
    Lte,
    /// Greater-than-or-equal `>=`.
    Gte,
    /// Logical `AND`.
    And,
    /// Logical `OR`.
    Or,
}

/// Recursive-descent parser turning a token stream into a [`Query`] AST.
pub struct Parser {
    tokens: Vec<Token>,
    pos: usize,
}

impl Parser {
    /// Creates a parser over the given token stream.
    pub fn new(tokens: Vec<Token>) -> Self {
        Self { tokens, pos: 0 }
    }

    /// Parses the token stream into a [`Query`], rejecting write operations.
    pub fn parse(mut self) -> Result<Query, ParseError> {
        // Check for blocked operations
        for tok in &self.tokens {
            match tok {
                Token::Create => return Err(ParseError::BlockedOperation("CREATE".into())),
                Token::Delete => return Err(ParseError::BlockedOperation("DELETE".into())),
                Token::Set => return Err(ParseError::BlockedOperation("SET".into())),
                Token::Merge => return Err(ParseError::BlockedOperation("MERGE".into())),
                Token::Drop => return Err(ParseError::BlockedOperation("DROP".into())),
                Token::Alter => return Err(ParseError::BlockedOperation("ALTER".into())),
                _ => {}
            }
        }

        let match_clause = if self.peek_is(&Token::Match) {
            Some(self.parse_match()?)
        } else {
            None
        };

        let where_clause = if self.peek_is(&Token::Where) {
            Some(self.parse_where()?)
        } else {
            None
        };

        let with_clause = if self.peek_is(&Token::With) {
            Some(self.parse_with()?)
        } else {
            None
        };

        let return_clause = self.parse_return()?;

        let order_by = if self.peek_is(&Token::Order) {
            Some(self.parse_order_by()?)
        } else {
            None
        };

        let skip = if self.peek_is(&Token::Skip) {
            self.advance();
            Some(self.expect_int()?)
        } else {
            None
        };

        let limit = if self.peek_is(&Token::Limit) {
            self.advance();
            Some(self.expect_int()?)
        } else {
            None
        };

        Ok(Query {
            match_clause,
            where_clause,
            with_clause,
            return_clause,
            order_by,
            skip,
            limit,
        })
    }

    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos)
    }

    fn peek_is(&self, tok: &Token) -> bool {
        self.peek()
            .map(|t| std::mem::discriminant(t) == std::mem::discriminant(tok))
            .unwrap_or(false)
    }

    fn advance(&mut self) -> Option<Token> {
        if self.pos < self.tokens.len() {
            let tok = self.tokens[self.pos].clone();
            self.pos += 1;
            Some(tok)
        } else {
            None
        }
    }

    fn expect(&mut self, expected: &Token) -> Result<Token, ParseError> {
        let tok = self.advance().ok_or(ParseError::UnexpectedEnd)?;
        if std::mem::discriminant(&tok) == std::mem::discriminant(expected) {
            Ok(tok)
        } else {
            Err(ParseError::Expected {
                expected: format!("{:?}", expected),
                found: format!("{:?}", tok),
            })
        }
    }

    fn expect_ident(&mut self) -> Result<String, ParseError> {
        match self.advance() {
            Some(Token::Ident(s)) => Ok(s),
            Some(t) => Err(ParseError::Expected {
                expected: "identifier".into(),
                found: format!("{:?}", t),
            }),
            None => Err(ParseError::UnexpectedEnd),
        }
    }

    fn expect_int(&mut self) -> Result<i64, ParseError> {
        match self.advance() {
            Some(Token::IntLit(n)) => Ok(n),
            Some(t) => Err(ParseError::Expected {
                expected: "integer".into(),
                found: format!("{:?}", t),
            }),
            None => Err(ParseError::UnexpectedEnd),
        }
    }

    fn parse_match(&mut self) -> Result<MatchClause, ParseError> {
        self.expect(&Token::Match)?;
        let mut patterns = Vec::new();
        let mut path_vars = Vec::new();

        // Parse patterns and path variable bindings
        loop {
            // Check for 'path = function(...)' syntax
            if let Some(Token::Ident(var_name)) = self.peek() {
                let saved_pos = self.pos;
                let var = var_name.clone();
                self.advance();

                if self.peek_is(&Token::Eq) {
                    self.advance();
                    // This is a path variable binding
                    let path_fn = self.parse_path_function()?;
                    path_vars.push(PathVariableBinding {
                        variable: var,
                        path_fn,
                    });
                } else {
                    // Not a path binding, restore position and parse as pattern
                    self.pos = saved_pos;
                    patterns.push(self.parse_pattern()?);
                }
            } else {
                patterns.push(self.parse_pattern()?);
            }

            if !self.peek_is(&Token::Comma) {
                break;
            }
            self.advance();
        }

        Ok(MatchClause {
            patterns,
            path_vars,
        })
    }

    fn parse_path_function(&mut self) -> Result<PathFunction, ParseError> {
        let func_name = self.expect_ident()?;
        self.expect(&Token::LParen)?;

        // Parse the path pattern: (a)-[*..depth]->(b)
        let start = self.parse_path_endpoint()?;

        // Parse relationship pattern: -[*..depth]->
        self.expect(&Token::Dash)?;
        self.expect(&Token::LBracket)?;
        self.expect(&Token::Star)?;

        let max_depth = if self.peek_is(&Token::Dot) {
            self.advance();
            self.expect(&Token::Dot)?;
            Some(self.expect_int()?)
        } else {
            None
        };

        self.expect(&Token::RBracket)?;
        self.expect(&Token::Arrow)?;

        let end = self.parse_path_endpoint()?;
        self.expect(&Token::RParen)?;

        let path_fn = match func_name.to_lowercase().as_str() {
            "shortestpath" => PathFunction::ShortestPath(PathFunctionArgs {
                start,
                end,
                max_depth: max_depth.or(Some(5)),
                rel_types: Vec::new(),
            }),
            "allpaths" => PathFunction::AllPaths(PathFunctionArgs {
                start,
                end,
                max_depth: max_depth.or(Some(5)),
                rel_types: Vec::new(),
            }),
            "anypath" => PathFunction::AnyPath(PathFunctionArgs {
                start,
                end,
                max_depth: max_depth.or(Some(5)),
                rel_types: Vec::new(),
            }),
            _ => {
                return Err(ParseError::UnexpectedToken(format!(
                    "Unknown path function: {}",
                    func_name
                )))
            }
        };

        Ok(path_fn)
    }

    fn parse_path_endpoint(&mut self) -> Result<Option<String>, ParseError> {
        self.expect(&Token::LParen)?;
        let var = if let Some(Token::Ident(_)) = self.peek() {
            Some(self.expect_ident()?)
        } else {
            None
        };
        // Skip label if present (e.g., :Function)
        if self.peek_is(&Token::Colon) {
            self.advance();
            self.expect_ident()?;
        }
        self.expect(&Token::RParen)?;
        Ok(var)
    }

    fn parse_pattern(&mut self) -> Result<Pattern, ParseError> {
        let node = self.parse_node_pattern()?;

        // Check if there's a relationship following
        if self.peek_is(&Token::Dash) || self.peek_is(&Token::LeftArrow) {
            let (rel, _direction_start) = self.parse_rel_start()?;
            let end_node = self.parse_node_pattern()?;
            return Ok(Pattern::Relationship(node, rel, end_node));
        }

        Ok(Pattern::Node(node))
    }

    fn parse_node_pattern(&mut self) -> Result<NodePattern, ParseError> {
        self.expect(&Token::LParen)?;

        let mut variable = None;
        let mut label = None;
        let mut properties = Vec::new();

        // Variable name
        if let Some(Token::Ident(_)) = self.peek() {
            variable = Some(self.expect_ident()?);
        }

        // Label
        if self.peek_is(&Token::Colon) {
            self.advance();
            label = Some(self.expect_ident()?);
        }

        // Properties { key: value, ... }
        if self.peek_is(&Token::LBrace) {
            self.advance();
            while !self.peek_is(&Token::RBrace) {
                let key = self.expect_ident()?;
                self.expect(&Token::Colon)?;
                let value = self.parse_expr()?;
                properties.push((key, value));
                if self.peek_is(&Token::Comma) {
                    self.advance();
                }
            }
            self.expect(&Token::RBrace)?;
        }

        self.expect(&Token::RParen)?;

        Ok(NodePattern {
            variable,
            label,
            properties,
        })
    }

    fn parse_rel_start(&mut self) -> Result<(RelPattern, Direction), ParseError> {
        let left_arrow = self.peek_is(&Token::LeftArrow);
        if left_arrow {
            self.advance(); // <-
        } else {
            self.advance(); // -
        }

        let mut variable = None;
        let mut rel_type = None;

        if self.peek_is(&Token::LBracket) {
            self.advance();
            if let Some(Token::Ident(_)) = self.peek() {
                variable = Some(self.expect_ident()?);
            }
            if self.peek_is(&Token::Colon) {
                self.advance();
                rel_type = Some(self.expect_ident()?);
            }
            self.expect(&Token::RBracket)?;
        }

        let direction = if left_arrow {
            self.expect(&Token::Dash)?;
            Direction::Left
        } else if self.peek_is(&Token::Arrow) {
            self.advance();
            Direction::Right
        } else if self.peek_is(&Token::Dash) {
            self.advance();
            Direction::Both
        } else {
            Direction::Both
        };

        Ok((
            RelPattern {
                variable,
                rel_type,
                direction: direction.clone(),
            },
            direction,
        ))
    }

    fn parse_where(&mut self) -> Result<WhereClause, ParseError> {
        self.expect(&Token::Where)?;
        let expr = self.parse_or_expr()?;
        Ok(WhereClause { expr })
    }

    fn parse_or_expr(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.parse_and_expr()?;
        while self.peek_is(&Token::Or) {
            self.advance();
            let right = self.parse_and_expr()?;
            left = Expr::BinOp(Box::new(left), BinOp::Or, Box::new(right));
        }
        Ok(left)
    }

    fn parse_and_expr(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.parse_comparison()?;
        while self.peek_is(&Token::And) {
            self.advance();
            let right = self.parse_comparison()?;
            left = Expr::BinOp(Box::new(left), BinOp::And, Box::new(right));
        }
        Ok(left)
    }

    fn parse_comparison(&mut self) -> Result<Expr, ParseError> {
        if self.peek_is(&Token::Not) {
            self.advance();
            let expr = self.parse_comparison()?;
            return Ok(Expr::Not(Box::new(expr)));
        }

        let left = self.parse_primary()?;

        // Check for CONTAINS
        if self.peek_is(&Token::Contains) {
            self.advance();
            let right = self.parse_primary()?;
            return Ok(Expr::Contains(Box::new(left), Box::new(right)));
        }

        // Check for IS NULL / IS NOT NULL
        if self.peek_is(&Token::Is) {
            self.advance();
            if self.peek_is(&Token::Not) {
                self.advance();
                self.expect(&Token::Null)?;
                return Ok(Expr::IsNotNull(Box::new(left)));
            }
            self.expect(&Token::Null)?;
            return Ok(Expr::IsNull(Box::new(left)));
        }

        // Comparison operators
        if let Some(op) = self.peek_binop() {
            self.advance();
            let right = self.parse_primary()?;
            return Ok(Expr::BinOp(Box::new(left), op, Box::new(right)));
        }

        Ok(left)
    }

    fn peek_binop(&self) -> Option<BinOp> {
        match self.peek()? {
            Token::Eq => Some(BinOp::Eq),
            Token::Neq | Token::NotEq => Some(BinOp::Neq),
            Token::Lt => Some(BinOp::Lt),
            Token::Gt => Some(BinOp::Gt),
            Token::Lte => Some(BinOp::Lte),
            Token::Gte => Some(BinOp::Gte),
            _ => None,
        }
    }

    fn parse_primary(&mut self) -> Result<Expr, ParseError> {
        match self.peek().cloned() {
            Some(Token::Ident(name)) => {
                self.advance();
                // Check for aggregation functions first
                if self.peek_is(&Token::LParen) {
                    let name_lower = name.to_lowercase();
                    let is_agg = matches!(
                        name_lower.as_str(),
                        "count" | "sum" | "avg" | "min" | "max" | "collect"
                    );

                    if is_agg {
                        self.advance(); // consume (
                        let arg = self.parse_expr()?;
                        self.expect(&Token::RParen)?;
                        let agg_func = match name_lower.as_str() {
                            "count" => AggregationFunc::Count,
                            "sum" => AggregationFunc::Sum,
                            "avg" => AggregationFunc::Avg,
                            "min" => AggregationFunc::Min,
                            "max" => AggregationFunc::Max,
                            "collect" => AggregationFunc::Collect,
                            _ => unreachable!(),
                        };
                        return Ok(Expr::Aggregation(agg_func, Box::new(arg)));
                    }
                }

                // Check for property access: name.property
                if self.peek_is(&Token::Dot) {
                    self.advance();
                    let prop = self.expect_ident()?;
                    // Check for function call: name.prop(...)  -- unlikely but possible
                    Ok(Expr::Property(name, prop))
                } else if self.peek_is(&Token::LParen) {
                    // Function call: name(...) (for non-aggregation functions)
                    self.advance();
                    let mut args = Vec::new();
                    while !self.peek_is(&Token::RParen) {
                        args.push(self.parse_expr()?);
                        if self.peek_is(&Token::Comma) {
                            self.advance();
                        }
                    }
                    self.expect(&Token::RParen)?;
                    Ok(Expr::FunctionCall(name, args))
                } else {
                    Ok(Expr::Ident(name))
                }
            }
            Some(Token::StringLit(s)) => {
                self.advance();
                Ok(Expr::StringLit(s))
            }
            Some(Token::QuotedString(s)) => {
                self.advance();
                Ok(Expr::StringLit(s))
            }
            Some(Token::IntLit(n)) => {
                self.advance();
                Ok(Expr::IntLit(n))
            }
            Some(Token::FloatLit(n)) => {
                self.advance();
                Ok(Expr::FloatLit(n))
            }
            Some(Token::True) => {
                self.advance();
                Ok(Expr::BoolLit(true))
            }
            Some(Token::False) => {
                self.advance();
                Ok(Expr::BoolLit(false))
            }
            Some(Token::Null) => {
                self.advance();
                Ok(Expr::Null)
            }
            Some(Token::Star) => {
                self.advance();
                Ok(Expr::Ident("*".into()))
            }
            Some(t) => Err(ParseError::UnexpectedToken(format!("{:?}", t))),
            None => Err(ParseError::UnexpectedEnd),
        }
    }

    fn parse_expr(&mut self) -> Result<Expr, ParseError> {
        self.parse_or_expr()
    }

    fn parse_with(&mut self) -> Result<WithClause, ParseError> {
        self.expect(&Token::With)?;
        let items = self.parse_return_items()?;
        Ok(WithClause { items })
    }

    fn parse_return(&mut self) -> Result<ReturnClause, ParseError> {
        self.expect(&Token::Return)?;
        let distinct = if self.peek_is(&Token::Distinct) {
            self.advance();
            true
        } else {
            false
        };
        let items = self.parse_return_items()?;
        Ok(ReturnClause { distinct, items })
    }

    fn parse_return_items(&mut self) -> Result<Vec<ReturnItem>, ParseError> {
        let mut items = Vec::new();
        items.push(self.parse_return_item()?);
        while self.peek_is(&Token::Comma) {
            self.advance();
            items.push(self.parse_return_item()?);
        }
        Ok(items)
    }

    fn parse_return_item(&mut self) -> Result<ReturnItem, ParseError> {
        let expr = self.parse_expr()?;
        let alias = if self.peek_is(&Token::As) {
            self.advance();
            Some(self.expect_ident()?)
        } else {
            None
        };
        Ok(ReturnItem { expr, alias })
    }

    fn parse_order_by(&mut self) -> Result<OrderByClause, ParseError> {
        self.expect(&Token::Order)?;
        self.expect(&Token::By)?;
        let mut items = Vec::new();
        loop {
            let expr = self.parse_expr()?;
            let ascending = if self.peek_is(&Token::Desc) {
                self.advance();
                false
            } else {
                if self.peek_is(&Token::Asc) {
                    self.advance();
                }
                true
            };
            items.push((expr, ascending));
            if !self.peek_is(&Token::Comma) {
                break;
            }
            self.advance();
        }
        Ok(OrderByClause { items })
    }
}

/// Lexes and parses a Cypher query string into a [`Query`] AST in one step.
pub fn parse_cypher(input: &str) -> Result<Query, ParseError> {
    let tokens: Vec<Token> = crate::lexer::lex(input)
        .into_iter()
        .map(|(t, _)| t)
        .collect();
    Parser::new(tokens).parse()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_match() {
        let q = parse_cypher("MATCH (s:CodeSymbol) RETURN s.name").unwrap();
        assert!(q.match_clause.is_some());
        assert_eq!(q.return_clause.items.len(), 1);
    }

    #[test]
    fn test_parse_with_limit() {
        let q = parse_cypher("MATCH (s:CodeSymbol) RETURN s.name LIMIT 5").unwrap();
        assert_eq!(q.limit, Some(5));
    }

    #[test]
    fn test_parse_where() {
        let q =
            parse_cypher("MATCH (s:CodeSymbol) WHERE s.kind = 'Function' RETURN s.name").unwrap();
        assert!(q.where_clause.is_some());
    }

    #[test]
    fn test_parse_contains() {
        let q = parse_cypher("MATCH (s:CodeSymbol) WHERE s.name CONTAINS 'user' RETURN s").unwrap();
        assert!(q.where_clause.is_some());
    }

    #[test]
    fn test_blocked_create() {
        let result = parse_cypher("CREATE (n:Node) RETURN n");
        assert!(result.is_err());
        assert!(matches!(result, Err(ParseError::BlockedOperation(_))));
    }

    #[test]
    fn test_parse_order_by() {
        let q = parse_cypher("MATCH (s:CodeSymbol) RETURN s.name ORDER BY s.name ASC LIMIT 10")
            .unwrap();
        assert!(q.order_by.is_some());
        assert_eq!(q.limit, Some(10));
    }

    #[test]
    fn test_parse_relationship() {
        let q = parse_cypher("MATCH (a:CodeSymbol)-[:CALLS]->(b:CodeSymbol) RETURN a.name, b.name")
            .unwrap();
        let patterns = &q.match_clause.unwrap().patterns;
        assert!(matches!(patterns[0], Pattern::Relationship(_, _, _)));
    }

    #[test]
    fn test_parse_shortest_path() {
        let q = parse_cypher("MATCH path = shortestPath((a)-[*..5]->(b)) RETURN path").unwrap();
        let match_clause = q.match_clause.unwrap();
        assert_eq!(match_clause.path_vars.len(), 1);
        assert_eq!(match_clause.path_vars[0].variable, "path");
        assert!(matches!(
            match_clause.path_vars[0].path_fn,
            PathFunction::ShortestPath(_)
        ));
    }

    #[test]
    fn test_parse_all_paths() {
        let q = parse_cypher("MATCH path = allPaths((a)-[*..3]->(b)) RETURN path").unwrap();
        let match_clause = q.match_clause.unwrap();
        assert_eq!(match_clause.path_vars.len(), 1);
        assert!(matches!(
            match_clause.path_vars[0].path_fn,
            PathFunction::AllPaths(_)
        ));
    }

    #[test]
    fn test_parse_any_path() {
        let q = parse_cypher("MATCH path = anyPath((a)-[*..10]->(b)) RETURN path").unwrap();
        let match_clause = q.match_clause.unwrap();
        assert_eq!(match_clause.path_vars.len(), 1);
        assert!(matches!(
            match_clause.path_vars[0].path_fn,
            PathFunction::AnyPath(_)
        ));
    }

    #[test]
    fn test_parse_path_with_max_depth() {
        let q =
            parse_cypher("MATCH path = shortestPath((start)-[*..7]->(end)) RETURN path").unwrap();
        let match_clause = q.match_clause.unwrap();
        if let PathFunction::ShortestPath(args) = &match_clause.path_vars[0].path_fn {
            assert_eq!(args.max_depth, Some(7));
        } else {
            panic!("Expected ShortestPath");
        }
    }

    #[test]
    fn test_parse_path_with_labels() {
        let q = parse_cypher(
            "MATCH path = shortestPath((a:Function)-[*..5]->(b:Function)) RETURN path",
        )
        .unwrap();
        let match_clause = q.match_clause.unwrap();
        assert_eq!(match_clause.path_vars.len(), 1);
    }
}
