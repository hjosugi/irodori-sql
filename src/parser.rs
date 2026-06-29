//! GEN-003 — recursive-descent parser for the `SELECT` family.
//!
//! Parses SQL text into [`crate::ast`]. It is the validation/repair counterpart
//! to [`crate::grammar`]: generated SQL is parsed back into an AST so identifiers
//! and types can be checked. It is tolerant of casing/whitespace (so it accepts
//! grammar output and most hand-written SQL) but rejects anything outside the
//! supported `SELECT` subset rather than guessing.
//!
//! Parentheses are *folded* (no `Expr::Paren` is produced); grouping is recovered
//! from operator precedence. Together with the precedence-aware
//! [`SelectStatement::render`](crate::ast::SelectStatement::render) this gives the
//! AST-stability round trip `parse(render(ast)) == ast`.

use crate::ast::*;
use crate::dialect::SqlDialect;
use thiserror::Error;

/// A parse failure with a human-readable message and byte offset into the input.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{message} (at byte {position})")]
pub struct ParseError {
    pub message: String,
    pub position: usize,
}

/// Parse a single `SELECT` statement (an optional trailing `;` is allowed).
#[tracing::instrument(skip_all, fields(sql_bytes = sql.len()))]
pub fn parse_select(sql: &str, dialect: &dyn SqlDialect) -> Result<SelectStatement, ParseError> {
    let tokens = tokenize(sql, dialect)?;
    let token_count = tokens.len();
    let mut parser = Parser {
        tokens,
        pos: 0,
        _dialect: dialect,
    };
    let stmt = parser.parse_statement()?;
    parser.skip_semicolons();
    if let Some(tok) = parser.peek() {
        return Err(ParseError {
            message: format!("unexpected trailing input: {}", tok.kind.describe()),
            position: tok.position,
        });
    }
    tracing::debug!(token_count, "parsed select statement");
    Ok(stmt)
}

