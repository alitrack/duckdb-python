//! Python table function — `py_scan(module, func) → TABLE(...)`.
//!
//! v2: auto-detects column types (BigInt, Double, Date, Varchar) and writes
//!     properly typed output vectors instead of all-VARCHAR.

use std::ffi::CString;

use libduckdb_sys as ffi;
use pyo3::prelude::*;
use quack_rs::data_chunk::DataChunk;
use quack_rs::error::ExtensionError;
use quack_rs::table::{BindInfo, FfiBindData, FfiInitData};
use quack_rs::types::TypeId;

use super::bridge::ensure_python;

fn pe(e: PyErr) -> String {
    let _ = ensure_python();
    Python::with_gil(|py| {
        e.traceback(py)
            .and_then(|t| t.format().ok())
            .unwrap_or_else(|| e.to_string())
    })
}

fn x(s: impl Into<String>) -> ExtensionError {
    ExtensionError::new(s.into())
}

fn is_valid_ident(s: &str) -> bool {
    !s.is_empty()
        && s.chars().all(|c| c.is_alphanumeric() || c == '_')
        && s.chars().next().map_or(false, |c| c.is_alphabetic() || c == '_')
}

#[derive(Clone)]
pub struct PyScanState {
    rows: Vec<Vec<String>>,
    col_count: usize,
    col_types: Vec<TypeId>,
    pos: usize,
}

// ── Type inference ──

fn infer_type(values: &[&str]) -> TypeId {
    if values.is_empty() {
        return TypeId::Varchar;
    }
    if values.iter().all(|s| s.parse::<i64>().is_ok()) {
        return TypeId::BigInt;
    }
    if values.iter().all(|s| s.parse::<f64>().is_ok()) {
        return TypeId::Double;
    }
    if values.iter().all(|s| {
        s.len() == 10
            && s.as_bytes().get(4) == Some(&b'-')
            && s.as_bytes().get(7) == Some(&b'-')
            && s[..4].parse::<u16>().is_ok()
    }) {
        return TypeId::Date;
    }
    TypeId::Varchar
}

fn infer_column_types(rows: &[Vec<String>], col_count: usize) -> Vec<TypeId> {
    if rows.is_empty() {
        return vec![TypeId::Varchar; col_count];
    }
    (0..col_count)
        .map(|c| {
            let samples: Vec<&str> = rows
                .iter()
                .filter_map(|r| r.get(c))
                .filter(|s| !s.is_empty())
                .take(100)
                .map(|s| s.as_str())
                .collect();
            infer_type(&samples)
        })
        .collect()
}

// ── Bind / Init ──

#[no_mangle]
pub unsafe extern "C" fn py_scan_bind(info: ffi::duckdb_bind_info) {
    let _ = ensure_python();
    let bind = unsafe { BindInfo::new(info) };
    if bind.parameter_count() < 2 {
        bind.set_error("py_scan requires module and func");
        return;
    }
    let m = unsafe { bind.get_parameter_value(0) }.as_str_or("");
    let f = unsafe { bind.get_parameter_value(1) }.as_str_or("");
    match fetch_py_table(&m, &f) {
        Ok(s) => {
            for i in 0..s.col_count {
                let tid = s.col_types.get(i).copied().unwrap_or(TypeId::Varchar);
                bind.add_result_column(&format!("col{}", i), tid);
            }
            unsafe { FfiBindData::set(info, s) };
        }
        Err(e) => bind.set_error(&format!("py_scan: {}", e)),
    }
}

#[no_mangle]
pub unsafe extern "C" fn py_scan_init(info: ffi::duckdb_init_info) {
    if let Some(s) = unsafe { FfiBindData::<PyScanState>::get_from_init(info) } {
        unsafe { FfiInitData::set(info, s.clone()) };
    }
}

// ── Scan: write typed output vectors ──

#[no_mangle]
pub unsafe extern "C" fn py_scan_scan(
    info: ffi::duckdb_function_info,
    output: ffi::duckdb_data_chunk,
) {
    let s = match unsafe { FfiInitData::<PyScanState>::get_mut(info) } {
        Some(s) => s,
        None => {
            let c = unsafe { DataChunk::from_raw(output) };
            unsafe { c.set_size(0) };
            return;
        }
    };
    let chunk = unsafe { DataChunk::from_raw(output) };
    let mut n = 0usize;
    while n < 2048 && s.pos < s.rows.len() {
        let row = &s.rows[s.pos];
        for c in 0..s.col_count {
            let vec = unsafe { chunk.vector(c) };
            let is_null = c >= row.len() || row[c].is_empty();
            let tid = s.col_types.get(c).copied().unwrap_or(TypeId::Varchar);

            if is_null {
                unsafe { set_null(vec, n) };
                continue;
            }
            let val = &row[c];
            unsafe {
                match tid {
                    TypeId::BigInt => {
                        if let Ok(i) = val.parse::<i64>() {
                            write_bigint(vec, n, i);
                        } else {
                            set_null(vec, n);
                        }
                    }
                    TypeId::Double => {
                        if let Ok(f) = val.parse::<f64>() {
                            write_double(vec, n, f);
                        } else {
                            set_null(vec, n);
                        }
                    }
                    TypeId::Date => {
                        write_date_str(vec, n, val);
                    }
                    _ => {
                        let mut w = chunk.writer(c);
                        w.write_varchar(n, val);
                        continue;
                    }
                }
            }
        }
        s.pos += 1;
        n += 1;
    }
    unsafe { chunk.set_size(n) };
}

