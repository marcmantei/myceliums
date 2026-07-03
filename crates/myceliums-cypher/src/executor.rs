use anyhow::Result;
use myceliums_core::Ontology;
use myceliums_storage::{CodeSymbol, FileNode, Relationship, Store};
use serde_json::{json, Value};
use std::collections::{HashMap, VecDeque};
use tracing::info;

use crate::parser::{
    self, AggregationFunc, BinOp, Direction, Expr, PathFunction, PathVariableBinding, Pattern,
    Query, WhereClause,
};

/// Maximum number of rows that can be returned from a query
const MAX_RESULT_ROWS: usize = 100_000;

/// Maximum allowed cartesian product size (when cross-joining patterns)
const MAX_INTERMEDIATE_ROWS: usize = 10_000;

pub struct CypherExecutor {
    symbols: Vec<CodeSymbol>,
    files: Vec<FileNode>,
    relationships: Vec<Relationship>,
    ontology: Option<Ontology>,
}

type Row = HashMap<String, Value>;

#[derive(Debug, Clone)]
struct PathNode {
    uid: String,
    symbol: Value,
}

#[derive(Debug, Clone)]
struct PathEdge {
    source_uid: String,
    target_uid: String,
    relationship: Value,
}

#[derive(Debug, Clone)]
struct Path {
    nodes: Vec<PathNode>,
    edges: Vec<PathEdge>,
}

impl CypherExecutor {
    pub async fn from_store(store: &Store) -> Result<Self> {
        let symbols = store.get_symbols().await?;
        let files = store.get_files().await?;
        let relationships = store.get_relationships().await?;
        Ok(Self {
            symbols,
            files,
            relationships,
            ontology: None,
        })
    }

    pub async fn from_store_with_ontology(store: &Store, ontology: Ontology) -> Result<Self> {
        let symbols = store.get_symbols().await?;
        let files = store.get_files().await?;
        let relationships = store.get_relationships().await?;
        Ok(Self {
            symbols,
            files,
            relationships,
            ontology: Some(ontology),
        })
    }

    pub fn with_ontology(mut self, ontology: Ontology) -> Self {
        self.ontology = Some(ontology);
        self
    }

    pub fn execute(&self, query_str: &str) -> Result<Vec<Row>> {
        let query =
            parser::parse_cypher(query_str).map_err(|e| anyhow::anyhow!("Parse error: {}", e))?;

        // Validate that unsupported clauses are not present
        if query.with_clause.is_some() {
            return Err(anyhow::anyhow!(
                "WITH clause is not supported. Please use MATCH ... RETURN or MATCH ... WHERE ... RETURN instead."
            ));
        }

        if query.return_clause.distinct {
            return Err(anyhow::anyhow!(
                "DISTINCT is not supported. Please remove DISTINCT from your RETURN clause."
            ));
        }

        // Check for unknown labels in patterns
        if let Some(ref match_clause) = query.match_clause {
            for pattern in &match_clause.patterns {
                self.validate_pattern_labels(pattern)?;
            }
        }

        // Check for inline property maps in patterns (properties in node patterns)
        if let Some(ref match_clause) = query.match_clause {
            for pattern in &match_clause.patterns {
                self.validate_pattern_properties(pattern)?;
            }
        }

        let mut rows = self.execute_match(&query)?;

        if let Some(ref where_clause) = query.where_clause {
            rows = self.apply_where(rows, where_clause);
        }

        // Check if query has aggregation functions
        let has_aggregation = query
            .return_clause
            .items
            .iter()
            .any(|item| contains_aggregation(&item.expr));

        let mut result = if has_aggregation {
            self.apply_aggregation(rows, &query)
        } else {
            self.apply_return(rows, &query)
        };

        if let Some(ref order_by) = query.order_by {
            self.apply_order_by(&mut result, order_by);
        }

        if let Some(skip) = query.skip {
            result = result.into_iter().skip(skip as usize).collect();
        }

        if let Some(limit) = query.limit {
            result.truncate(limit as usize);
        }

        // Check final result size against max rows limit
        if result.len() > MAX_RESULT_ROWS {
            return Err(anyhow::anyhow!(
                "Query returned {} rows, exceeding maximum limit of {}. \
                 Use LIMIT or WHERE clauses to reduce the result set.",
                result.len(),
                MAX_RESULT_ROWS
            ));
        }

        Ok(result)
    }

    fn validate_pattern_labels(&self, pattern: &Pattern) -> Result<()> {
        match pattern {
            Pattern::Node(node_pat) => {
                if let Some(label) = &node_pat.label {
                    if !self.is_supported_label(label) {
                        return Err(anyhow::anyhow!(
                            "Unknown label: `{}`. Supported labels are: CodeSymbol, File",
                            label
                        ));
                    }
                }
                Ok(())
            }
            Pattern::Relationship(start, _rel, end) => {
                if let Some(label) = &start.label {
                    if !self.is_supported_label(label) {
                        return Err(anyhow::anyhow!(
                            "Unknown label: `{}`. Supported labels are: CodeSymbol, File",
                            label
                        ));
                    }
                }
                if let Some(label) = &end.label {
                    if !self.is_supported_label(label) {
                        return Err(anyhow::anyhow!(
                            "Unknown label: `{}`. Supported labels are: CodeSymbol, File",
                            label
                        ));
                    }
                }
                Ok(())
            }
        }
    }