// ---------------------------------------------------------------------------
// Tokens
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
enum TokKind {
    /// Bare word (identifier or keyword); matched case-insensitively.
    Word(String),
    /// Quoted identifier value (quotes already stripped/unescaped).
    Quoted(String),
    Number(String),
    String(String),
    /// A parameter placeholder; the optional position if it was numeric.
    Param(Option<usize>),
    Comma,
    Dot,
    LParen,
    RParen,
    Star,
    Op(&'static str),
}

impl TokKind {
    fn describe(&self) -> String {
        match self {
            TokKind::Word(w) => format!("word `{w}`"),
            TokKind::Quoted(w) => format!("quoted `{w}`"),
            TokKind::Number(n) => format!("number `{n}`"),
            TokKind::String(_) => "string literal".to_string(),
            TokKind::Param(_) => "parameter".to_string(),
            TokKind::Comma => "`,`".to_string(),
            TokKind::Dot => "`.`".to_string(),
            TokKind::LParen => "`(`".to_string(),
            TokKind::RParen => "`)`".to_string(),
            TokKind::Star => "`*`".to_string(),
            TokKind::Op(op) => format!("`{op}`"),
        }
    }
}

#[derive(Debug, Clone)]
struct Token {
    kind: TokKind,
    position: usize,
}

fn tokenize(sql: &str, dialect: &dyn SqlDialect) -> Result<Vec<Token>, ParseError> {
    let bytes = sql.as_bytes();
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i] as char;
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        // Comments.
        if c == '-' && bytes.get(i + 1) == Some(&b'-') {
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if c == '/' && bytes.get(i + 1) == Some(&b'*') {
            i += 2;
            while i + 1 < bytes.len() && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                i += 1;
            }
            i += 2;
            continue;
        }
        let start = i;
        match c {
            ';' => {
                tokens.push(tok(TokKind::Op(";"), start));
                i += 1;
            }
            ',' => {
                tokens.push(tok(TokKind::Comma, start));
                i += 1;
            }
            '.' => {
                tokens.push(tok(TokKind::Dot, start));
                i += 1;
            }
            '(' => {
                tokens.push(tok(TokKind::LParen, start));
                i += 1;
            }
            ')' => {
                tokens.push(tok(TokKind::RParen, start));
                i += 1;
            }
            '*' => {
                tokens.push(tok(TokKind::Star, start));
                i += 1;
            }
            '+' => {
                tokens.push(tok(TokKind::Op("+"), start));
                i += 1;
            }
            '%' => {
                tokens.push(tok(TokKind::Op("%"), start));
                i += 1;
            }
            '/' => {
                tokens.push(tok(TokKind::Op("/"), start));
                i += 1;
            }
            '-' => {
                tokens.push(tok(TokKind::Op("-"), start));
                i += 1;
            }
            '|' => {
                if bytes.get(i + 1) == Some(&b'|') {
                    tokens.push(tok(TokKind::Op("||"), start));
                    i += 2;
                } else {
                    return Err(ParseError {
                        message: "unexpected `|`".into(),
                        position: start,
                    });
                }
            }
            '=' => {
                tokens.push(tok(TokKind::Op("="), start));
                i += 1;
            }
            '<' => {
                if bytes.get(i + 1) == Some(&b'=') {
                    tokens.push(tok(TokKind::Op("<="), start));
                    i += 2;
                } else if bytes.get(i + 1) == Some(&b'>') {
                    tokens.push(tok(TokKind::Op("<>"), start));
                    i += 2;
                } else {
                    tokens.push(tok(TokKind::Op("<"), start));
                    i += 1;
                }
            }
            '>' => {
                if bytes.get(i + 1) == Some(&b'=') {
                    tokens.push(tok(TokKind::Op(">="), start));
                    i += 2;
                } else {
                    tokens.push(tok(TokKind::Op(">"), start));
                    i += 1;
                }
            }
            '!' => {
                if bytes.get(i + 1) == Some(&b'=') {
                    tokens.push(tok(TokKind::Op("<>"), start));
                    i += 2;
                } else {
                    return Err(ParseError {
                        message: "unexpected `!`".into(),
                        position: start,
                    });
                }
            }
            '\'' => {
                let (value, next) = read_string(sql, i)?;
                tokens.push(tok(TokKind::String(value), start));
                i = next;
            }
            '"' | '`' | '[' => {
                let (value, next) = read_quoted_ident(sql, i, c)?;
                tokens.push(tok(TokKind::Quoted(value), start));
                i = next;
            }
            '?' => {
                tokens.push(tok(TokKind::Param(None), start));
                i += 1;
            }
            '$' | ':' | '@' => {
                let (value, next) = read_param(sql, i, c);
                tokens.push(tok(TokKind::Param(value), start));
                i = next;
            }
            _ if c.is_ascii_digit() => {
                let (value, next) = read_number(sql, i);
                tokens.push(tok(TokKind::Number(value), start));
                i = next;
            }
            _ if is_ident_start(c) => {
                let (value, next) = read_word(sql, i);
                tokens.push(tok(TokKind::Word(value), start));
                i = next;
            }
            _ => {
                return Err(ParseError {
                    message: format!("unexpected character `{c}`"),
                    position: start,
                });
            }
        }
    }
    let _ = dialect; // dialect-specific quoting is handled by char in read_quoted_ident
    Ok(tokens)
}

fn tok(kind: TokKind, position: usize) -> Token {
    Token { kind, position }
}

fn read_string(sql: &str, start: usize) -> Result<(String, usize), ParseError> {
    let bytes = sql.as_bytes();
    let mut i = start + 1;
    let mut value = String::new();
    while i < bytes.len() {
        let c = bytes[i] as char;
        if c == '\'' {
            if bytes.get(i + 1) == Some(&b'\'') {
                value.push('\'');
                i += 2;
                continue;
            }
            return Ok((value, i + 1));
        }
        value.push(c);
        i += 1;
    }
    Err(ParseError {
        message: "unterminated string literal".into(),
        position: start,
    })
}

fn read_quoted_ident(sql: &str, start: usize, open: char) -> Result<(String, usize), ParseError> {
    let close = match open {
        '[' => ']',
        other => other,
    };
    let bytes = sql.as_bytes();
    let mut i = start + 1;
    let mut value = String::new();
    while i < bytes.len() {
        let c = bytes[i] as char;
        if c == close {
            if open != '[' && bytes.get(i + 1) == Some(&(close as u8)) {
                value.push(close);
                i += 2;
                continue;
            }
            return Ok((value, i + 1));
        }
        value.push(c);
        i += 1;
    }
    Err(ParseError {
        message: "unterminated quoted identifier".into(),
        position: start,
    })
}