// ── Raw DuckDB vector helpers (for typed output) ──

unsafe fn set_null(vec: ffi::duckdb_vector, idx: usize) {
    unsafe {
        ffi::duckdb_vector_ensure_validity_writable(vec);
        let validity = ffi::duckdb_vector_get_validity(vec);
        if !validity.is_null() {
            ffi::duckdb_validity_set_row_invalid(validity, idx as u64);
        }
    }
}

unsafe fn write_bigint(vec: ffi::duckdb_vector, idx: usize, val: i64) {
    unsafe {
        let data = ffi::duckdb_vector_get_data(vec) as *mut i64;
        *data.add(idx) = val;
    }
}

unsafe fn write_double(vec: ffi::duckdb_vector, idx: usize, val: f64) {
    unsafe {
        let data = ffi::duckdb_vector_get_data(vec) as *mut f64;
        *data.add(idx) = val;
    }
}

unsafe fn write_date_str(vec: ffi::duckdb_vector, idx: usize, s: &str) {
    // Parse YYYY-MM-DD → days since 1970-01-01
    if s.len() >= 10 {
        let y: i32 = s[..4].parse().unwrap_or(1970);
        let m: u32 = s[5..7].parse().unwrap_or(1);
        let d: u32 = s[8..10].parse().unwrap_or(1);
        let days = days_since_epoch(y, m, d);
        let data = unsafe { ffi::duckdb_vector_get_data(vec) as *mut i32 };
        unsafe { *data.add(idx) = days };
    }
}

