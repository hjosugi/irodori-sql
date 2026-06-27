//! GEN-001 — SQL abstract syntax tree for the `SELECT` family.
//!
//! This is the "perfect syntax tree" that grounds local SQL generation: the
//! [`grammar`](crate::grammar) module emits a GBNF grammar that produces only
//! strings this AST can represent, the [`parser`](crate::parser) module parses
//! generated SQL back into this AST, and downstream crates validate/repair at the
//! AST level. Scope is intentionally the generation target (`SELECT`); DML/DDL
//! grow here over later iterations.
//!
//! [`SelectStatement::render`] produces a canonical, dialect-quoted string. The
//! round-trip invariant the tests rely on is *AST stability*:
//! `parse(render(ast)) == ast`.

use crate::dialect::SqlDialect;
use std::fmt::Write as _;

/// A top-level `SELECT` query (optionally with leading CTEs).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SelectStatement {
    pub with: Vec<Cte>,
    pub distinct: bool,
    pub projection: Vec<SelectItem>,
    pub from: Option<TableExpr>,
    pub joins: Vec<Join>,
    pub filter: Option<Expr>,
    pub group_by: Vec<Expr>,
    pub having: Option<Expr>,
    pub order_by: Vec<OrderByItem>,
    pub limit: Option<u64>,
    pub offset: Option<u64>,
}

/// A `WITH` common-table-expression.
#[derive(Debug, Clone, PartialEq)]
pub struct Cte {
    pub name: String,
    pub columns: Vec<String>,
    pub query: Box<SelectStatement>,
}

/// One entry in the `SELECT` projection list.
#[derive(Debug, Clone, PartialEq)]
pub enum SelectItem {
    /// `*`
    Wildcard,
    /// `alias.*`
    QualifiedWildcard(String),
    /// `expr` or `expr AS alias`
    Expr { expr: Expr, alias: Option<String> },
}

/// A base table reference with an optional alias (`schema.table t`).
#[derive(Debug, Clone, PartialEq)]
pub struct TableExpr {
    pub name: ObjectName,
    pub alias: Option<String>,
}

/// A possibly schema-qualified object name, most-significant part first.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ObjectName(pub Vec<String>);

impl ObjectName {
    pub fn bare(name: impl Into<String>) -> Self {
        Self(vec![name.into()])
    }

    /// The final (object) component, e.g. `table` in `schema.table`.
    pub fn object(&self) -> &str {
        self.0.last().map(String::as_str).unwrap_or_default()
    }