fn read_param(sql: &str, start: usize, prefix: char) -> (Option<usize>, usize) {
    let bytes = sql.as_bytes();
    let mut i = start + 1;
    // Allow an optional leading letter (e.g. `@P1`).
    if prefix == '@' && i < bytes.len() && (bytes[i] as char).is_ascii_alphabetic() {
        i += 1;
    }
    let digits_start = i;
    while i < bytes.len() && (bytes[i] as char).is_ascii_digit() {
        i += 1;
    }
    let value = sql[digits_start..i].parse::<usize>().ok();
    // Named parameters (e.g. `:name`) consume the identifier but carry no index.
    if i == digits_start {
        while i < bytes.len() && is_ident_part(bytes[i] as char) {
            i += 1;
        }
    }
    (value, i)
}

fn read_number(sql: &str, start: usize) -> (String, usize) {
    let bytes = sql.as_bytes();
    let mut i = start;
    while i < bytes.len() && (bytes[i] as char).is_ascii_digit() {
        i += 1;
    }
    if i < bytes.len() && bytes[i] == b'.' {
        i += 1;
        while i < bytes.len() && (bytes[i] as char).is_ascii_digit() {
            i += 1;
        }
    }
    (sql[start..i].to_string(), i)
}

fn read_word(sql: &str, start: usize) -> (String, usize) {
    let bytes = sql.as_bytes();
    let mut i = start;
    while i < bytes.len() && is_ident_part(bytes[i] as char) {
        i += 1;
    }
    (sql[start..i].to_string(), i)
}

fn is_ident_start(c: char) -> bool {
    c == '_' || c.is_ascii_alphabetic()
}

fn is_ident_part(c: char) -> bool {
    c == '_' || c == '$' || c.is_ascii_alphanumeric()
}

// ---------------------------------------------------------------------------
// Parser
// ---------------------------------------------------------------------------

struct Parser<'a> {
    tokens: Vec<Token>,
    pos: usize,
    _dialect: &'a dyn SqlDialect,
}