    fn is_supported_label(&self, label: &str) -> bool {
        matches!(label, "CodeSymbol" | "File")
    }

    fn validate_pattern_properties(&self, pattern: &Pattern) -> Result<()> {
        match pattern {
            Pattern::Node(node_pat) => {
                if !node_pat.properties.is_empty() {
                    return Err(anyhow::anyhow!(
                        "Inline property maps in MATCH patterns are not supported. \
                         Use WHERE clauses to filter by properties instead. \
                         Example: MATCH (n) WHERE n.name = 'value' RETURN n"
                    ));
                }
                Ok(())
            }
            Pattern::Relationship(start, _rel, end) => {
                if !start.properties.is_empty() {
                    return Err(anyhow::anyhow!(
                        "Inline property maps in MATCH patterns are not supported. \
                         Use WHERE clauses to filter by properties instead. \
                         Example: MATCH (n) WHERE n.name = 'value' RETURN n"
                    ));
                }
                if !end.properties.is_empty() {
                    return Err(anyhow::anyhow!(
                        "Inline property maps in MATCH patterns are not supported. \
                         Use WHERE clauses to filter by properties instead. \
                         Example: MATCH (n) WHERE n.name = 'value' RETURN n"
                    ));
                }
                Ok(())
            }
        }
    }

    fn execute_match(&self, query: &Query) -> Result<Vec<Row>> {
        let match_clause = match &query.match_clause {
            Some(m) => m,
            None => return Ok(vec![Row::new()]),
        };

        let mut rows: Vec<Row> = vec![];

        for pattern in &match_clause.patterns {
            let pattern_rows = self.match_pattern(pattern)?;
            if rows.is_empty() {
                rows = pattern_rows;
            } else {
                // Cross join with cartesian product size check
                let mut new_rows = Vec::new();
                for existing in &rows {
                    for new in &pattern_rows {
                        // Check if cartesian product would exceed threshold
                        if new_rows.len() >= MAX_INTERMEDIATE_ROWS {
                            return Err(anyhow::anyhow!(
                                "Query cartesian product too large. Patterns joined would create {} rows (max: {}). \
                                 Consider using WHERE clauses to reduce the result set before joining patterns.",
                                new_rows.len(),
                                MAX_INTERMEDIATE_ROWS
                            ));
                        }

                        let mut merged = existing.clone();
                        merged.extend(new.iter().map(|(k, v)| (k.clone(), v.clone())));
                        new_rows.push(merged);
                    }
                }
                rows = new_rows;
            }
        }

        // Process path variable bindings
        for path_var in &match_clause.path_vars {
            rows = self.process_path_binding(rows, path_var)?;
        }

        Ok(rows)
    }

    fn match_pattern(&self, pattern: &Pattern) -> Result<Vec<Row>> {
        match pattern {
            Pattern::Node(node_pat) => {
                let label = node_pat.label.as_deref();
                let var = node_pat.variable.as_deref().unwrap_or("_");

                match label {
                    Some("CodeSymbol") | None => Ok(self
                        .symbols
                        .iter()
                        .map(|s| {
                            let mut row = Row::new();
                            row.insert(var.to_string(), symbol_to_value(s));
                            row
                        })
                        .collect()),
                    Some("File") => Ok(self
                        .files
                        .iter()
                        .map(|f| {
                            let mut row = Row::new();
                            row.insert(var.to_string(), file_to_value(f));
                            row
                        })
                        .collect()),
                    Some(other) => {
                        // This should not happen if validate_pattern_labels was called
                        Err(anyhow::anyhow!(
                            "Unknown label: `{}`. Supported labels are: CodeSymbol, File",
                            other
                        ))
                    }
                }
            }
            Pattern::Relationship(start, rel, end) => {
                let start_var = start.variable.as_deref().unwrap_or("a");
                let end_var = end.variable.as_deref().unwrap_or("b");
                let rel_type = rel.rel_type.as_deref();

                let mut rows = Vec::new();

                let uid_to_symbol: HashMap<&str, &CodeSymbol> =
                    self.symbols.iter().map(|s| (s.uid.as_str(), s)).collect();

                for r in &self.relationships {
                    // Filter by relationship type if specified
                    if let Some(rt) = rel_type {
                        if r.kind.to_string() != rt {
                            continue;
                        }
                    }

                    let (source, target) = match rel.direction {
                        Direction::Right => (&r.source_uid, &r.target_uid),
                        Direction::Left => (&r.target_uid, &r.source_uid),
                        Direction::Both => (&r.source_uid, &r.target_uid),
                    };

                    // Filter by label
                    let source_sym = uid_to_symbol.get(source.as_str());
                    let target_sym = uid_to_symbol.get(target.as_str());

                    if let (Some(src), Some(tgt)) = (source_sym, target_sym) {
                        // Check label filters
                        if let Some(label) = &start.label {
                            if label != "CodeSymbol" {
                                continue;
                            }
                        }
                        if let Some(label) = &end.label {
                            if label != "CodeSymbol" {
                                continue;
                            }
                        }

                        let mut row = Row::new();
                        row.insert(start_var.to_string(), symbol_to_value(src));
                        row.insert(end_var.to_string(), symbol_to_value(tgt));
                        if let Some(rv) = &rel.variable {
                            row.insert(
                                rv.clone(),
                                json!({
                                    "kind": r.kind.to_string(),
                                    "source_uid": r.source_uid,
                                    "target_uid": r.target_uid,
                                }),
                            );
                        }
                        rows.push(row);
                    }
                }

                Ok(rows)
            }
        }
    }

