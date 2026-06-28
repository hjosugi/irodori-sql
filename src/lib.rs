//! SQL dialect metadata, parsing, grammar, formatting hooks, and schema helpers.

pub mod ast;
pub mod dialect;
pub mod grammar;
pub mod metamodel;
pub mod migration;
pub mod params;
pub mod parser;
pub mod schema;

pub const CRATE_NAME: &str = "irodori-sql";
