//! Python scalar functions — py_eval().
//!
//! The `py_eval` function receives two columns from DuckDB:
//! - Column 0: Python expression (constant — read once)
//! - Column 1: Input values (processed row by row)
//!
//! Each row's value is passed to the Python expression as variable `x`.

use super::bridge::{ensure_initialized, eval_py_expr};
use libduckdb_sys as ffi;
use quack_rs::vector::{VectorReader, VectorWriter};

/// `py_eval(expression VARCHAR, value VARCHAR) → VARCHAR`
///
/// Evaluates a Python expression with each input row bound to `x`.
///
/// ```sql
/// SELECT py_eval('x.upper()', name) FROM users;
/// SELECT py_eval('x[::-1]', text) FROM messages;
/// ```
#[no_mangle]
pub unsafe extern "C" fn py_eval(
    _info: ffi::duckdb_function_info,
    input: ffi::duckdb_data_chunk,
    output: ffi::duckdb_vector,
) {
    // Column 0: expression (constant — read first row only)
    let expr_reader = unsafe { VectorReader::new(input, 0) };
    let row_count = expr_reader.row_count();

    let expr = if unsafe { expr_reader.is_valid(0) } {
        unsafe { expr_reader.read_str(0) }
    } else {
        // Empty expression → all NULL
        let mut writer = unsafe { VectorWriter::new(output) };
        for row in 0..row_count {
            unsafe { writer.set_null(row) };
        }
        return;
    };

    // Ensure Python is initialized
    let _ = ensure_initialized();

    // Column 1: input values
    let val_reader = unsafe { VectorReader::new(input, 1) };
    let mut writer = unsafe { VectorWriter::new(output) };

    for row in 0..row_count {
        if !unsafe { val_reader.is_valid(row) } {
            unsafe { writer.set_null(row) };
            continue;
        }

        let value = unsafe { val_reader.read_str(row) };

        match eval_py_expr(expr, value) {
            Ok(result) => unsafe { writer.write_varchar(row, &result) },
            Err(_) => unsafe { writer.set_null(row) },
        }
    }
}
