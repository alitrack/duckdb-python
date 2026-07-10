//! Python function registry.
//!
//! Stores named Python functions registered via `py_register()`
//! and invokes them via `py_call()`.

use std::collections::HashMap;
use std::sync::Mutex;

use once_cell::sync::Lazy;
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

/// List all registered function names (for testing).
#[allow(dead_code)]
pub fn list_names() -> Vec<String> {
    REGISTRY.lock().unwrap().keys().cloned().collect()
}

/// `py_register(name VARCHAR, expr VARCHAR, nargs INTEGER) → VARCHAR`
///
/// Registers a named Python function for later invocation via `py_call()`.
///
/// ```sql
/// SELECT py_register('my_upper', 'x.upper()', 1);
/// SELECT py_call('my_upper', 'hello');  -- 'HELLO'
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

    register(&name, &expr, nargs);

    let mut writer = unsafe { VectorWriter::new(output) };
    for row in 0..row_count {
        unsafe {
            writer.write_varchar(row, "OK");
        }
    }
}

/// `py_call(name VARCHAR, arg VARCHAR) → VARCHAR`
///
/// Invokes a previously registered Python function with one argument.
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