    fn apply_where(&self, rows: Vec<Row>, where_clause: &WhereClause) -> Vec<Row> {
        rows.into_iter()
            .filter(|row| self.eval_expr(&where_clause.expr, row).as_bool())
            .collect()
    }

    fn eval_expr(&self, expr: &Expr, row: &Row) -> EvalResult {
        match expr {
            Expr::Property(var, prop) => {
                if let Some(node_val) = row.get(var) {
                    if let Some(obj) = node_val.as_object() {
                        EvalResult::Value(obj.get(prop).cloned().unwrap_or(Value::Null))
                    } else {
                        EvalResult::Null
                    }
                } else {
                    EvalResult::Null
                }
            }
            Expr::Ident(name) => {
                if let Some(val) = row.get(name) {
                    EvalResult::Value(val.clone())
                } else {
                    EvalResult::Null
                }
            }
            Expr::StringLit(s) => EvalResult::Value(Value::String(s.clone())),
            Expr::IntLit(n) => EvalResult::Value(json!(n)),
            Expr::FloatLit(n) => EvalResult::Value(json!(n)),
            Expr::BoolLit(b) => EvalResult::Value(json!(b)),
            Expr::Null => EvalResult::Null,
            Expr::BinOp(left, op, right) => {
                let l = self.eval_expr(left, row);
                let r = self.eval_expr(right, row);
                match op {
                    BinOp::And => EvalResult::Value(json!(l.as_bool() && r.as_bool())),
                    BinOp::Or => EvalResult::Value(json!(l.as_bool() || r.as_bool())),
                    BinOp::Eq => EvalResult::Value(json!(l.to_value() == r.to_value())),
                    BinOp::Neq => EvalResult::Value(json!(l.to_value() != r.to_value())),
                    BinOp::Lt => {
                        // Compare as strings if both are strings, otherwise as numbers
                        let cmp = compare_operands_for_ordering(&l, &r);
                        EvalResult::Value(json!(cmp == std::cmp::Ordering::Less))
                    }
                    BinOp::Gt => {
                        let cmp = compare_operands_for_ordering(&l, &r);
                        EvalResult::Value(json!(cmp == std::cmp::Ordering::Greater))
                    }
                    BinOp::Lte => {
                        let cmp = compare_operands_for_ordering(&l, &r);
                        EvalResult::Value(json!(
                            cmp == std::cmp::Ordering::Less || cmp == std::cmp::Ordering::Equal
                        ))
                    }
                    BinOp::Gte => {
                        let cmp = compare_operands_for_ordering(&l, &r);
                        EvalResult::Value(json!(
                            cmp == std::cmp::Ordering::Greater || cmp == std::cmp::Ordering::Equal
                        ))
                    }
                }
            }
            Expr::Not(inner) => {
                let result = self.eval_expr(inner, row);
                EvalResult::Value(json!(!result.as_bool()))
            }
            Expr::Contains(left, right) => {
                let l = self.eval_expr(left, row).as_string();
                let r = self.eval_expr(right, row).as_string();
                EvalResult::Value(json!(l.to_lowercase().contains(&r.to_lowercase())))
            }
            Expr::IsNull(inner) => {
                let result = self.eval_expr(inner, row);
                EvalResult::Value(json!(result.is_null()))
            }
            Expr::IsNotNull(inner) => {
                let result = self.eval_expr(inner, row);
                EvalResult::Value(json!(!result.is_null()))
            }
            Expr::FunctionCall(name, args) => {
                match name.to_lowercase().as_str() {
                    "count" => EvalResult::Value(json!(1)), // Simplified
                    "tolower" | "lower" => {
                        if let Some(arg) = args.first() {
                            let val = self.eval_expr(arg, row).as_string();
                            EvalResult::Value(json!(val.to_lowercase()))
                        } else {
                            EvalResult::Null
                        }
                    }
                    "toupper" | "upper" => {
                        if let Some(arg) = args.first() {
                            let val = self.eval_expr(arg, row).as_string();
                            EvalResult::Value(json!(val.to_uppercase()))
                        } else {
                            EvalResult::Null
                        }
                    }
                    _ => EvalResult::Null,
                }
            }
            Expr::Aggregation(_, _) => {
                // Aggregation should only be evaluated in aggregation context
                EvalResult::Null
            }
        }
    }

