//! Python interpreter bridge — embeds CPython via PyO3.
//!
//! The `pyudf` extension initializes one CPython interpreter per DuckDB
//! connection (via Python subinterpreters, PEP 684), keeping GIL contention
//! minimal and enabling true multi-connection concurrency.

pub mod scalar;
// pub mod aggregate;
// pub mod table;
// pub mod venv;
