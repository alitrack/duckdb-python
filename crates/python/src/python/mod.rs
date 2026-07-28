//! Python bridge module — organizes the Python/DuckDB integration.

pub mod aggregate;
pub mod bridge;
pub mod loader;
pub mod map;
pub mod registry;
pub mod scalar;
pub mod table;
pub mod venv;

#[cfg(test)]
#[path = "bridge_tests.rs"]
mod bridge_tests;

#[cfg(test)]
#[path = "aggregate_tests.rs"]
mod aggregate_tests;

#[cfg(test)]
#[path = "registry_tests.rs"]
mod registry_tests;