    fn apply_return(&self, rows: Vec<Row>, query: &Query) -> Vec<Row> {
        rows.into_iter()
            .map(|row| {
                let mut result = Row::new();
                for item in &query.return_clause.items {
                    let key = item.alias.clone().unwrap_or_else(|| expr_key(&item.expr));
                    let val = self.eval_expr(&item.expr, &row);
                    result.insert(key, val.to_value());
                }
                result
            })
            .collect()
    }

    fn apply_aggregation(&self, rows: Vec<Row>, query: &Query) -> Vec<Row> {
        // Infer GROUP BY columns from non-aggregated RETURN items
        let group_by_cols: Vec<String> = query
            .return_clause
            .items
            .iter()
            .filter(|item| !contains_aggregation(&item.expr))
            .map(|item| item.alias.clone().unwrap_or_else(|| expr_key(&item.expr)))
            .collect();

        if group_by_cols.is_empty() {
            // No GROUP BY columns: aggregate all rows into single result
            let mut result = Row::new();
            for item in &query.return_clause.items {
                let key = item.alias.clone().unwrap_or_else(|| expr_key(&item.expr));
                let val = self.eval_aggregation(&item.expr, &rows);
                result.insert(key, val);
            }
            return vec![result];
        }

        // GROUP BY: group rows by non-aggregated columns
        let mut groups: HashMap<Vec<Value>, Vec<Row>> = HashMap::new();
        for row in rows {
            let group_key: Vec<Value> = group_by_cols
                .iter()
                .map(|col| row.get(col).cloned().unwrap_or(Value::Null))
                .collect();
            groups.entry(group_key).or_default().push(row);
        }

        // Compute aggregations for each group
        let mut results = Vec::new();
        for (group_key, group_rows) in groups {
            let mut result = Row::new();

            // Add GROUP BY columns
            for (i, col) in group_by_cols.iter().enumerate() {
                result.insert(col.clone(), group_key[i].clone());
            }

            // Add aggregated columns
            for item in &query.return_clause.items {
                if !contains_aggregation(&item.expr) {
                    continue; // Already added
                }
                let key = item.alias.clone().unwrap_or_else(|| expr_key(&item.expr));
                let val = self.eval_aggregation(&item.expr, &group_rows);
                result.insert(key, val);
            }

            results.push(result);
        }

        results
    }

    fn eval_aggregation(&self, expr: &Expr, rows: &[Row]) -> Value {
        match expr {
            Expr::Aggregation(agg_func, inner_expr) => match agg_func {
                AggregationFunc::Count => {
                    json!(rows
                        .iter()
                        .filter(|row| {
                            let val = self.eval_expr(inner_expr, row);
                            !val.is_null()
                        })
                        .count())
                }
                AggregationFunc::Sum => {
                    let sum: f64 = rows
                        .iter()
                        .map(|row| self.eval_expr(inner_expr, row).as_f64())
                        .sum();
                    json!(sum)
                }
                AggregationFunc::Avg => {
                    let values: Vec<f64> = rows
                        .iter()
                        .map(|row| self.eval_expr(inner_expr, row).as_f64())
                        .collect();
                    let avg = if values.is_empty() {
                        0.0
                    } else {
                        values.iter().sum::<f64>() / values.len() as f64
                    };
                    json!(avg)
                }
                AggregationFunc::Min => {
                    let vals: Vec<f64> = rows
                        .iter()
                        .map(|row| self.eval_expr(inner_expr, row).as_f64())
                        .collect();
                    let min = vals.iter().cloned().fold(f64::INFINITY, f64::min);
                    if min.is_infinite() {
                        Value::Null
                    } else {
                        json!(min)
                    }
                }
                AggregationFunc::Max => {
                    let vals: Vec<f64> = rows
                        .iter()
                        .map(|row| self.eval_expr(inner_expr, row).as_f64())
                        .collect();
                    let max = vals.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
                    if max.is_infinite() {
                        Value::Null
                    } else {
                        json!(max)
                    }
                }
                AggregationFunc::Collect => {
                    let collected: Vec<Value> = rows
                        .iter()
                        .map(|row| self.eval_expr(inner_expr, row).to_value())
                        .filter(|v| !matches!(v, Value::Null))
                        .collect();
                    Value::Array(collected)
                }
            },
            _ => {
                // Non-aggregation expression: just evaluate first row
                if let Some(first) = rows.first() {
                    self.eval_expr(expr, first).to_value()
                } else {
                    Value::Null
                }
            }
        }
    }

