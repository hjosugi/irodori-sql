//! GEN-002 — GBNF grammar emission for grammar-constrained SQL generation.
//!
//! Emits a [GBNF](https://github.com/ggml-org/llama.cpp/blob/master/grammars/README.md)
//! grammar describing the `SELECT` subset that [`crate::ast`] can represent. When
//! a [`GrammarSchema`] is supplied, table and column names become *closed*
//! alternations of the real identifiers, so a grammar-constrained decoder can
//! only ever emit names that exist — hallucinated relations become unsamplable.
//!
//! The grammar guarantees **syntax + identifier existence**. The stricter
//! semantic check (a column belongs to a table actually in `FROM`) is left to the
//! AST-level validation step in `irodori-generate`, which keeps this grammar
//! small enough for a tiny model to decode quickly.

use std::collections::BTreeSet;
use std::fmt::Write as _;

/// A table the grammar should allow, with its columns.
#[derive(Debug, Clone)]
pub struct GrammarTable {
    pub name: String,
    pub columns: Vec<String>,
}

/// The schema the grammar is specialized to.
#[derive(Debug, Clone, Default)]
pub struct GrammarSchema {
    pub tables: Vec<GrammarTable>,
}

impl GrammarSchema {
    pub fn new(tables: Vec<GrammarTable>) -> Self {
        Self { tables }
    }

    /// Bare table names that can be expressed as closed GBNF terminals.
    fn terminal_tables(&self) -> Vec<String> {
        let mut names: BTreeSet<String> = BTreeSet::new();
        for table in &self.tables {
            if is_grammar_safe(&table.name) {
                names.insert(table.name.clone());
            }
        }
        names.into_iter().collect()
    }

    /// Union of bare column names across all tables, as closed terminals.
    fn terminal_columns(&self) -> Vec<String> {
        let mut names: BTreeSet<String> = BTreeSet::new();
        for table in &self.tables {
            for column in &table.columns {
                if is_grammar_safe(column) {
                    names.insert(column.clone());
                }
            }
        }
        names.into_iter().collect()
    }
}

