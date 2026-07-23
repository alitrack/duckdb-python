//! pyudf — DuckDB extension for Python User-Defined Functions.
//!
//! Embeds CPython (via PyO3) inside DuckDB's process to enable
//! Python scalar, aggregate, and table functions from any DuckDB client.

use std::sync::atomic::{AtomicPtr, Ordering};

use libduckdb_sys::duckdb_connection;
use quack_rs::aggregate::AggregateFunctionBuilder;
#[cfg(feature = "cpython")]
use quack_rs::config_option::{ConfigOptionBuilder, ConfigOptionScope};
use quack_rs::entry_point::init_extension;
use quack_rs::error::ExtensionError;
use quack_rs::scalar::ScalarFunctionBuilder;
use quack_rs::table::TableFunctionBuilder;
use quack_rs::types::TypeId;

mod python;

/// Wrapper around `duckdb_connection` (`*mut _duckdb_connection`) that is
/// `Send + Sync`. DuckDB extensions are inherently single-threaded; this
/// wrapper is only used to store the connection in a static for dynamic
/// function registration from `py_register`.
struct Conn(duckdb_connection);
unsafe impl Send for Conn {}
unsafe impl Sync for Conn {}

/// Cached connection handle for dynamic function registration.
/// Set during extension init, used by `py_register` to register
/// user-named scalar functions on the fly.
pub(crate) static CON: AtomicPtr<Conn> = AtomicPtr::new(std::ptr::null_mut());

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
    // Cache connection handle for dynamic function registration in py_register
    CON.store(Box::into_raw(Box::new(Conn(con))), Ordering::Relaxed);

    // ── Config option: py_venv ──
    #[cfg(feature = "cpython")]
    unsafe {
        ConfigOptionBuilder::try_new("py_venv")?
            .description("Path to Python virtual environment for UDFs")?
            .option_type(TypeId::Varchar)
            .default_value("")?
            .scope(ConfigOptionScope::Global)
            .register(con)?;
    }

    // ── Scalar: py_activate_venv(path VARCHAR) → VARCHAR ──
    #[cfg(feature = "cpython")]
    unsafe {
        ScalarFunctionBuilder::new("py_activate_venv")
            .param(TypeId::Varchar)
            .returns(TypeId::Varchar)
            .function(python::venv::py_activate_venv)
            .register(con)?;
    }

    // ── Scalar: py_register(name VARCHAR, expr VARCHAR, nargs INTEGER) → VARCHAR ──
    #[cfg(feature = "cpython")]
    unsafe {
        ScalarFunctionBuilder::new("py_register")
            .param(TypeId::Varchar)
            .param(TypeId::Varchar)
            .param(TypeId::Integer)
            .returns(TypeId::Varchar)
            .function(python::registry::py_register)
            .register(con)?;
    }

    // ── Scalar: py_call(name VARCHAR, arg VARCHAR) → VARCHAR ──
    #[cfg(feature = "cpython")]
    unsafe {
        ScalarFunctionBuilder::new("py_call")
            .param(TypeId::Varchar)
            .param(TypeId::Varchar)
            .returns(TypeId::Varchar)
            .function(python::registry::py_call)
            .register(con)?;
    }

    // ── Scalar: py_map(expr VARCHAR, value VARCHAR) → VARCHAR ──
    #[cfg(feature = "cpython")]
    unsafe {
        ScalarFunctionBuilder::new("py_map")
            .param(TypeId::Varchar)
            .param(TypeId::Varchar)
            .returns(TypeId::Varchar)
            .function(python::map::py_map)
            .register(con)?;
    }

    // ── Scalar: py_eval(expr VARCHAR, value VARCHAR) → VARCHAR ──
    #[cfg(feature = "cpython")]
    unsafe {
        ScalarFunctionBuilder::new("py_eval")
            .param(TypeId::Varchar)
            .param(TypeId::Varchar)
            .returns(TypeId::Varchar)
            .function(python::scalar::py_eval)
            .register(con)?;
    }

    // ── Aggregate: py_agg(value VARCHAR, update VARCHAR, finalize VARCHAR) → VARCHAR ──
    #[cfg(feature = "cpython")]
    unsafe {
        AggregateFunctionBuilder::new("py_agg")
            .param(TypeId::Varchar)
            .param(TypeId::Varchar)
            .param(TypeId::Varchar)
            .returns(TypeId::Varchar)
            .state_size(python::aggregate::py_agg_state_size)
            .init(python::aggregate::py_agg_init)
            .update(python::aggregate::py_agg_update)
            .combine(python::aggregate::py_agg_combine)
            .finalize(python::aggregate::py_agg_finalize)
            .destructor(python::aggregate::py_agg_destroy)
            .register(con)?;
    }

    // ── Table: py_scan(module VARCHAR, func VARCHAR) → TABLE(...) ──
    #[cfg(feature = "cpython")]
    unsafe {
        TableFunctionBuilder::new("py_scan")
            .param(TypeId::Varchar)
            .param(TypeId::Varchar)
            .bind(python::table::py_scan_bind)
            .init(python::table::py_scan_init)
            .scan(python::table::py_scan_scan)
            .register(con)?;
    }

    // ── Config option: py_subinterpreter ──
    #[cfg(feature = "cpython")]
    unsafe {
        ConfigOptionBuilder::try_new("py_subinterpreter")?
            .description("Enable PEP 684 subinterpreter isolation (Python 3.12+)")?
            .option_type(TypeId::Boolean)
            .default_value("false")?
            .scope(ConfigOptionScope::Global)
            .register(con)?;
    }

    // ── Check PYUD_VENV env var on load ──
    #[cfg(feature = "cpython")]
    python::venv::check_env_venv();

    Ok(())
}
