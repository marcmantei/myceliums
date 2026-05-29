use crate::lexer::Token;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum ParseError {
    #[error("Unexpected token: {0}")]
    UnexpectedToken(String),
    #[error("Expected {expected}, found {found}")]
    Expected { expected: String, found: String },
    #[error("Write operations are blocked: {0}")]
    BlockedOperation(String),
    #[error("Unexpected end of input")]
    UnexpectedEnd,
}

#[derive(Debug, Clone)]
pub struct Query {
    pub match_clause: Option<MatchClause>,
    pub where_clause: Option<WhereClause>,
    pub with_clause: Option<WithClause>,
    pub return_clause: ReturnClause,
    pub order_by: Option<OrderByClause>,
    pub skip: Option<i64>,
    pub limit: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct MatchClause {
    pub patterns: Vec<Pattern>,
    pub path_vars: Vec<PathVariableBinding>,
}

#[derive(Debug, Clone)]
pub struct PathVariableBinding {
    pub variable: String,
    pub path_fn: PathFunction,
}

#[derive(Debug, Clone)]
pub enum PathFunction {
    ShortestPath(PathFunctionArgs),
    AllPaths(PathFunctionArgs),
    AnyPath(PathFunctionArgs),
}

#[derive(Debug, Clone)]
pub struct PathFunctionArgs {
    pub start: Option<String>,
    pub end: Option<String>,
    pub max_depth: Option<i64>,
    pub rel_types: Vec<String>,
}

#[derive(Debug, Clone)]
pub enum Pattern {
    Node(NodePattern),
    Relationship(NodePattern, RelPattern, NodePattern),
}

#[derive(Debug, Clone)]
pub struct NodePattern {
    pub variable: Option<String>,
    pub label: Option<String>,
    pub properties: Vec<(String, Expr)>,
}

#[derive(Debug, Clone)]
pub struct RelPattern {
    pub variable: Option<String>,
    pub rel_type: Option<String>,
    pub direction: Direction,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Direction {
    Right, // -->
    Left,  // <--
    Both,  // --
}

#[derive(Debug, Clone)]
pub struct WhereClause {
    pub expr: Expr,
}

#[derive(Debug, Clone)]
pub struct WithClause {
    pub items: Vec<ReturnItem>,
}

#[derive(Debug, Clone)]
pub struct ReturnClause {
    pub distinct: bool,
    pub items: Vec<ReturnItem>,
}

#[derive(Debug, Clone)]
pub struct ReturnItem {
    pub expr: Expr,
    pub alias: Option<String>,
}

#[derive(Debug, Clone)]
pub struct OrderByClause {
    pub items: Vec<(Expr, bool)>, // (expr, ascending)
}

#[derive(Debug, Clone)]
pub enum Expr {
    Ident(String),
    Property(String, String), // variable.property
    StringLit(String),
    IntLit(i64),
    FloatLit(f64),
    BoolLit(bool),
    Null,
    BinOp(Box<Expr>, BinOp, Box<Expr>),
    Not(Box<Expr>),
    Contains(Box<Expr>, Box<Expr>),
    IsNull(Box<Expr>),
    IsNotNull(Box<Expr>),
    FunctionCall(String, Vec<Expr>),
    Aggregation(AggregationFunc, Box<Expr>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AggregationFunc {
    Count,
    Sum,
    Avg,
    Min,
    Max,
    Collect,
}

#[derive(Debug, Clone)]
pub enum BinOp {
    Eq,
    Neq,
    Lt,
    Gt,
    Lte,
    Gte,
    And,
    Or,
}

pub struct Parser {
    tokens: Vec<Token>,
    pos: usize,
}

impl Parser {
    pub fn new(tokens: Vec<Token>) -> Self {
        Self { tokens, pos: 0 }
    }

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