    fn apply_order_by(&self, rows: &mut [Row], order_by: &crate::parser::OrderByClause) {
        rows.sort_by(|a, b| {
            for (expr, ascending) in &order_by.items {
                let key = expr_key(expr);
                let va = a.get(&key).cloned().unwrap_or(Value::Null);
                let vb = b.get(&key).cloned().unwrap_or(Value::Null);
                let cmp = compare_values(&va, &vb);
                let cmp = if *ascending { cmp } else { cmp.reverse() };
                if cmp != std::cmp::Ordering::Equal {
                    return cmp;
                }
            }
            std::cmp::Ordering::Equal
        });
    }

    fn process_path_binding(
        &self,
        rows: Vec<Row>,
        path_var: &PathVariableBinding,
    ) -> Result<Vec<Row>> {
        let uid_to_symbol: HashMap<&str, &CodeSymbol> =
            self.symbols.iter().map(|s| (s.uid.as_str(), s)).collect();

        let result_rows = match &path_var.path_fn {
            PathFunction::ShortestPath(args) => {
                let mut result = Vec::new();
                for row in rows {
                    let paths = self.find_shortest_paths(&row, args, &uid_to_symbol)?;
                    if paths.is_empty() {
                        // No path found, still include the row with path = null
                        let mut new_row = row.clone();
                        new_row.insert(path_var.variable.clone(), Value::Null);
                        result.push(new_row);
                    } else {
                        // Return one row per path found
                        for path in paths {
                            let mut new_row = row.clone();
                            new_row.insert(path_var.variable.clone(), self.path_to_json(&path));
                            result.push(new_row);
                        }
                    }
                }
                result
            }
            PathFunction::AllPaths(args) => {
                let mut result = Vec::new();
                for row in rows {
                    let paths = self.find_all_paths(&row, args, &uid_to_symbol)?;
                    if paths.is_empty() {
                        let mut new_row = row.clone();
                        new_row.insert(path_var.variable.clone(), Value::Null);
                        result.push(new_row);
                    } else {
                        for path in paths {
                            let mut new_row = row.clone();
                            new_row.insert(path_var.variable.clone(), self.path_to_json(&path));
                            result.push(new_row);
                        }
                    }
                }
                result
            }
            PathFunction::AnyPath(args) => {
                let mut result = Vec::new();
                for row in rows {
                    let paths = self.find_any_path(&row, args, &uid_to_symbol)?;
                    if paths.is_empty() {
                        let mut new_row = row.clone();
                        new_row.insert(path_var.variable.clone(), Value::Null);
                        result.push(new_row);
                    } else {
                        for path in paths.into_iter().take(1) {
                            let mut new_row = row.clone();
                            new_row.insert(path_var.variable.clone(), self.path_to_json(&path));
                            result.push(new_row);
                        }
                    }
                }
                result
            }
        };

        Ok(result_rows)
    }

    fn find_shortest_paths(
        &self,
        row: &Row,
        args: &crate::parser::PathFunctionArgs,
        uid_to_symbol: &HashMap<&str, &CodeSymbol>,
    ) -> Result<Vec<Path>> {
        let start_uid = match &args.start {
            Some(var) => {
                if let Some(node) = row.get(var) {
                    if let Some(uid) = node.get("uid").and_then(|u| u.as_str()) {
                        uid.to_string()
                    } else {
                        return Ok(Vec::new());
                    }
                } else {
                    return Ok(Vec::new());
                }
            }
            None => return Ok(Vec::new()),
        };

        let end_uid = match &args.end {
            Some(var) => {
                if let Some(node) = row.get(var) {
                    if let Some(uid) = node.get("uid").and_then(|u| u.as_str()) {
                        uid.to_string()
                    } else {
                        return Ok(Vec::new());
                    }
                } else {
                    return Ok(Vec::new());
                }
            }
            None => return Ok(Vec::new()),
        };

        let max_depth = args.max_depth.unwrap_or(5).min(10) as usize;

        // BFS to find shortest path
        let path = self.bfs_shortest_path(&start_uid, &end_uid, max_depth, uid_to_symbol)?;
        Ok(path.into_iter().collect())
    }