    /// The qualifier component (schema), if the name is qualified.
    pub fn schema(&self) -> Option<&str> {
        if self.0.len() >= 2 {
            self.0.get(self.0.len() - 2).map(String::as_str)
        } else {
            None
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JoinKind {
    Inner,
    Left,
    Right,
    Full,
    Cross,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Join {
    pub kind: JoinKind,
    pub table: TableExpr,
    /// `ON` condition; `None` only for `CROSS JOIN`.
    pub on: Option<Expr>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortDir {
    Asc,
    Desc,
}

#[derive(Debug, Clone, PartialEq)]
pub struct OrderByItem {
    pub expr: Expr,
    pub dir: Option<SortDir>,
}

/// A scalar expression.
#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    Column(ColumnRef),
    Literal(Literal),
    /// A positional parameter placeholder (rendered per dialect).
    Param(usize),
    Unary {
        op: UnaryOp,
        expr: Box<Expr>,
    },
    Binary {
        left: Box<Expr>,
        op: BinaryOp,
        right: Box<Expr>,
    },
    Function {
        name: String,
        distinct: bool,
        args: Vec<FuncArg>,
    },
    Case {
        operand: Option<Box<Expr>>,
        whens: Vec<(Expr, Expr)>,
        else_expr: Option<Box<Expr>>,
    },
    InList {
        expr: Box<Expr>,
        list: Vec<Expr>,
        negated: bool,
    },
    Between {
        expr: Box<Expr>,
        low: Box<Expr>,
        high: Box<Expr>,
        negated: bool,
    },
    IsNull {
        expr: Box<Expr>,
        negated: bool,
    },
    Paren(Box<Expr>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum FuncArg {
    /// `count(*)`
    Wildcard,
    Expr(Expr),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ColumnRef {
    pub qualifier: Option<String>,
    pub name: String,
}

impl ColumnRef {
    pub fn bare(name: impl Into<String>) -> Self {
        Self {
            qualifier: None,
            name: name.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Literal {
    Number(String),
    String(String),
    Boolean(bool),
    Null,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOp {
    Not,
    Neg,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOp {
    Eq,
    NotEq,
    Lt,
    LtEq,
    Gt,
    GtEq,
    Plus,
    Minus,
    Mul,
    Div,
    Mod,
    And,
    Or,
    Like,
    NotLike,
    Concat,
}

impl BinaryOp {
    /// Canonical SQL spelling.
    pub fn as_sql(self) -> &'static str {
        match self {
            BinaryOp::Eq => "=",
            BinaryOp::NotEq => "<>",
            BinaryOp::Lt => "<",
            BinaryOp::LtEq => "<=",
            BinaryOp::Gt => ">",
            BinaryOp::GtEq => ">=",
            BinaryOp::Plus => "+",
            BinaryOp::Minus => "-",
            BinaryOp::Mul => "*",
            BinaryOp::Div => "/",
            BinaryOp::Mod => "%",
            BinaryOp::And => "AND",
            BinaryOp::Or => "OR",
            BinaryOp::Like => "LIKE",
            BinaryOp::NotLike => "NOT LIKE",
            BinaryOp::Concat => "||",
        }
    }
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

impl SelectStatement {
    /// Render canonical, dialect-quoted SQL (uppercase keywords, single spaces).
    pub fn render(&self, dialect: &dyn SqlDialect) -> String {
        let mut out = String::new();
        self.render_into(&mut out, dialect);
        out
    }

    fn render_into(&self, out: &mut String, d: &dyn SqlDialect) {
        if !self.with.is_empty() {
            out.push_str("WITH ");
            for (i, cte) in self.with.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                out.push_str(&d.quote_identifier_if_needed(&cte.name));
                if !cte.columns.is_empty() {
                    out.push_str(" (");
                    render_ident_list(out, &cte.columns, d);
                    out.push(')');
                }
                out.push_str(" AS (");
                cte.query.render_into(out, d);
                out.push(')');
            }
            out.push(' ');
        }

        out.push_str("SELECT ");
        if self.distinct {
            out.push_str("DISTINCT ");
        }
        if self.projection.is_empty() {
            out.push('*');
        } else {
            for (i, item) in self.projection.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                render_select_item(out, item, d);
            }
        }

        if let Some(from) = &self.from {
            out.push_str(" FROM ");
            render_table_expr(out, from, d);
        }

        for join in &self.joins {
            render_join(out, join, d);
        }

        if let Some(filter) = &self.filter {
            out.push_str(" WHERE ");
            render_expr(out, filter, d);
        }

        if !self.group_by.is_empty() {
            out.push_str(" GROUP BY ");
            for (i, expr) in self.group_by.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                render_expr(out, expr, d);
            }
        }

        if let Some(having) = &self.having {
            out.push_str(" HAVING ");
            render_expr(out, having, d);
        }

        if !self.order_by.is_empty() {
            out.push_str(" ORDER BY ");
            for (i, item) in self.order_by.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                render_expr(out, &item.expr, d);
                match item.dir {
                    Some(SortDir::Asc) => out.push_str(" ASC"),
                    Some(SortDir::Desc) => out.push_str(" DESC"),
                    None => {}
                }
            }
        }

        if let Some(limit) = self.limit {
            let _ = write!(out, " LIMIT {limit}");
        }
        if let Some(offset) = self.offset {
            let _ = write!(out, " OFFSET {offset}");
        }
    }
}

fn render_ident_list(out: &mut String, idents: &[String], d: &dyn SqlDialect) {
    for (i, ident) in idents.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        out.push_str(&d.quote_identifier_if_needed(ident));
    }
}

fn render_select_item(out: &mut String, item: &SelectItem, d: &dyn SqlDialect) {
    match item {
        SelectItem::Wildcard => out.push('*'),
        SelectItem::QualifiedWildcard(q) => {
            out.push_str(&d.quote_identifier_if_needed(q));
            out.push_str(".*");
        }
        SelectItem::Expr { expr, alias } => {
            render_expr(out, expr, d);
            if let Some(alias) = alias {
                out.push_str(" AS ");
                out.push_str(&d.quote_identifier_if_needed(alias));
            }
        }
    }
}

fn render_table_expr(out: &mut String, table: &TableExpr, d: &dyn SqlDialect) {
    let parts: Vec<&str> = table.name.0.iter().map(String::as_str).collect();
    out.push_str(&d.quote_qualified_identifier_if_needed(&parts));
    if let Some(alias) = &table.alias {
        out.push(' ');
        out.push_str(&d.quote_identifier_if_needed(alias));
    }
}

fn render_join(out: &mut String, join: &Join, d: &dyn SqlDialect) {
    let keyword = match join.kind {
        JoinKind::Inner => " JOIN ",
        JoinKind::Left => " LEFT JOIN ",
        JoinKind::Right => " RIGHT JOIN ",
        JoinKind::Full => " FULL JOIN ",
        JoinKind::Cross => " CROSS JOIN ",
    };
    out.push_str(keyword);
    render_table_expr(out, &join.table, d);
    if let Some(on) = &join.on {
        out.push_str(" ON ");
        render_expr(out, on, d);
    }
}

/// Operator precedence used to parenthesize on render so the round trip is stable.
fn precedence(expr: &Expr) -> u8 {
    match expr {
        Expr::Binary { op, .. } => match op {
            BinaryOp::Or => 1,
            BinaryOp::And => 2,
            BinaryOp::Eq
            | BinaryOp::NotEq
            | BinaryOp::Lt
            | BinaryOp::LtEq
            | BinaryOp::Gt
            | BinaryOp::GtEq
            | BinaryOp::Like
            | BinaryOp::NotLike => 4,
            BinaryOp::Plus | BinaryOp::Minus | BinaryOp::Concat => 5,
            BinaryOp::Mul | BinaryOp::Div | BinaryOp::Mod => 6,
        },
        Expr::Unary { .. } | Expr::IsNull { .. } => 7,
        _ => 8,
    }
}

fn render_expr(out: &mut String, expr: &Expr, d: &dyn SqlDialect) {
    match expr {
        Expr::Column(col) => {
            if let Some(q) = &col.qualifier {
                out.push_str(&d.quote_identifier_if_needed(q));
                out.push('.');
            }
            out.push_str(&d.quote_identifier_if_needed(&col.name));
        }
        Expr::Literal(lit) => render_literal(out, lit),
        Expr::Param(n) => out.push_str(&d.placeholder(*n)),
        Expr::Unary { op, expr } => {
            match op {
                UnaryOp::Not => out.push_str("NOT "),
                UnaryOp::Neg => out.push('-'),
            }
            render_child(out, expr, 7, d);
        }
        Expr::Binary { left, op, right } => {
            let parent = precedence(expr);
            render_child(out, left, parent, d);
            out.push(' ');
            out.push_str(op.as_sql());
            out.push(' ');
            // Right side uses parent+1 so equal-precedence right operands parenthesize,
            // keeping left-associative reparse identical.
            render_child(out, right, parent + 1, d);
        }
        Expr::Function {
            name,
            distinct,
            args,
        } => {
            // Preserve the function name's spelling so the parse round trip is
            // stable; callers that want canonical casing normalize the AST.
            out.push_str(name);
            out.push('(');
            if *distinct {
                out.push_str("DISTINCT ");
            }
            for (i, arg) in args.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                match arg {
                    FuncArg::Wildcard => out.push('*'),
                    FuncArg::Expr(e) => render_expr(out, e, d),
                }
            }
            out.push(')');
        }
        Expr::Case {
            operand,
            whens,
            else_expr,
        } => {
            out.push_str("CASE");
            if let Some(operand) = operand {
                out.push(' ');
                render_expr(out, operand, d);
            }
            for (when, then) in whens {
                out.push_str(" WHEN ");
                render_expr(out, when, d);
                out.push_str(" THEN ");
                render_expr(out, then, d);
            }
            if let Some(else_expr) = else_expr {
                out.push_str(" ELSE ");
                render_expr(out, else_expr, d);
            }
            out.push_str(" END");
        }
        Expr::InList {
            expr,
            list,
            negated,
        } => {
            render_child(out, expr, 7, d);
            out.push_str(if *negated { " NOT IN (" } else { " IN (" });
            for (i, item) in list.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                render_expr(out, item, d);
            }
            out.push(')');
        }
        Expr::Between {
            expr,
            low,
            high,
            negated,
        } => {
            render_child(out, expr, 7, d);
            out.push_str(if *negated {
                " NOT BETWEEN "
            } else {
                " BETWEEN "
            });
            render_child(out, low, 7, d);
            out.push_str(" AND ");
            render_child(out, high, 7, d);
        }
        Expr::IsNull { expr, negated } => {
            render_child(out, expr, 7, d);
            out.push_str(if *negated {
                " IS NOT NULL"
            } else {
                " IS NULL"
            });
        }
        Expr::Paren(inner) => {
            out.push('(');
            render_expr(out, inner, d);
            out.push(')');
        }
    }
}

fn render_child(out: &mut String, expr: &Expr, parent_prec: u8, d: &dyn SqlDialect) {
    if precedence(expr) < parent_prec {
        out.push('(');
        render_expr(out, expr, d);
        out.push(')');
    } else {
        render_expr(out, expr, d);
    }
}

fn render_literal(out: &mut String, lit: &Literal) {
    match lit {
        Literal::Number(n) => out.push_str(n),
        Literal::String(s) => {
            out.push('\'');
            out.push_str(&s.replace('\'', "''"));
            out.push('\'');
        }
        Literal::Boolean(true) => out.push_str("TRUE"),
        Literal::Boolean(false) => out.push_str("FALSE"),
        Literal::Null => out.push_str("NULL"),
    }
}
