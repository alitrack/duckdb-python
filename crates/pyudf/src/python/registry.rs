//! Python function registry.
//!
//! Stores named Python functions registered via `py_register()`
//! and invokes them via `py_call()` or directly by name (after `py_register`
//! dynamically registers a DuckDB scalar function).

use std::collections::HashMap;
use std::ffi::{c_char, c_void, CStr, CString};
use std::sync::atomic::Ordering;
use std::sync::Mutex;

use once_cell::sync::Lazy;
use quack_rs::scalar::{ScalarFunctionBuilder, ScalarFunctionInfo};
use quack_rs::types::TypeId;
use quack_rs::vector::{VectorReader, VectorWriter};

use super::bridge::eval_py_expr;

/// Global registry of named Python functions.
/// Maps name → (expression, arg_count).
static REGISTRY: Lazy<Mutex<HashMap<String, (String, usize)>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

/// Register a named Python function.
///
/// The expression uses `x` as the input variable.
pub fn register(name: &str, expr: &str, nargs: usize) {
    REGISTRY
        .lock()
        .unwrap()
        .insert(name.to_string(), (expr.to_string(), nargs));
}

/// Look up a registered function's expression.
pub fn lookup(name: &str) -> Option<String> {
    REGISTRY
        .lock()
        .unwrap()
        .get(name)
        .map(|(expr, _)| expr.clone())
}

/// List all registered function names.
pub fn list_names() -> Vec<String> {
    REGISTRY.lock().unwrap().keys().cloned().collect()
}

// ── Dynamic dispatch: one callback for all user-registered functions ──

/// Destroy callback for extra_info: frees the CString holding the function name.
unsafe extern "C" fn free_string(ptr: *mut c_void) {
    if !ptr.is_null() {
        drop(CString::from_raw(ptr as *mut c_char));
    }
}

/// Generic dispatch callback for dynamically registered Python UDFs.
///
/// Reads the function name from `extra_info`, looks up the Python body
/// in REGISTRY, and evaluates it against the input arguments.
///
/// This callback is shared by ALL functions registered via `py_register`.
/// DuckDB routes calls to `double(5)`, `my_upper('hello')`, etc. all here.
unsafe extern "C" fn py_dispatch(
    info: libduckdb_sys::duckdb_function_info,
    input: libduckdb_sys::duckdb_data_chunk,
    output: libduckdb_sys::duckdb_vector,
) {
    // 1. Get function name from extra_info
    let func_info = unsafe { ScalarFunctionInfo::new(info) };
    let name_ptr = unsafe { func_info.get_extra_info() } as *const c_char;
    let name = if name_ptr.is_null() {
        // Should never happen — py_register always sets extra_info
        fill_null(output, 0);
        return;
    } else {
        match unsafe { CStr::from_ptr(name_ptr) }.to_str() {
            Ok(n) => n.to_string(),
            Err(_) => {
                fill_null(output, 0);
                return;
            }
        }
    };

    // 2. Look up function body
    let body = match REGISTRY.lock().unwrap().get(&name) {
        Some((expr, _)) => expr.clone(),
        None => {
            // Function was unregistered somehow — return nulls
            fill_null(output, 0);
            return;
        }
    };

    // 3. Read first input column as `x` argument
    let arg_reader = unsafe { VectorReader::new(input, 0) };
    let row_count = arg_reader.row_count();

    let mut writer = unsafe { VectorWriter::new(output) };
    for row in 0..row_count {
        if unsafe { !arg_reader.is_valid(row) } {
            unsafe { writer.set_null(row); }
            continue;
        }
        let arg = unsafe { arg_reader.read_str(row) };
        match eval_py_expr(&body, &arg) {
            Ok(result) => unsafe { writer.write_varchar(row, &result) },
            Err(_) => unsafe { writer.set_null(row) },
        }
    }
}

// ── py_register: stores in REGISTRY + registers native DuckDB function ──