    fn bfs_shortest_path(
        &self,
        start: &str,
        end: &str,
        max_depth: usize,
        uid_to_symbol: &HashMap<&str, &CodeSymbol>,
    ) -> Result<Option<Path>> {
        if start == end {
            // Path to self
            if let Some(sym) = uid_to_symbol.get(start) {
                return Ok(Some(Path {
                    nodes: vec![PathNode {
                        uid: start.to_string(),
                        symbol: symbol_to_value(sym),
                    }],
                    edges: vec![],
                }));
            }
            return Ok(None);
        }

        let mut queue: VecDeque<(String, Vec<PathNode>, Vec<PathEdge>)> = VecDeque::new();
        let mut visited: std::collections::HashSet<String> = std::collections::HashSet::new();

        if let Some(start_sym) = uid_to_symbol.get(start) {
            queue.push_back((
                start.to_string(),
                vec![PathNode {
                    uid: start.to_string(),
                    symbol: symbol_to_value(start_sym),
                }],
                vec![],
            ));
            visited.insert(start.to_string());
        } else {
            return Ok(None);
        }

        while let Some((current_uid, nodes, edges)) = queue.pop_front() {
            if nodes.len() > max_depth {
                continue;
            }

            for rel in &self.relationships {
                let next_uid = if rel.source_uid == current_uid {
                    &rel.target_uid
                } else if rel.target_uid == current_uid {
                    &rel.source_uid
                } else {
                    continue;
                };

                if next_uid == end {
                    let mut final_nodes = nodes.clone();
                    let mut final_edges = edges.clone();

                    if let Some(sym) = uid_to_symbol.get(next_uid.as_str()) {
                        final_nodes.push(PathNode {
                            uid: next_uid.clone(),
                            symbol: symbol_to_value(sym),
                        });
                        final_edges.push(PathEdge {
                            source_uid: current_uid.clone(),
                            target_uid: next_uid.clone(),
                            relationship: json!({
                                "kind": rel.kind.to_string(),
                                "source_uid": rel.source_uid,
                                "target_uid": rel.target_uid,
                            }),
                        });

                        return Ok(Some(Path {
                            nodes: final_nodes,
                            edges: final_edges,
                        }));
                    }
                } else if !visited.contains(next_uid) && nodes.len() < max_depth {
                    visited.insert(next_uid.clone());

                    if let Some(sym) = uid_to_symbol.get(next_uid.as_str()) {
                        let mut new_nodes = nodes.clone();
                        let mut new_edges = edges.clone();

                        new_nodes.push(PathNode {
                            uid: next_uid.clone(),
                            symbol: symbol_to_value(sym),
                        });
                        new_edges.push(PathEdge {
                            source_uid: current_uid.clone(),
                            target_uid: next_uid.clone(),
                            relationship: json!({
                                "kind": rel.kind.to_string(),
                                "source_uid": rel.source_uid,
                                "target_uid": rel.target_uid,
                            }),
                        });

                        queue.push_back((next_uid.clone(), new_nodes, new_edges));
                    }
                }
            }
        }

        Ok(None)
    }

    fn find_all_paths(
        &self,
        row: &Row,
        args: &crate::parser::PathFunctionArgs,
        uid_to_symbol: &HashMap<&str, &CodeSymbol>,
    ) -> Result<Vec<Path>> {
        let start_uid = match &args.start {
            Some(var) => {
                if let Some(node) = row.get(var) {
                    if let Some(uid) = node.get("uid").and_then(|u| u.as_str()) {
                        uid.to_string()
                    } else {
                        return Ok(Vec::new());
                    }
                } else {
                    return Ok(Vec::new());
                }
            }
            None => return Ok(Vec::new()),
        };

        let end_uid = match &args.end {
            Some(var) => {
                if let Some(node) = row.get(var) {
                    if let Some(uid) = node.get("uid").and_then(|u| u.as_str()) {
                        uid.to_string()
                    } else {
                        return Ok(Vec::new());
                    }
                } else {
                    return Ok(Vec::new());
                }
            }
            None => return Ok(Vec::new()),
        };

        let max_depth = args.max_depth.unwrap_or(5).min(10) as usize;

        // DFS to find all paths
        let mut all_paths = Vec::new();
        let mut visited = std::collections::HashSet::new();

        if let Some(start_sym) = uid_to_symbol.get(start_uid.as_str()) {
            self.dfs_all_paths(
                &start_uid,
                &end_uid,
                vec![PathNode {
                    uid: start_uid.clone(),
                    symbol: symbol_to_value(start_sym),
                }],
                vec![],
                &mut visited,
                0,
                max_depth,
                uid_to_symbol,
                &mut all_paths,
            );
        }

        Ok(all_paths)
    }

    #[allow(clippy::too_many_arguments)]
    fn dfs_all_paths(
        &self,
        current: &str,
        target: &str,
        nodes: Vec<PathNode>,
        edges: Vec<PathEdge>,
        visited: &mut std::collections::HashSet<String>,
        depth: usize,
        max_depth: usize,
        uid_to_symbol: &HashMap<&str, &CodeSymbol>,
        all_paths: &mut Vec<Path>,
    ) {
        if current == target {
            all_paths.push(Path { nodes, edges });
            return;
        }

        if depth >= max_depth {
            return;
        }

        visited.insert(current.to_string());

        for rel in &self.relationships {
            let next_uid = if rel.source_uid == current {
                &rel.target_uid
            } else if rel.target_uid == current {
                &rel.source_uid
            } else {
                continue;
            };

            if !visited.contains(next_uid) {
                if let Some(sym) = uid_to_symbol.get(next_uid.as_str()) {
                    let mut new_nodes = nodes.clone();
                    let mut new_edges = edges.clone();

                    new_nodes.push(PathNode {
                        uid: next_uid.clone(),
                        symbol: symbol_to_value(sym),
                    });
                    new_edges.push(PathEdge {
                        source_uid: current.to_string(),
                        target_uid: next_uid.clone(),
                        relationship: json!({
                            "kind": rel.kind.to_string(),
                            "source_uid": rel.source_uid,
                            "target_uid": rel.target_uid,
                        }),
                    });

                    self.dfs_all_paths(
                        next_uid,
                        target,
                        new_nodes,
                        new_edges,
                        visited,
                        depth + 1,
                        max_depth,
                        uid_to_symbol,
                        all_paths,
                    );
                }
            }
        }

        visited.remove(current);
    }