impl<'a> Parser<'a> {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos)
    }

    fn peek_at(&self, offset: usize) -> Option<&Token> {
        self.tokens.get(self.pos + offset)
    }

    fn advance(&mut self) -> Option<Token> {
        let token = self.tokens.get(self.pos).cloned();
        if token.is_some() {
            self.pos += 1;
        }
        token
    }

    fn position(&self) -> usize {
        self.peek().map(|t| t.position).unwrap_or(usize::MAX)
    }

    fn err<T>(&self, message: impl Into<String>) -> Result<T, ParseError> {
        Err(ParseError {
            message: message.into(),
            position: self.position(),
        })
    }

    fn skip_semicolons(&mut self) {
        while matches!(self.peek().map(|t| &t.kind), Some(TokKind::Op(";"))) {
            self.pos += 1;
        }
    }

    /// True when the current token is the keyword `kw` (case-insensitive).
    fn is_keyword(&self, kw: &str) -> bool {
        matches!(self.peek().map(|t| &t.kind), Some(TokKind::Word(w)) if w.eq_ignore_ascii_case(kw))
    }

    fn keyword_at(&self, offset: usize, kw: &str) -> bool {
        matches!(self.peek_at(offset).map(|t| &t.kind), Some(TokKind::Word(w)) if w.eq_ignore_ascii_case(kw))
    }

    /// Consume the keyword if present.
    fn eat_keyword(&mut self, kw: &str) -> bool {
        if self.is_keyword(kw) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn expect_keyword(&mut self, kw: &str) -> Result<(), ParseError> {
        if self.eat_keyword(kw) {
            Ok(())
        } else {
            self.err(format!("expected `{kw}`"))
        }
    }

    fn eat(&mut self, kind: &TokKind) -> bool {
        if self.peek().map(|t| &t.kind) == Some(kind) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn expect(&mut self, kind: &TokKind) -> Result<(), ParseError> {
        if self.eat(kind) {
            Ok(())
        } else {
            self.err(format!("expected {}", kind.describe()))
        }
    }

    fn parse_statement(&mut self) -> Result<SelectStatement, ParseError> {
        let with = if self.is_keyword("WITH") {
            self.parse_with()?
        } else {
            Vec::new()
        };
        let mut stmt = self.parse_select_core()?;
        stmt.with = with;
        Ok(stmt)
    }

    fn parse_with(&mut self) -> Result<Vec<Cte>, ParseError> {
        self.expect_keyword("WITH")?;
        let mut ctes = Vec::new();
        loop {
            let name = self.parse_ident("CTE name")?;
            let mut columns = Vec::new();
            if self.eat(&TokKind::LParen) {
                loop {
                    columns.push(self.parse_ident("CTE column")?);
                    if self.eat(&TokKind::Comma) {
                        continue;
                    }
                    break;
                }
                self.expect(&TokKind::RParen)?;
            }
            self.expect_keyword("AS")?;
            self.expect(&TokKind::LParen)?;
            let query = self.parse_statement()?;
            self.expect(&TokKind::RParen)?;
            ctes.push(Cte {
                name,
                columns,
                query: Box::new(query),
            });
            if self.eat(&TokKind::Comma) {
                continue;
            }
            break;
        }
        Ok(ctes)
    }

    fn parse_select_core(&mut self) -> Result<SelectStatement, ParseError> {
        self.expect_keyword("SELECT")?;
        let mut stmt = SelectStatement {
            distinct: self.eat_keyword("DISTINCT"),
            ..SelectStatement::default()
        };
        stmt.projection = self.parse_projection()?;

        if self.eat_keyword("FROM") {
            stmt.from = Some(self.parse_table_expr()?);
            stmt.joins = self.parse_joins()?;
        }

        if self.eat_keyword("WHERE") {
            stmt.filter = Some(self.parse_expr()?);
        }

        if self.eat_keyword("GROUP") {
            self.expect_keyword("BY")?;
            stmt.group_by = self.parse_expr_list()?;
        }

        if self.eat_keyword("HAVING") {
            stmt.having = Some(self.parse_expr()?);
        }

        if self.eat_keyword("ORDER") {
            self.expect_keyword("BY")?;
            stmt.order_by = self.parse_order_by()?;
        }

        if self.eat_keyword("LIMIT") {
            stmt.limit = Some(self.parse_u64()?);
            if self.eat_keyword("OFFSET") {
                stmt.offset = Some(self.parse_u64()?);
            }
        } else if self.eat_keyword("OFFSET") {
            stmt.offset = Some(self.parse_u64()?);
        }

        Ok(stmt)
    }

    fn parse_projection(&mut self) -> Result<Vec<SelectItem>, ParseError> {
        let mut items = Vec::new();
        loop {
            items.push(self.parse_select_item()?);
            if self.eat(&TokKind::Comma) {
                continue;
            }
            break;
        }
        Ok(items)
    }

    fn parse_select_item(&mut self) -> Result<SelectItem, ParseError> {
        // `*`
        if self.eat(&TokKind::Star) {
            return Ok(SelectItem::Wildcard);
        }
        // `alias.*`
        if let (Some(t0), Some(t1), Some(t2)) =
            (self.peek(), self.peek_at(1), self.peek_at(2))
        {
            if matches!(&t0.kind, TokKind::Word(_) | TokKind::Quoted(_))
                && t1.kind == TokKind::Dot
                && t2.kind == TokKind::Star
            {
                let qualifier = self.parse_ident("qualifier")?;
                self.expect(&TokKind::Dot)?;
                self.expect(&TokKind::Star)?;
                return Ok(SelectItem::QualifiedWildcard(qualifier));
            }
        }
        let expr = self.parse_expr()?;
        let alias = self.parse_optional_alias()?;
        Ok(SelectItem::Expr { expr, alias })
    }

    fn parse_optional_alias(&mut self) -> Result<Option<String>, ParseError> {
        if self.eat_keyword("AS") {
            return Ok(Some(self.parse_ident("alias")?));
        }
        if self.peek_is_alias() {
            return Ok(Some(self.parse_ident("alias")?));
        }
        Ok(None)
    }

    /// An alias may follow without `AS` when the next token is a non-reserved
    /// identifier (quoted identifiers always qualify).
    fn peek_is_alias(&self) -> bool {
        match self.peek().map(|t| &t.kind) {
            Some(TokKind::Quoted(_)) => true,
            Some(TokKind::Word(w)) => !is_reserved_boundary(w),
            _ => false,
        }
    }

    fn parse_table_expr(&mut self) -> Result<TableExpr, ParseError> {
        let name = self.parse_object_name()?;
        let alias = self.parse_optional_alias()?;
        Ok(TableExpr { name, alias })
    }

    fn parse_joins(&mut self) -> Result<Vec<Join>, ParseError> {
        let mut joins = Vec::new();
        while let Some(kind) = self.parse_join_kind() {
            let table = self.parse_table_expr()?;
            let on = if kind == JoinKind::Cross {
                None
            } else {
                self.expect_keyword("ON")?;
                Some(self.parse_expr()?)
            };
            joins.push(Join { kind, table, on });
        }
        Ok(joins)
    }

    fn parse_join_kind(&mut self) -> Option<JoinKind> {
        if self.is_keyword("JOIN") {
            self.pos += 1;
            return Some(JoinKind::Inner);
        }
        if self.is_keyword("INNER") && self.keyword_at(1, "JOIN") {
            self.pos += 2;
            return Some(JoinKind::Inner);
        }
        if self.is_keyword("CROSS") && self.keyword_at(1, "JOIN") {
            self.pos += 2;
            return Some(JoinKind::Cross);
        }
        for (word, kind) in [
            ("LEFT", JoinKind::Left),
            ("RIGHT", JoinKind::Right),
            ("FULL", JoinKind::Full),
        ] {
            if self.is_keyword(word) {
                let mut consumed = 1;
                if self.keyword_at(consumed, "OUTER") {
                    consumed += 1;
                }
                if self.keyword_at(consumed, "JOIN") {
                    self.pos += consumed + 1;
                    return Some(kind);
                }
            }
        }
        None
    }

    fn parse_order_by(&mut self) -> Result<Vec<OrderByItem>, ParseError> {
        let mut items = Vec::new();
        loop {
            let expr = self.parse_expr()?;
            let dir = if self.eat_keyword("ASC") {
                Some(SortDir::Asc)
            } else if self.eat_keyword("DESC") {
                Some(SortDir::Desc)
            } else {
                None
            };
            items.push(OrderByItem { expr, dir });
            if self.eat(&TokKind::Comma) {
                continue;
            }
            break;
        }
        Ok(items)
    }

    fn parse_expr_list(&mut self) -> Result<Vec<Expr>, ParseError> {
        let mut exprs = Vec::new();
        loop {
            exprs.push(self.parse_expr()?);
            if self.eat(&TokKind::Comma) {
                continue;
            }
            break;
        }
        Ok(exprs)
    }

    // --- expressions (precedence climbing) ---

    fn parse_expr(&mut self) -> Result<Expr, ParseError> {
        self.parse_or()
    }

    fn parse_or(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.parse_and()?;
        while self.eat_keyword("OR") {
            let right = self.parse_and()?;
            left = binary(left, BinaryOp::Or, right);
        }
        Ok(left)
    }

    fn parse_and(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.parse_not()?;
        while self.eat_keyword("AND") {
            let right = self.parse_not()?;
            left = binary(left, BinaryOp::And, right);
        }
        Ok(left)
    }

    fn parse_not(&mut self) -> Result<Expr, ParseError> {
        if self.eat_keyword("NOT") {
            let expr = self.parse_cmp()?;
            return Ok(Expr::Unary {
                op: UnaryOp::Not,
                expr: Box::new(expr),
            });
        }
        self.parse_cmp()
    }

    fn parse_cmp(&mut self) -> Result<Expr, ParseError> {
        let left = self.parse_add()?;

        // IS [NOT] NULL
        if self.eat_keyword("IS") {
            let negated = self.eat_keyword("NOT");
            self.expect_keyword("NULL")?;
            return Ok(Expr::IsNull {
                expr: Box::new(left),
                negated,
            });
        }

        // [NOT] IN / BETWEEN / LIKE
        let negated = if self.is_keyword("NOT")
            && (self.keyword_at(1, "IN")
                || self.keyword_at(1, "BETWEEN")
                || self.keyword_at(1, "LIKE"))
        {
            self.pos += 1;
            true
        } else {
            false
        };

        if self.eat_keyword("IN") {
            self.expect(&TokKind::LParen)?;
            let list = self.parse_expr_list()?;
            self.expect(&TokKind::RParen)?;
            return Ok(Expr::InList {
                expr: Box::new(left),
                list,
                negated,
            });
        }
        if self.eat_keyword("BETWEEN") {
            let low = self.parse_add()?;
            self.expect_keyword("AND")?;
            let high = self.parse_add()?;
            return Ok(Expr::Between {
                expr: Box::new(left),
                low: Box::new(low),
                high: Box::new(high),
                negated,
            });
        }
        if self.eat_keyword("LIKE") {
            let right = self.parse_add()?;
            let op = if negated {
                BinaryOp::NotLike
            } else {
                BinaryOp::Like
            };
            return Ok(binary(left, op, right));
        }
        if negated {
            return self.err("expected IN, BETWEEN, or LIKE after NOT");
        }

        if let Some(op) = self.peek_compare_op() {
            self.pos += 1;
            let right = self.parse_add()?;
            return Ok(binary(left, op, right));
        }

        Ok(left)
    }

    fn peek_compare_op(&self) -> Option<BinaryOp> {
        match self.peek().map(|t| &t.kind) {
            Some(TokKind::Op("=")) => Some(BinaryOp::Eq),
            Some(TokKind::Op("<>")) => Some(BinaryOp::NotEq),
            Some(TokKind::Op("<")) => Some(BinaryOp::Lt),
            Some(TokKind::Op("<=")) => Some(BinaryOp::LtEq),
            Some(TokKind::Op(">")) => Some(BinaryOp::Gt),
            Some(TokKind::Op(">=")) => Some(BinaryOp::GtEq),
            _ => None,
        }
    }

    fn parse_add(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.parse_term()?;
        loop {
            let op = match self.peek().map(|t| &t.kind) {
                Some(TokKind::Op("+")) => BinaryOp::Plus,
                Some(TokKind::Op("-")) => BinaryOp::Minus,
                Some(TokKind::Op("||")) => BinaryOp::Concat,
                _ => break,
            };
            self.pos += 1;
            let right = self.parse_term()?;
            left = binary(left, op, right);
        }
        Ok(left)
    }

    fn parse_term(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.parse_unary()?;
        loop {
            let op = match self.peek().map(|t| &t.kind) {
                Some(TokKind::Star) => BinaryOp::Mul,
                Some(TokKind::Op("/")) => BinaryOp::Div,
                Some(TokKind::Op("%")) => BinaryOp::Mod,
                _ => break,
            };
            self.pos += 1;
            let right = self.parse_unary()?;
            left = binary(left, op, right);
        }
        Ok(left)
    }

    fn parse_unary(&mut self) -> Result<Expr, ParseError> {
        if matches!(self.peek().map(|t| &t.kind), Some(TokKind::Op("-"))) {
            self.pos += 1;
            let expr = self.parse_unary()?;
            return Ok(Expr::Unary {
                op: UnaryOp::Neg,
                expr: Box::new(expr),
            });
        }
        self.parse_primary()
    }

    fn parse_primary(&mut self) -> Result<Expr, ParseError> {
        let Some(token) = self.peek().cloned() else {
            return self.err("unexpected end of input");
        };
        match &token.kind {
            TokKind::Number(n) => {
                self.pos += 1;
                Ok(Expr::Literal(Literal::Number(n.clone())))
            }
            TokKind::String(s) => {
                self.pos += 1;
                Ok(Expr::Literal(Literal::String(s.clone())))
            }
            TokKind::Param(n) => {
                self.pos += 1;
                Ok(Expr::Param(n.unwrap_or(0)))
            }
            TokKind::LParen => {
                self.pos += 1;
                let inner = self.parse_expr()?;
                self.expect(&TokKind::RParen)?;
                Ok(inner) // fold parentheses
            }
            TokKind::Word(w) if w.eq_ignore_ascii_case("TRUE") => {
                self.pos += 1;
                Ok(Expr::Literal(Literal::Boolean(true)))
            }
            TokKind::Word(w) if w.eq_ignore_ascii_case("FALSE") => {
                self.pos += 1;
                Ok(Expr::Literal(Literal::Boolean(false)))
            }
            TokKind::Word(w) if w.eq_ignore_ascii_case("NULL") => {
                self.pos += 1;
                Ok(Expr::Literal(Literal::Null))
            }
            TokKind::Word(w) if w.eq_ignore_ascii_case("CASE") => self.parse_case(),
            // A bare reserved keyword (FROM, WHERE, ...) is not a valid value
            // expression; it must be quoted to be used as an identifier.
            TokKind::Word(w) if is_reserved_boundary(w) => {
                let message = format!("unexpected keyword `{w}`");
                self.err(message)
            }
            TokKind::Word(_) | TokKind::Quoted(_) => self.parse_name_expr(),
            _ => self.err(format!("unexpected {}", token.kind.describe())),
        }
    }

    fn parse_name_expr(&mut self) -> Result<Expr, ParseError> {
        let first = self.parse_ident("identifier")?;
        // Function call.
        if matches!(self.peek().map(|t| &t.kind), Some(TokKind::LParen)) {
            return self.parse_function(first);
        }
        // Column reference, optionally qualified.
        if matches!(self.peek().map(|t| &t.kind), Some(TokKind::Dot)) {
            self.pos += 1;
            let name = self.parse_ident("column")?;
            return Ok(Expr::Column(ColumnRef {
                qualifier: Some(first),
                name,
            }));
        }
        Ok(Expr::Column(ColumnRef::bare(first)))
    }

    fn parse_function(&mut self, name: String) -> Result<Expr, ParseError> {
        self.expect(&TokKind::LParen)?;
        // `count(*)`
        if self.eat(&TokKind::Star) {
            self.expect(&TokKind::RParen)?;
            return Ok(Expr::Function {
                name,
                distinct: false,
                args: vec![FuncArg::Wildcard],
            });
        }
        let distinct = self.eat_keyword("DISTINCT");
        let mut args = Vec::new();
        if !matches!(self.peek().map(|t| &t.kind), Some(TokKind::RParen)) {
            loop {
                args.push(FuncArg::Expr(self.parse_expr()?));
                if self.eat(&TokKind::Comma) {
                    continue;
                }
                break;
            }
        }
        self.expect(&TokKind::RParen)?;
        Ok(Expr::Function {
            name,
            distinct,
            args,
        })
    }

    fn parse_case(&mut self) -> Result<Expr, ParseError> {
        self.expect_keyword("CASE")?;
        let operand = if !self.is_keyword("WHEN") {
            Some(Box::new(self.parse_expr()?))
        } else {
            None
        };
        let mut whens = Vec::new();
        while self.eat_keyword("WHEN") {
            let when = self.parse_expr()?;
            self.expect_keyword("THEN")?;
            let then = self.parse_expr()?;
            whens.push((when, then));
        }
        if whens.is_empty() {
            return self.err("CASE requires at least one WHEN");
        }
        let else_expr = if self.eat_keyword("ELSE") {
            Some(Box::new(self.parse_expr()?))
        } else {
            None
        };
        self.expect_keyword("END")?;
        Ok(Expr::Case {
            operand,
            whens,
            else_expr,
        })
    }

    // --- leaf helpers ---

    fn parse_object_name(&mut self) -> Result<ObjectName, ParseError> {
        let mut parts = vec![self.parse_ident("object name")?];
        while matches!(self.peek().map(|t| &t.kind), Some(TokKind::Dot)) {
            self.pos += 1;
            parts.push(self.parse_ident("name part")?);
        }
        Ok(ObjectName(parts))
    }

    fn parse_ident(&mut self, what: &str) -> Result<String, ParseError> {
        match self.advance().map(|t| t.kind) {
            Some(TokKind::Word(w)) => Ok(w),
            Some(TokKind::Quoted(w)) => Ok(w),
            _ => {
                self.pos = self.pos.saturating_sub(1);
                self.err(format!("expected {what}"))
            }
        }
    }

    fn parse_u64(&mut self) -> Result<u64, ParseError> {
        match self.peek().map(|t| &t.kind) {
            Some(TokKind::Number(n)) if !n.contains('.') => {
                let value = n.parse::<u64>().map_err(|_| ParseError {
                    message: "invalid integer".into(),
                    position: self.position(),
                })?;
                self.pos += 1;
                Ok(value)
            }
            _ => self.err("expected an integer"),
        }
    }
}

fn binary(left: Expr, op: BinaryOp, right: Expr) -> Expr {
    Expr::Binary {
        left: Box::new(left),
        op,
        right: Box::new(right),
    }
}

/// Words that terminate an implicit alias / table-ref (clause + join keywords).
fn is_reserved_boundary(word: &str) -> bool {
    const BOUNDARY: &[&str] = &[
        "FROM", "WHERE", "GROUP", "ORDER", "BY", "HAVING", "LIMIT", "OFFSET", "JOIN", "INNER",
        "LEFT", "RIGHT", "FULL", "CROSS", "OUTER", "ON", "UNION", "EXCEPT", "INTERSECT", "AS",
        "AND", "OR", "ASC", "DESC", "WHEN", "THEN", "ELSE", "END", "IS", "IN", "LIKE", "BETWEEN",
        "NOT",
    ];
    BOUNDARY.iter().any(|kw| kw.eq_ignore_ascii_case(word))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dialect::PostgresDialect;

    fn round_trip(sql: &str) -> SelectStatement {
        let d = PostgresDialect;
        let ast = parse_select(sql, &d).unwrap_or_else(|e| panic!("parse `{sql}` failed: {e}"));
        let rendered = ast.render(&d);
        let reparsed = parse_select(&rendered, &d)
            .unwrap_or_else(|e| panic!("reparse `{rendered}` failed: {e}"));
        assert_eq!(ast, reparsed, "AST not stable: `{sql}` -> `{rendered}`");
        ast
    }

    #[test]
    fn parses_simple_select() {
        let ast = round_trip("select * from users");
        assert_eq!(ast.projection, vec![SelectItem::Wildcard]);
        assert_eq!(ast.from.unwrap().name.object(), "users");
    }

    #[test]
    fn parses_projection_and_aliases() {
        let ast = round_trip("SELECT id, name AS full_name, count(*) total FROM users u");
        assert_eq!(ast.projection.len(), 3);
        assert_eq!(ast.from.as_ref().unwrap().alias.as_deref(), Some("u"));
    }

    #[test]
    fn parses_joins_with_on() {
        let ast =
            round_trip("SELECT * FROM orders o JOIN customers c ON o.customer_id = c.id");
        assert_eq!(ast.joins.len(), 1);
        assert_eq!(ast.joins[0].kind, JoinKind::Inner);
    }

    #[test]
    fn parses_left_outer_join() {
        let ast = round_trip("SELECT * FROM a LEFT OUTER JOIN b ON a.id = b.a_id");
        assert_eq!(ast.joins[0].kind, JoinKind::Left);
    }

    #[test]
    fn parses_where_with_precedence() {
        round_trip("SELECT * FROM t WHERE a = 1 AND b = 2 OR c = 3");
        round_trip("SELECT * FROM t WHERE (a = 1 OR b = 2) AND c = 3");
        round_trip("SELECT * FROM t WHERE a + b * c > 10");
        round_trip("SELECT * FROM t WHERE NOT a = 1");
    }

    #[test]
    fn parses_in_between_like_isnull() {
        round_trip("SELECT * FROM t WHERE x IN (1, 2, 3)");
        round_trip("SELECT * FROM t WHERE x NOT IN (1, 2)");
        round_trip("SELECT * FROM t WHERE x BETWEEN 1 AND 10");
        round_trip("SELECT * FROM t WHERE name LIKE 'a%'");
        round_trip("SELECT * FROM t WHERE name NOT LIKE 'a%'");
        round_trip("SELECT * FROM t WHERE x IS NULL");
        round_trip("SELECT * FROM t WHERE x IS NOT NULL");
    }

    #[test]
    fn parses_group_having_order_limit() {
        let ast = round_trip(
            "SELECT customer_id, count(*) AS n FROM orders GROUP BY customer_id HAVING count(*) > 5 ORDER BY n DESC LIMIT 10 OFFSET 20",
        );
        assert_eq!(ast.group_by.len(), 1);
        assert_eq!(ast.limit, Some(10));
        assert_eq!(ast.offset, Some(20));
    }

    #[test]
    fn parses_cte() {
        let ast = round_trip(
            "WITH recent (id, total) AS (SELECT id, total FROM orders WHERE total > 100) SELECT * FROM recent",
        );
        assert_eq!(ast.with.len(), 1);
        assert_eq!(ast.with[0].columns, vec!["id", "total"]);
    }

    #[test]
    fn parses_case_and_distinct_aggregate() {
        round_trip(
            "SELECT CASE WHEN total > 100 THEN 'big' ELSE 'small' END AS bucket FROM orders",
        );
        round_trip("SELECT count(DISTINCT customer_id) FROM orders");
    }

    #[test]
    fn quotes_keyword_identifiers_on_render() {
        let d = PostgresDialect;
        let ast = parse_select(r#"SELECT "order" FROM "select""#, &d).unwrap();
        let rendered = ast.render(&d);
        assert!(rendered.contains("\"order\""));
        assert!(rendered.contains("\"select\""));
    }

    #[test]
    fn rejects_unsupported_input() {
        let d = PostgresDialect;
        assert!(parse_select("DELETE FROM t", &d).is_err());
        assert!(parse_select("SELECT FROM", &d).is_err());
        assert!(parse_select("SELECT * FROM t WHERE", &d).is_err());
    }
}
