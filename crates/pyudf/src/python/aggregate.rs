//! Python aggregate — `py_agg(value, update, finalize) → VARCHAR`.
//!
//! IMPORTANT: libduckdb-sys typedefs (duckdb_function_info, duckdb_aggregate_state,
//! duckdb_data_chunk, duckdb_vector) are ALREADY raw pointer types.

use std::ffi::CString;

use libduckdb_sys as ffi;
use pyo3::prelude::*;
use pyo3::types::PyDict;
use quack_rs::error::ExtensionError;

use super::bridge::ensure_initialized;

fn ensure_python() {
    let _ = ensure_initialized();
}
use quack_rs::vector::{VectorReader, VectorWriter};

const STATE_SIZE: usize = 8192;
const FMAX: usize = 1024;
const JOFF: usize = FMAX + 4;
const JMAX: usize = STATE_SIZE - JOFF - 4;

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

unsafe fn w32(p: *mut u8, v: u32) {
    unsafe { std::ptr::copy_nonoverlapping(v.to_le_bytes().as_ptr(), p, 4); }
}
unsafe fn r32(p: *const u8) -> u32 {
    let mut b = [0u8; 4];
    unsafe { std::ptr::copy_nonoverlapping(p, b.as_mut_ptr(), 4); }
    u32::from_le_bytes(b)
}
unsafe fn rs(p: *const u8, max: usize) -> String {
    let l = unsafe { r32(p) as usize }.min(max);
    if l == 0 { return String::new(); }
    String::from_utf8_lossy(unsafe { std::slice::from_raw_parts(p.add(4), l) }).into_owned()
}
unsafe fn ws(p: *mut u8, s: &str, max: usize) {
    let b = s.as_bytes();
    let l = b.len().min(max);
    unsafe { w32(p, l as u32); }
    unsafe { std::ptr::copy_nonoverlapping(b.as_ptr(), p.add(4), l); }
}

fn py_json_loads(json: &str) -> String {
    format!(
        "__import__('json').loads('{}')",
        json.replace('\\', "\\\\").replace('\'', "\\'")
    )
}

// ── Callbacks ──────────────────────────────────────────────

#[no_mangle]
pub unsafe extern "C" fn py_agg_state_size(_info: ffi::duckdb_function_info) -> u64 {
    STATE_SIZE as u64
}

#[no_mangle]
pub unsafe extern "C" fn py_agg_init(
    _info: ffi::duckdb_function_info,
    state: ffi::duckdb_aggregate_state,
) {
    let p = state as *mut u8;
    unsafe { ws(p, "", FMAX) };
    unsafe { ws(p.add(JOFF), "{}", JMAX) };
}

