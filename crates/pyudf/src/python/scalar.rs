//! Python scalar functions — called once per DataChunk (2048 rows).
//!
//! Each call receives the full chunk as a PyArrow Array via zero-copy
//! pointer handoff, runs the user's Python callable, and writes results
//! back to the DuckDB output vector.

use pyo3::prelude::*;
use pyo3::types::PyDict;

use libduckdb_sys as ffi;
use quack_rs::vector::{VectorReader, VectorWriter};
use quack_rs::types::TypeId;

/// `py_eval(expression VARCHAR, input VARCHAR) → VARCHAR`
///
/// Evaluates a Python expression against each input row.
/// The expression is compiled once (lazily) and cached per connection.
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
    // TODO: Implement Python interpreter init + expression evaluation
    // Phase 1 stub: identity passthrough for now
    let reader = unsafe { VectorReader::new(input, 1) }; // arg 1 = input column
    let mut writer = unsafe { VectorWriter::new(output) };
    let row_count = reader.row_count();

    for row in 0..row_count {
        if !unsafe { reader.is_valid(row) } {
            unsafe { writer.set_null(row) };
            continue;
        }
        let s = unsafe { reader.read_str(row) };
        // Stub: passthrough. Will become: call Python expression
        unsafe { writer.write_varchar(row, s) };
    }
}