/// The fixed body of the SELECT grammar (everything except the identifier rules).
const SELECT_BODY: &str = r#"
select ::= "SELECT " distinct? projection " FROM " table-ref join* where? group? having? order? limit?
distinct ::= "DISTINCT "
projection ::= "*" | proj-item (", " proj-item)*
proj-item ::= expr (" AS " ident)?
table-ref ::= table-name (" " ident)?
join ::= join-kw table-name (" " ident)? " ON " expr
join-kw ::= " JOIN " | " INNER JOIN " | " LEFT JOIN " | " RIGHT JOIN "
where ::= " WHERE " expr
group ::= " GROUP BY " expr (", " expr)*
having ::= " HAVING " expr
order ::= " ORDER BY " order-item (", " order-item)*
order-item ::= expr order-dir?
order-dir ::= " ASC" | " DESC"
limit ::= " LIMIT " int offset?
offset ::= " OFFSET " int
expr ::= or-expr
or-expr ::= and-expr (" OR " and-expr)*
and-expr ::= not-expr (" AND " not-expr)*
not-expr ::= "NOT " cmp-expr | cmp-expr
cmp-expr ::= add-expr (cmp-rhs | in-rhs | between-rhs | null-rhs)?
cmp-rhs ::= cmp-op add-expr
cmp-op ::= " = " | " <> " | " < " | " <= " | " > " | " >= " | " LIKE "
in-rhs ::= " IN (" expr (", " expr)* ")"
between-rhs ::= " BETWEEN " add-expr " AND " add-expr
null-rhs ::= " IS NULL" | " IS NOT NULL"
add-expr ::= term (add-op term)*
add-op ::= " + " | " - "
term ::= primary (mul-op primary)*
mul-op ::= " * " | " / "
primary ::= func | column | literal | "(" expr ")"
func ::= func-name "(" func-args ")"
func-name ::= "COUNT" | "SUM" | "AVG" | "MIN" | "MAX" | "COALESCE"
func-args ::= "*" | func-arg (", " func-arg)*
func-arg ::= "DISTINCT " expr | expr
column ::= qualifier? column-name
qualifier ::= ident "."
literal ::= number | string | "TRUE" | "FALSE" | "NULL"
number ::= "-"? [0-9]+ ("." [0-9]+)?
int ::= [0-9]+
string ::= "'" char* "'"
char ::= [^'\\] | "\\" .
ident ::= [a-zA-Z_] [a-zA-Z0-9_]*
"#;

/// Render a GBNF grammar for the `SELECT` family. When `schema` is `Some`, table
/// and column names are constrained to the schema's identifiers; otherwise they
/// fall back to a generic identifier rule.
pub fn select_grammar(schema: Option<&GrammarSchema>) -> String {
    let mut out = String::new();
    out.push_str("root ::= select\n");
    out.push_str(SELECT_BODY.trim_start());
    out.push('\n');

    let tables = schema
        .map(GrammarSchema::terminal_tables)
        .unwrap_or_default();
    let columns = schema
        .map(GrammarSchema::terminal_columns)
        .unwrap_or_default();

    if tables.is_empty() {
        out.push_str("table-name ::= ident\n");
    } else {
        out.push_str("table-name ::= ");
        push_alternation(&mut out, &tables);
        out.push('\n');
    }

    if columns.is_empty() {
        out.push_str("column-name ::= ident\n");
    } else {
        out.push_str("column-name ::= ");
        push_alternation(&mut out, &columns);
        out.push('\n');
    }

    out
}

fn push_alternation(out: &mut String, values: &[String]) {
    for (i, value) in values.iter().enumerate() {
        if i > 0 {
            out.push_str(" | ");
        }
        let _ = write!(out, "\"{}\"", escape_gbnf_string(value));
    }
}

/// A name is "grammar safe" as a closed terminal when it is a bare identifier we
/// can emit unquoted (the common case). Anything needing quoting is left to the
/// generic `ident` fallback + AST validation rather than complicating the grammar.
fn is_grammar_safe(name: &str) -> bool {
    crate::dialect::is_bare_identifier(name)
}

fn escape_gbnf_string(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_schema() -> GrammarSchema {
        GrammarSchema::new(vec![
            GrammarTable {
                name: "customers".into(),
                columns: vec!["id".into(), "name".into()],
            },
            GrammarTable {
                name: "orders".into(),
                columns: vec!["id".into(), "customer_id".into(), "total".into()],
            },
        ])
    }

    #[test]
    fn schemaless_grammar_uses_generic_identifiers() {
        let g = select_grammar(None);
        assert!(g.contains("root ::= select"));
        assert!(g.contains("table-name ::= ident"));
        assert!(g.contains("column-name ::= ident"));
    }

    #[test]
    fn schema_grammar_closes_table_and_column_terminals() {
        let g = select_grammar(Some(&sample_schema()));
        assert!(g.contains(r#"table-name ::= "customers" | "orders""#));
        // Columns are the de-duplicated, sorted union across tables.
        assert!(g.contains(r#"column-name ::= "customer_id" | "id" | "name" | "total""#));
        // Core structural rules are always present.
        assert!(g.contains("join ::="));
        assert!(g.contains("func-name ::= \"COUNT\""));
    }

    #[test]
    fn unsafe_identifiers_are_skipped_from_terminals() {
        let schema = GrammarSchema::new(vec![GrammarTable {
            name: "weird name".into(),
            columns: vec!["ok_col".into(), "1bad".into()],
        }]);
        let g = select_grammar(Some(&schema));
        // No grammar-safe tables -> generic fallback.
        assert!(g.contains("table-name ::= ident"));
        // Only the safe column survives as a terminal.
        assert!(g.contains(r#"column-name ::= "ok_col""#));
        assert!(!g.contains("1bad"));
    }
}