#[no_mangle]
pub unsafe extern "C" fn py_agg_update(
    _info: ffi::duckdb_function_info,
    input: ffi::duckdb_data_chunk,
    state: *mut ffi::duckdb_aggregate_state,
) {
    let p = unsafe { *state as *mut u8 };
    let vr = unsafe { VectorReader::new(input, 0) };
    let ur = unsafe { VectorReader::new(input, 1) };
    let fr = unsafe { VectorReader::new(input, 2) };
    let upd = if unsafe { ur.is_valid(0) } {
        unsafe { ur.read_str(0) }
    } else {
        return;
    };
    let fin = if unsafe { fr.is_valid(0) } {
        unsafe { fr.read_str(0) }
    } else {
        ""
    };
    if !fin.is_empty() {
        let e = unsafe { rs(p, FMAX) };
        if e.is_empty() {
            unsafe { ws(p, &fin, FMAX) };
        }
    }
    for i in 0..vr.row_count() {
        if !unsafe { vr.is_valid(i) } {
            continue;
        }
        let v = unsafe { vr.read_str(i) };
        let c = unsafe { rs(p.add(JOFF), JMAX) };
        if let Ok(n) = upd_json(&upd, &c, v) {
            unsafe { ws(p.add(JOFF), &n, JMAX) };
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn py_agg_combine(
    _info: ffi::duckdb_function_info,
    a: *mut ffi::duckdb_aggregate_state,
    b: *mut ffi::duckdb_aggregate_state,
    _count: u64,
) {
    let pa = unsafe { *a as *mut u8 };
    let pb = unsafe { *b as *mut u8 };
    let ja = unsafe { rs(pa.add(JOFF), JMAX) };
    let jb = unsafe { rs(pb.add(JOFF), JMAX) };
    if let Ok(m) = merge_json(&ja, &jb) {
        unsafe { ws(pa.add(JOFF), &m, JMAX) };
    }
}

#[no_mangle]
pub unsafe extern "C" fn py_agg_finalize(
    _info: ffi::duckdb_function_info,
    state: *mut ffi::duckdb_aggregate_state,
    output: ffi::duckdb_vector,
    _count: u64,
    _row: u64,
) {
    let p = unsafe { *state as *mut u8 };
    let fin = unsafe { rs(p, FMAX) };
    let j = unsafe { rs(p.add(JOFF), JMAX) };
    let mut w = unsafe { VectorWriter::new(output) };
    match fin_json(&fin, &j) {
        Ok(r) => unsafe { w.write_varchar(0, &r) },
        Err(_) => unsafe { w.set_null(0) },
    }
}

#[no_mangle]
pub unsafe extern "C" fn py_agg_destroy(
    _state: *mut ffi::duckdb_aggregate_state,
    _count: u64,
) {}

// ── Test helpers ───────────────────────────────────────────

#[doc(hidden)]
pub fn __test_upd_json(expr: &str, json: &str, val: &str) -> Result<String, String> {
    upd_json(expr, json, val).map_err(|e| e.to_string())
}
#[doc(hidden)]
pub fn __test_merge_json(a: &str, b: &str) -> Result<String, String> {
    merge_json(a, b).map_err(|e| e.to_string())
}
#[doc(hidden)]
pub fn __test_fin_json(expr: &str, json: &str) -> Result<String, String> {
    fin_json(expr, json).map_err(|e| e.to_string())
}

// ── Python calls ───────────────────────────────────────────

fn upd_json(expr: &str, json: &str, val: &str) -> Result<String, ExtensionError> {
    ensure_python();
    Python::with_gil(|py| {
        let l = PyDict::new(py);
        let st = py
            .eval(&CString::new(py_json_loads(json)).unwrap(), None, None)
            .map_err(|e| x(pe(e)))?;
        l.set_item("state", &st).map_err(|e| x(pe(e)))?;
        l.set_item("x", val).map_err(|e| x(pe(e)))?;

        // run() for statements (assignment), eval() is expressions only
        py.run(&CString::new(expr).unwrap(), Some(&l), None)
            .map_err(|e| x(format!("update: {}", pe(e))))?;

        let state_obj = l
            .get_item("state")
            .map_err(|e| x(pe(e)))?
            .ok_or_else(|| x("state disappeared"))?;
        let r: String = py
            .eval(
                &CString::new(format!("__import__('json').dumps({})", state_obj)).unwrap(),
                None,
                None,
            )
            .and_then(|r| r.extract())
            .map_err(|e| x(pe(e)))?;
        Ok(r)
    })
}

fn merge_json(a: &str, b: &str) -> Result<String, ExtensionError> {
    ensure_python();
    Python::with_gil(|py| {
        let la = py
            .eval(&CString::new(py_json_loads(a)).unwrap(), None, None)
            .map_err(|e| x(pe(e)))?;
        let lb = py
            .eval(&CString::new(py_json_loads(b)).unwrap(), None, None)
            .map_err(|e| x(pe(e)))?;
        la.call_method1("update", (&lb,))
            .map_err(|e| x(pe(e)))?;
        let r: String = py
            .eval(
                &CString::new(format!("__import__('json').dumps({})", la)).unwrap(),
                None,
                None,
            )
            .and_then(|r| r.extract())
            .map_err(|e| x(pe(e)))?;
        Ok(r)
    })
}

fn fin_json(expr: &str, json: &str) -> Result<String, ExtensionError> {
    ensure_python();
    Python::with_gil(|py| {
        let l = PyDict::new(py);
        let st = py
            .eval(&CString::new(py_json_loads(json)).unwrap(), None, None)
            .map_err(|e| x(pe(e)))?;
        l.set_item("state", &st).map_err(|e| x(pe(e)))?;
        let r = py
            .eval(&CString::new(expr).unwrap(), Some(&l), None)
            .map_err(|e| x(format!("finalize: {}", pe(e))))?;
        if r.is_none() {
            return Err(x("finalize returned None"));
        }
        let s: String = r
            .str()
            .map(|ps| ps.to_string_lossy().into_owned())
            .or_else(|_| r.call_method0("__str__").and_then(|r| r.extract()))
            .map_err(|e| x(pe(e)))?;
        Ok(s)
    })
}