fn days_since_epoch(y: i32, m: u32, d: u32) -> i32 {
    let (y, m) = if m <= 2 { (y as i64 - 1, m as i64 + 12) } else { (y as i64, m as i64) };
    let d = d as i64;
    let era = if y >= 0 { y / 400 } else { (y - 399) / 400 };
    let yoe = y - era * 400;
    let doy = (153 * (m - 3) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    (era * 146097 + doe - 719468) as i32
}

// ── Fetch rows + infer types ──

fn fetch_py_table(module: &str, func: &str) -> Result<PyScanState, ExtensionError> {
    let _ = ensure_python();
    if !is_valid_ident(module) || !is_valid_ident(func) {
        return Err(x(format!(
            "Invalid module or function name: '{}'.'{}'",
            module, func
        )));
    }
    Python::with_gil(|py| {
        let locals = pyo3::types::PyDict::new(py);
        locals.set_item("__m__", module).map_err(|e| x(pe(e)))?;
        locals.set_item("__f__", func).map_err(|e| x(pe(e)))?;
        let code =
            CString::new("getattr(__import__(__m__), __f__)()").map_err(|e| x(format!("Invalid code: {}", e)))?;
        let result = py
            .eval(&code, Some(&locals), None)
            .map_err(|e| x(format!("import {}.{}: {}", module, func, pe(e))))?;
        let rows: Vec<Vec<String>> = if let Ok(l) = result.downcast::<pyo3::types::PyList>() {
            l.iter().map(|v| obj_to_row(py, &v)).collect::<Result<_, _>>()?
        } else if let Ok(t) = result.downcast::<pyo3::types::PyTuple>() {
            t.iter().map(|v| obj_to_row(py, &v)).collect::<Result<_, _>>()?
        } else {
            result.call_method0("__iter__").map_err(|e| x(pe(e)))?;
            let l = py
                .eval(&CString::new(b"list(iter)").unwrap(), None, None)
                .map_err(|e| x(pe(e)))?;
            l.downcast::<pyo3::types::PyList>()
                .map_err(|e| x(pe(e.into())))?
                .iter()
                .map(|v| obj_to_row(py, &v))
                .collect::<Result<_, _>>()?
        };
        let cc = if rows.is_empty() { 1 } else { rows[0].len().max(1) };
        let col_types = infer_column_types(&rows, cc);
        Ok(PyScanState {
            rows,
            col_count: cc,
            col_types,
            pos: 0,
        })
    })
}

// ── py_script(code VARCHAR) → TABLE(...) ──
// Executes arbitrary Python code, then returns rows from a `main()` function
// (preferred) or a `result` variable. Direct mapping target for KNIME
// "Python Script" nodes and arbitrary data-science scripts.

#[no_mangle]
pub unsafe extern "C" fn py_script_bind(info: ffi::duckdb_bind_info) {
    let _ = ensure_python();
    let bind = unsafe { BindInfo::new(info) };
    if bind.parameter_count() < 1 {
        bind.set_error("py_script requires a code string");
        return;
    }
    let code = unsafe { bind.get_parameter_value(0) }.as_str_or("");
    match fetch_py_script(&code) {
        Ok(s) => {
            for i in 0..s.col_count {
                let tid = s.col_types.get(i).copied().unwrap_or(TypeId::Varchar);
                bind.add_result_column(&format!("col{}", i), tid);
            }
            unsafe { FfiBindData::set(info, s) };
        }
        Err(e) => bind.set_error(&format!("py_script: {}", e)),
    }
}

#[no_mangle]
pub unsafe extern "C" fn py_script_init(info: ffi::duckdb_init_info) {
    py_scan_init(info)
}

#[no_mangle]
pub unsafe extern "C" fn py_script_scan(
    info: ffi::duckdb_function_info,
    output: ffi::duckdb_data_chunk,
) {
    py_scan_scan(info, output)
}

/// Execute `code`, then collect rows from a `main()` function (preferred) or a
/// `result` variable. Rows: list/tuple/iterable of rows (row = list/tuple/dict/str).
fn fetch_py_script(code: &str) -> Result<PyScanState, ExtensionError> {
    let _ = ensure_python();
    Python::with_gil(|py| {
        let locals = pyo3::types::PyDict::new(py);
        let code_c = CString::new(code).map_err(|e| x(format!("Invalid code: {}", e)))?;
        py.run(&code_c, Some(&locals), None)
            .map_err(|e| x(format!("exec: {}", pe(e))))?;

        let result = if locals.contains("main").unwrap_or(false) {
            let main = locals
                .get_item("main")
                .map_err(|e| x(pe(e)))?
                .ok_or_else(|| x("main disappeared after check"))?;
            main.call0().map_err(|e| x(format!("main(): {}", pe(e))))?
        } else if let Some(r) = locals.get_item("result").map_err(|e| x(pe(e)))? {
            r
        } else {
            return Err(x("py_script: code must define main() or a result variable"));
        };

        let rows: Vec<Vec<String>> = if let Ok(l) = result.downcast::<pyo3::types::PyList>() {
            l.iter().map(|v| obj_to_row(py, &v)).collect::<Result<_, _>>()?
        } else if let Ok(t) = result.downcast::<pyo3::types::PyTuple>() {
            t.iter().map(|v| obj_to_row(py, &v)).collect::<Result<_, _>>()?
        } else {
            let iter = result.call_method0("__iter__").map_err(|e| x(pe(e)))?;
            let l = py
                .eval(&CString::new(b"list(iter)").unwrap(), None, None)
                .map_err(|e| x(pe(e)))?;
            l.downcast::<pyo3::types::PyList>()
                .map_err(|e| x(pe(e.into())))?
                .iter()
                .map(|v| obj_to_row(py, &v))
                .collect::<Result<_, _>>()?
        };
        let cc = if rows.is_empty() { 1 } else { rows[0].len().max(1) };
        let col_types = infer_column_types(&rows, cc);
        Ok(PyScanState {
            rows,
            col_count: cc,
            col_types,
            pos: 0,
        })
    })
}

fn obj_to_row(py: Python<'_>, o: &Bound<'_, PyAny>) -> Result<Vec<String>, ExtensionError> {
    if let Ok(l) = o.downcast::<pyo3::types::PyList>() {
        return l.iter().map(|v| obj_to_str(py, &v)).collect();
    }
    if let Ok(t) = o.downcast::<pyo3::types::PyTuple>() {
        return t.iter().map(|v| obj_to_str(py, &v)).collect();
    }
    if let Ok(d) = o.downcast::<pyo3::types::PyDict>() {
        return d.values().into_iter().map(|v| obj_to_str(py, &v)).collect();
    }
    Ok(vec![obj_to_str(py, o)?])
}

fn obj_to_str(_py: Python<'_>, o: &Bound<'_, PyAny>) -> Result<String, ExtensionError> {
    if o.is_none() {
        return Ok(String::new());
    }
    if let Ok(s) = o.str() {
        return Ok(s.to_string_lossy().into_owned());
    }
    Ok(o.to_string())
}