/// `py_register(name VARCHAR, expr VARCHAR, nargs INTEGER) → VARCHAR`
///
/// Registers a named Python function AND creates a corresponding DuckDB
/// scalar function so it can be called directly by name.
///
/// ```sql
/// SELECT py_register('double', 'str(int(x)*2)', 1);
/// SELECT double('5');  -- '10'  (direct call, no py_call needed!)
/// SELECT py_call('double', '5');  -- also still works
/// ```
#[no_mangle]
pub unsafe extern "C" fn py_register(
    _info: libduckdb_sys::duckdb_function_info,
    input: libduckdb_sys::duckdb_data_chunk,
    output: libduckdb_sys::duckdb_vector,
) {
    let name_reader = unsafe { VectorReader::new(input, 0) };
    let expr_reader = unsafe { VectorReader::new(input, 1) };
    let nargs_reader = unsafe { VectorReader::new(input, 2) };
    let row_count = name_reader.row_count();

    let name = if unsafe { name_reader.is_valid(0) } {
        unsafe { name_reader.read_str(0) }
    } else {
        fill_null(output, row_count);
        return;
    };

    let expr = if unsafe { expr_reader.is_valid(0) } {
        unsafe { expr_reader.read_str(0) }
    } else {
        fill_null(output, row_count);
        return;
    };

    let nargs = if unsafe { nargs_reader.is_valid(0) } {
        (unsafe { nargs_reader.read_i32(0) }) as usize
    } else {
        1
    };

    // Store in REGISTRY (existing logic)
    register(&name, &expr, nargs);

    // 🔑 Try fresh connection from database handle
    let db = crate::DB.load(Ordering::Acquire);
    if db.is_null() {
        let mut writer = unsafe { VectorWriter::new(output) };
        for row in 0..row_count { unsafe { writer.write_varchar(row, "NODB"); } }
        return;
    }
    let mut con: libduckdb_sys::duckdb_connection = std::ptr::null_mut();
    let rc = unsafe { libduckdb_sys::duckdb_connect(db, &raw mut con) };
    if rc != libduckdb_sys::DuckDBSuccess {
        let mut writer = unsafe { VectorWriter::new(output) };
        let msg = format!("CONNECT_ERR {}", rc as i32);
        for row in 0..row_count { unsafe { writer.write_varchar(row, &msg); } }
        return;
    }
    // Register on fresh connection
    let name_c = CString::new(&name[..]).unwrap();
    let name_ptr = name_c.into_raw() as *mut c_void;
    let result = unsafe {
        ScalarFunctionBuilder::new(&name)
            .param(TypeId::Varchar)
            .returns(TypeId::Varchar)
            .function(py_dispatch)
            .extra_info(name_ptr, Some(free_string))
            .register(con)
    };
    if result.is_err() {
        drop(unsafe { CString::from_raw(name_ptr as *mut c_char) });
    }
    libduckdb_sys::duckdb_disconnect(&raw mut con);

    let mut writer = unsafe { VectorWriter::new(output) };
    for row in 0..row_count {
        unsafe {
            writer.write_varchar(row, "OK");
        }
    }
}

// ── py_call: backward-compatible explicit invocation ──

/// `py_call(name VARCHAR, arg VARCHAR) → VARCHAR`
///
/// Invokes a previously registered Python function with one argument.
/// Also works for functions registered with the old API.
///
/// ```sql
/// SELECT py_register('double', 'str(int(x)*2)', 1);
/// SELECT py_call('double', '5');  -- '10'
/// ```
#[no_mangle]
pub unsafe extern "C" fn py_call(
    _info: libduckdb_sys::duckdb_function_info,
    input: libduckdb_sys::duckdb_data_chunk,
    output: libduckdb_sys::duckdb_vector,
) {
    let name_reader = unsafe { VectorReader::new(input, 0) };
    let arg_reader = unsafe { VectorReader::new(input, 1) };
    let row_count = name_reader.row_count();

    let expr = if unsafe { name_reader.is_valid(0) } {
        let name = unsafe { name_reader.read_str(0) };
        match lookup(&name) {
            Some(e) => e,
            None => {
                let mut writer = unsafe { VectorWriter::new(output) };
                for row in 0..row_count {
                    unsafe {
                        writer.write_varchar(
                            row,
                            &format!("ERROR: function '{}' not registered", name),
                        );
                    }
                }
                return;
            }
        }
    } else {
        fill_null(output, row_count);
        return;
    };

    let mut writer = unsafe { VectorWriter::new(output) };
    for row in 0..row_count {
        if unsafe { !arg_reader.is_valid(row) } {
            unsafe {
                writer.set_null(row);
            }
            continue;
        }

        let arg = unsafe { arg_reader.read_str(row) };
        match eval_py_expr(&expr, arg) {
            Ok(result) => unsafe {
                writer.write_varchar(row, &result);
            },
            Err(_) => unsafe {
                writer.set_null(row);
            },
        }
    }
}

fn fill_null(output: libduckdb_sys::duckdb_vector, count: usize) {
    let mut writer = unsafe { VectorWriter::new(output) };
    for row in 0..count {
        unsafe {
            writer.set_null(row);
        }
    }
}

/// `py_list_functions() → VARCHAR`
///
/// Returns a comma-separated list of all registered Python function names.
///
/// ```sql
/// SELECT py_list_functions();  -- 'double_it,my_upper,...'
/// ```
#[no_mangle]
pub unsafe extern "C" fn py_list_functions(
    _info: libduckdb_sys::duckdb_function_info,
    _input: libduckdb_sys::duckdb_data_chunk,
    output: libduckdb_sys::duckdb_vector,
) {
    let names = list_names();
    let result = names.join(",");
    let mut writer = unsafe { VectorWriter::new(output) };
    unsafe { writer.write_varchar(0, &result) };
}
