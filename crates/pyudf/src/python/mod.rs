//! Python bridge module — organizes the Python/DuckDB integration.
//!
//! - `bridge.rs` — CPython lifecycle, expression cache, error conversion
//! - `scalar.rs` — py_eval scalar function
//! - `aggregate.rs` — Python aggregate functions (planned)
//! - `table.rs` — py_scan table function (planned)

pub mod bridge;
pub mod scalar;

#[cfg(test)]
#[path = "bridge_tests.rs"]
mod bridge_tests;
