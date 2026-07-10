//! pyudf — DuckDB extension for Python User-Defined Functions.
//!
//! Embeds CPython (via PyO3) inside DuckDB's process to enable
//! `CREATE FUNCTION ... LANGUAGE python` in any DuckDB client.
//!
//! ## Capabilities (planned)
//! - **Scalar functions**: `py_fn(x VARCHAR) → VARCHAR`
//! - **Aggregate functions**: `py_agg(x DOUBLE) → DOUBLE`
//! - **Table functions**: `py_scan(url) → TABLE(...)`
//! - **Virtual envs**: `SET py_venv = '/path/to/.venv'`
//!
//! ## Architecture
//! ```text
//! SQL → quack-rs callback → Arrow DataChunk
//!                          → PyO3 → CPython → numpy/sklearn/...
//!                          → Arrow DataChunk → SQL
//! ```

use quack_rs::entry_point::init_extension;
use quack_rs::error::ExtensionError;
use quack_rs::scalar::ScalarFunctionBuilder;
use quack_rs::types::TypeId;

mod python;

/// Extension entry point — DuckDB calls this symbol at load time.
#[no_mangle]
pub unsafe extern "C" fn pyudf_init_c_api(
    info: libduckdb_sys::duckdb_extension_info,
    access: *const libduckdb_sys::duckdb_extension_access,
) -> bool {
    unsafe {
        init_extension(info, access, quack_rs::DUCKDB_API_VERSION, |con| {
            register_functions(con)
        })
    }
}

/// Register all Python-backed functions with DuckDB.
unsafe fn register_functions(con: libduckdb_sys::duckdb_connection) -> Result<(), ExtensionError> {
    // Scalar: py_eval(expr VARCHAR, ...args) → VARCHAR
    unsafe {
        ScalarFunctionBuilder::new("py_eval")
            .param(TypeId::Varchar)
            .param(TypeId::Varchar)
            .returns(TypeId::Varchar)
            .function(python::scalar::py_eval)
            .register(con)?;
    }

    Ok(())
}
