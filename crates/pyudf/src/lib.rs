//! pyudf — DuckDB extension for Python User-Defined Functions.
//!
//! Embeds CPython (via PyO3) inside DuckDB's process to enable
//! Python scalar, aggregate, and table functions from any DuckDB client.

use quack_rs::aggregate::AggregateFunctionBuilder;
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
unsafe fn register_functions(
    con: libduckdb_sys::duckdb_connection,
) -> Result<(), ExtensionError> {
    // ── Scalar: py_eval(expr VARCHAR, value VARCHAR) → VARCHAR ──
    unsafe {
        ScalarFunctionBuilder::new("py_eval")
            .param(TypeId::Varchar)
            .param(TypeId::Varchar)
            .returns(TypeId::Varchar)
            .function(python::scalar::py_eval)
            .register(con)?;
    }

    // ── Aggregate: py_agg(value VARCHAR, update VARCHAR, finalize VARCHAR) → VARCHAR ──
    unsafe {
        AggregateFunctionBuilder::new("py_agg")
            .param(TypeId::Varchar) // value
            .param(TypeId::Varchar) // update expression
            .param(TypeId::Varchar) // finalize expression
            .returns(TypeId::Varchar)
            .state_size(python::aggregate::py_agg_state_size)
            .init(python::aggregate::py_agg_init)
            .update(python::aggregate::py_agg_update)
            .combine(python::aggregate::py_agg_combine)
            .finalize(python::aggregate::py_agg_finalize)
            .destructor(python::aggregate::py_agg_destroy)
            .register(con)?;
    }

    Ok(())
}
