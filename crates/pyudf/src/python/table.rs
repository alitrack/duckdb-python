//! Python table function — `py_scan(module, func) → TABLE(...)`.

use std::ffi::CString;

use libduckdb_sys as ffi;
use pyo3::prelude::*;
use pyo3::prepare_freethreaded_python;
use quack_rs::error::ExtensionError;
use quack_rs::table::{BindInfo, FfiBindData, FfiInitData};
use quack_rs::types::TypeId;

fn ensure_python() {
    prepare_freethreaded_python();
}

fn pe(e: PyErr) -> String {
    ensure_python();
    Python::with_gil(|py| {
        e.traceback(py)
            .and_then(|t| t.format().ok())
            .unwrap_or_else(|| e.to_string())
    })
}

fn x(s: impl Into<String>) -> ExtensionError {
    ExtensionError::new(s.into())
}

#[derive(Clone)]
pub struct PyScanState {
    rows: Vec<Vec<String>>,
    col_count: usize,
    pos: usize,
}

#[no_mangle]
pub unsafe extern "C" fn py_scan_bind(info: ffi::duckdb_bind_info) {
    ensure_python();
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
                bind.add_result_column(&format!("col{}", i), TypeId::Varchar);
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

#[no_mangle]
pub unsafe extern "C" fn py_scan_scan(
    info: ffi::duckdb_function_info,
    output: ffi::duckdb_data_chunk,
) {
    use quack_rs::data_chunk::DataChunk;
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
            let mut w = unsafe { chunk.writer(c) };
            if c < row.len() {
                unsafe { w.write_varchar(n, &row[c]) };
            } else {
                unsafe { w.set_null(n) };
            }
        }
        s.pos += 1;
        n += 1;
    }
    unsafe { chunk.set_size(n) };
}

fn fetch_py_table(module: &str, func: &str) -> Result<PyScanState, ExtensionError> {
    ensure_python();
    Python::with_gil(|py| {
        let code = format!("__import__('{}').{}", module, func);
        let py_fn = py
            .eval(&CString::new(code.as_bytes()).unwrap(), None, None)
            .map_err(|e| x(format!("import {}.{}: {}", module, func, pe(e))))?;
        let result = py_fn.call0().map_err(|e| x(format!("call: {}", pe(e))))?;
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
        Ok(PyScanState {
            rows,
            col_count: cc,
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