    fn find_any_path(
        &self,
        row: &Row,
        args: &crate::parser::PathFunctionArgs,
        uid_to_symbol: &HashMap<&str, &CodeSymbol>,
    ) -> Result<Vec<Path>> {
        let start_uid = match &args.start {
            Some(var) => {
                if let Some(node) = row.get(var) {
                    if let Some(uid) = node.get("uid").and_then(|u| u.as_str()) {
                        uid.to_string()
                    } else {
                        return Ok(Vec::new());
                    }
                } else {
                    return Ok(Vec::new());
                }
            }
            None => return Ok(Vec::new()),
        };

        let end_uid = match &args.end {
            Some(var) => {
                if let Some(node) = row.get(var) {
                    if let Some(uid) = node.get("uid").and_then(|u| u.as_str()) {
                        uid.to_string()
                    } else {
                        return Ok(Vec::new());
                    }
                } else {
                    return Ok(Vec::new());
                }
            }
            None => return Ok(Vec::new()),
        };

        let max_depth = args.max_depth.unwrap_or(5).min(10) as usize;

        // DFS to find any path (returns first path found)
        let mut visited = std::collections::HashSet::new();

        if let Some(start_sym) = uid_to_symbol.get(start_uid.as_str()) {
            let result = self.dfs_any_path(
                &start_uid,
                &end_uid,
                vec![PathNode {
                    uid: start_uid.clone(),
                    symbol: symbol_to_value(start_sym),
                }],
                vec![],
                &mut visited,
                0,
                max_depth,
                uid_to_symbol,
            );

            return Ok(result.into_iter().collect());
        }

        Ok(Vec::new())
    }

    #[allow(clippy::too_many_arguments)]
    fn dfs_any_path(
        &self,
        current: &str,
        target: &str,
        nodes: Vec<PathNode>,
        edges: Vec<PathEdge>,
        visited: &mut std::collections::HashSet<String>,
        depth: usize,
        max_depth: usize,
        uid_to_symbol: &HashMap<&str, &CodeSymbol>,
    ) -> Option<Path> {
        if current == target {
            return Some(Path { nodes, edges });
        }

        if depth >= max_depth {
            return None;
        }

        visited.insert(current.to_string());

        for rel in &self.relationships {
            let next_uid = if rel.source_uid == current {
                &rel.target_uid
            } else if rel.target_uid == current {
                &rel.source_uid
            } else {
                continue;
            };

            if !visited.contains(next_uid) {
                if let Some(sym) = uid_to_symbol.get(next_uid.as_str()) {
                    let mut new_nodes = nodes.clone();
                    let mut new_edges = edges.clone();

                    new_nodes.push(PathNode {
                        uid: next_uid.clone(),
                        symbol: symbol_to_value(sym),
                    });
                    new_edges.push(PathEdge {
                        source_uid: current.to_string(),
                        target_uid: next_uid.clone(),
                        relationship: json!({
                            "kind": rel.kind.to_string(),
                            "source_uid": rel.source_uid,
                            "target_uid": rel.target_uid,
                        }),
                    });

                    let result = self.dfs_any_path(
                        next_uid,
                        target,
                        new_nodes,
                        new_edges,
                        visited,
                        depth + 1,
                        max_depth,
                        uid_to_symbol,
                    );

                    if result.is_some() {
                        visited.remove(current);
                        return result;
                    }
                }
            }
        }

        visited.remove(current);
        None
    }

    fn path_to_json(&self, path: &Path) -> Value {
        json!({
            "nodes": path.nodes.iter().map(|n| json!({
                "uid": n.uid,
                "symbol": n.symbol,
            })).collect::<Vec<_>>(),
            "edges": path.edges.iter().map(|e| json!({
                "source": e.source_uid,
                "target": e.target_uid,
                "relationship": e.relationship,
            })).collect::<Vec<_>>(),
        })
    }

    /// Validate an entity type against the ontology if one is loaded.
    pub fn validate_entity_type(&self, entity_type: &str) -> Result<()> {
        if let Some(ref ontology) = self.ontology {
            ontology
                .validate_entity_type(entity_type)
                .map_err(|e| anyhow::anyhow!(e))
        } else {
            Ok(())
        }
    }

    /// Validate an edge type against the ontology if one is loaded.
    pub fn validate_edge_type(&self, edge_type: &str) -> Result<()> {
        if let Some(ref ontology) = self.ontology {
            ontology
                .validate_edge_type(edge_type)
                .map_err(|e| anyhow::anyhow!(e))
        } else {
            Ok(())
        }
    }

