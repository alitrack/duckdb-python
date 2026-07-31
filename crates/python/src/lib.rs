//! python_ext — DuckDB extension for Python User-Defined Functions.
//!
//! Embeds CPython (via PyO3) inside DuckDB's process to enable
//! Python scalar, aggregate, and table functions from any DuckDB client.

use std::sync::atomic::{AtomicPtr, Ordering};
use quack_rs::aggregate::AggregateFunctionBuilder;
#[cfg(feature = "cpython")]
use quack_rs::config_option::{ConfigOptionBuilder, ConfigOptionScope};
use quack_rs::entry_point::init_extension;
use quack_rs::error::ExtensionError;
use quack_rs::scalar::ScalarFunctionBuilder;
use quack_rs::table::TableFunctionBuilder;
use quack_rs::types::TypeId;

mod python;

/// Cached database handle for opening fresh connections during `py_register`.
/// Using a new connection avoids the lifecycle issue where the init_extension
/// connection is invalidated after the callback returns.
static DB: AtomicPtr<libduckdb_sys::_duckdb_database> = AtomicPtr::new(std::ptr::null_mut());

/// Long-lived connection opened during extension init.
/// We open this connection while the DuckDB C API is freshly initialized
/// (inside the `init_extension` callback), so it stays valid for the lifetime
/// of the extension. `py_register` uses this instead of calling `duckdb_connect`
/// at runtime, which crashes due to static-linking conflicts with the host's libduckdb.
static LONG_LIVED_CON: AtomicPtr<libduckdb_sys::_duckdb_connection> =
    AtomicPtr::new(std::ptr::null_mut());
/// Extension entry point — DuckDB calls this symbol at load time.
#[no_mangle]
pub unsafe extern "C" fn python_udf_init_c_api(
    info: libduckdb_sys::duckdb_extension_info,
    access: *const libduckdb_sys::duckdb_extension_access,
) -> bool {
    unsafe {
        // Store database handle for fresh connection in py_register.
        if let Some(get_db) = (*access).get_database {
            let db = get_db(info);
            DB.store(*db, Ordering::Release);
        }

        init_extension(info, access, quack_rs::DUCKDB_API_VERSION, |con| {
            register_functions(con)
        })
    }
}

/// Register all Python-backed functions with DuckDB.
unsafe fn register_functions(con: libduckdb_sys::duckdb_connection) -> Result<(), ExtensionError> {
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

    // ── Scalar: py_register(name VARCHAR, expr VARCHAR, nargs INTEGER [, rettype VARCHAR]) → VARCHAR ──
    #[cfg(feature = "cpython")]
    unsafe {
        ScalarFunctionBuilder::new("py_register")
            .param(TypeId::Varchar)
            .param(TypeId::Varchar)
            .param(TypeId::Integer)
            .param(TypeId::Varchar)  // optional return type hint
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

    // ── Scalar: py_list_functions() → VARCHAR ──
    #[cfg(feature = "cpython")]
    unsafe {
        ScalarFunctionBuilder::new("py_list_functions")
            .returns(TypeId::Varchar)
            .function(python::registry::py_list_functions)
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

    // ── Table: py_script(code VARCHAR) → TABLE(...) ──
    // Executes Python code; rows come from main() / result. Maps KNIME "Python Script" nodes.
    #[cfg(feature = "cpython")]
    unsafe {
        TableFunctionBuilder::new("py_script")
            .param(TypeId::Varchar)
            .bind(python::table::py_script_bind)
            .init(python::table::py_script_init)
            .scan(python::table::py_script_scan)
            .register(con)?;
    }

    // ── Check PYUD_VENV env var on load ──
    #[cfg(feature = "cpython")]
    python::venv::check_env_venv();

    // Open a long-lived connection for dynamic function registration.
    // This MUST be done inside the init_extension callback while the C API
    // is freshly initialized. Storing this connection avoids the need to call
    // duckdb_connect at runtime, which crashes due to static-linking conflicts.
    let db = DB.load(Ordering::Acquire);
    if !db.is_null() {
        let mut con: libduckdb_sys::duckdb_connection = std::ptr::null_mut();
        let rc = unsafe { libduckdb_sys::duckdb_connect(db, &raw mut con) };
        if rc == libduckdb_sys::DuckDBSuccess {
            LONG_LIVED_CON.store(con, Ordering::Release);
        }
    }

    Ok(())
}