    /// Get schema information for an entity type from the ontology.
    pub fn get_entity_schema(&self, entity_type: &str) -> Result<Vec<String>> {
        if let Some(ref ontology) = self.ontology {
            let properties = ontology
                .get_entity_schema(entity_type)
                .map_err(|e| anyhow::anyhow!(e))?;
            Ok(properties.iter().map(|p| p.name.clone()).collect())
        } else {
            Ok(vec![])
        }
    }

    /// Get schema information for an edge type from the ontology.
    pub fn get_edge_schema(&self, edge_type: &str) -> Result<Vec<String>> {
        if let Some(ref ontology) = self.ontology {
            let properties = ontology
                .get_edge_schema(edge_type)
                .map_err(|e| anyhow::anyhow!(e))?;
            Ok(properties.iter().map(|p| p.name.clone()).collect())
        } else {
            Ok(vec![])
        }
    }

    /// Get access to the ontology if loaded.
    pub fn ontology(&self) -> Option<&Ontology> {
        self.ontology.as_ref()
    }
}

#[derive(Debug)]
enum EvalResult {
    Value(Value),
    Null,
}

impl EvalResult {
    fn as_bool(&self) -> bool {
        match self {
            EvalResult::Value(v) => v.as_bool().unwrap_or(false),
            EvalResult::Null => false,
        }
    }

    fn as_string(&self) -> String {
        match self {
            EvalResult::Value(v) => match v {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            },
            EvalResult::Null => String::new(),
        }
    }

    fn as_f64(&self) -> f64 {
        match self {
            EvalResult::Value(v) => v.as_f64().unwrap_or(0.0),
            EvalResult::Null => 0.0,
        }
    }

    fn is_null(&self) -> bool {
        matches!(self, EvalResult::Null) || matches!(self, EvalResult::Value(Value::Null))
    }

    fn to_value(&self) -> Value {
        match self {
            EvalResult::Value(v) => v.clone(),
            EvalResult::Null => Value::Null,
        }
    }
}

fn symbol_to_value(s: &CodeSymbol) -> Value {
    let mut obj = json!({
        "uid": s.uid,
        "name": s.name,
        "qualified_name": s.qualified_name,
        "kind": s.kind.to_string(),
        "file_path": s.file_path,
        "start_line": s.start_line,
        "end_line": s.end_line,
        "signature": s.signature,
    });
    // Include parsed metadata if present
    if let Some(ref meta_json) = s.metadata {
        if let Ok(meta) = serde_json::from_str::<Value>(meta_json) {
            obj["metadata"] = meta;
        }
    }
    obj
}

fn file_to_value(f: &FileNode) -> Value {
    json!({
        "uid": f.uid,
        "path": f.path,
        "language": f.language,
        "num_symbols": f.num_symbols,
    })
}

fn expr_key(expr: &Expr) -> String {
    match expr {
        Expr::Property(var, prop) => format!("{}.{}", var, prop),
        Expr::Ident(name) => name.clone(),
        Expr::FunctionCall(name, _) => name.clone(),
        Expr::Aggregation(agg_func, inner) => {
            let func_name = match agg_func {
                AggregationFunc::Count => "count",
                AggregationFunc::Sum => "sum",
                AggregationFunc::Avg => "avg",
                AggregationFunc::Min => "min",
                AggregationFunc::Max => "max",
                AggregationFunc::Collect => "collect",
            };
            format!("{}({})", func_name, expr_key(inner))
        }
        _ => "?".to_string(),
    }
}

fn compare_values(a: &Value, b: &Value) -> std::cmp::Ordering {
    match (a, b) {
        (Value::String(a), Value::String(b)) => a.cmp(b),
        (Value::Number(a), Value::Number(b)) => a
            .as_f64()
            .unwrap_or(0.0)
            .partial_cmp(&b.as_f64().unwrap_or(0.0))
            .unwrap_or(std::cmp::Ordering::Equal),
        _ => std::cmp::Ordering::Equal,
    }
}

fn compare_operands_for_ordering(left: &EvalResult, right: &EvalResult) -> std::cmp::Ordering {
    // If both are strings, compare as strings
    let left_str = left.as_string();
    let right_str = right.as_string();
    
    // Check if both values are numeric (either JSON numbers or strings that parse as numbers)
    let left_num = left.to_value().as_f64();
    let right_num = right.to_value().as_f64();
    
    // If both are numeric, compare as numbers
    if let (Some(l), Some(r)) = (left_num, right_num) {
        return l.partial_cmp(&r).unwrap_or(std::cmp::Ordering::Equal);
    }
    
    // Otherwise, compare as strings
    left_str.cmp(&right_str)
}

fn contains_aggregation(expr: &Expr) -> bool {
    match expr {
        Expr::Aggregation(_, _) => true,
        Expr::BinOp(left, _, right) => contains_aggregation(left) || contains_aggregation(right),
        Expr::Not(inner) | Expr::IsNull(inner) | Expr::IsNotNull(inner) => {
            contains_aggregation(inner)
        }
        Expr::Contains(left, right) => contains_aggregation(left) || contains_aggregation(right),
        Expr::FunctionCall(_, args) => args.iter().any(contains_aggregation),
        _ => false,
    }
}
