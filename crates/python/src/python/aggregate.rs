//! Python aggregate — `py_agg(value, update, finalize) → VARCHAR`.
//!
//! State layout (STATE_SIZE = 8200 bytes):
//!   [0..FMAX):         finalize expression (1024 bytes)
//!   [FMAX..FMAX+4):    unused padding (4 bytes)
//!   [JOFF..PTR):       JSON state backup (7160 bytes)
//!   [PTR..STATE_SIZE): *mut Python dict pointer (8 bytes) — perf optimization

use std::ffi::CString;

use libduckdb_sys as ffi;
use pyo3::prelude::*;
use pyo3::types::PyDict;
use quack_rs::error::ExtensionError;

use super::bridge::ensure_python;
use quack_rs::vector::{VectorReader, VectorWriter};

const FMAX: usize = 1024;
const PTR_SIZE: usize = 8;
const STATE_SIZE: usize = 8192 + PTR_SIZE;
const JOFF: usize = FMAX + 4;
const JMAX: usize = STATE_SIZE - JOFF - 4 - PTR_SIZE;
const PTR_OFFSET: usize = STATE_SIZE - PTR_SIZE;

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

// ── Python dict pointer helpers ─────────────────────────────

unsafe fn read_dict_ptr(p: *const u8) -> *mut std::ffi::c_void {
    let mut val: usize = 0;
    unsafe {
        std::ptr::copy_nonoverlapping(
            p.add(PTR_OFFSET),
            &mut val as *mut usize as *mut u8,
            PTR_SIZE,
        );
    }
    val as *mut std::ffi::c_void
}

unsafe fn write_dict_ptr(p: *mut u8, ptr: *mut std::ffi::c_void) {
    let val: usize = ptr as usize;
    unsafe {
        std::ptr::copy_nonoverlapping(val.to_le_bytes().as_ptr(), p.add(PTR_OFFSET), PTR_SIZE);
    }
}

/// Take ownership of the cached PyDict pointer (consumes the extra ref).
/// Returns None if no cached dict exists.
unsafe fn take_dict<'py>(py: Python<'py>, p: *mut u8) -> Option<Bound<'py, PyDict>> {
    let ptr = unsafe { read_dict_ptr(p) };
    if ptr.is_null() {
        return None;
    }
    unsafe { write_dict_ptr(p, std::ptr::null_mut()) };
    let obj: Py<PyAny> =
        unsafe { Py::from_owned_ptr(py, ptr as *mut pyo3::ffi::PyObject) };
    let dict: Bound<'py, PyDict> = obj.into_bound(py).downcast_into().expect("cached state is not a dict");
    Some(dict)
}

/// Deserialize JSON → dict, cache pointer (adds extra ref), return borrowed dict.
unsafe fn init_dict<'py>(py: Python<'py>, p: *mut u8, json: &str) -> Result<Bound<'py, PyDict>, ExtensionError> {
    let code = format!(
        "__import__('json').loads('{}')",
        json.replace('\\', "\\\\").replace('\'', "\\'")
    );
    let c = CString::new(code.as_bytes()).map_err(|e| x(format!("null byte: {}", e)))?;
    let st = py.eval(&c, None, None).map_err(|e| x(pe(e)))?;
    let dict: Bound<'py, PyDict> = st.downcast_into().map_err(|_| x("state is not a dict"))?;
    let raw = dict.as_ptr() as *mut std::ffi::c_void;
    unsafe {
        pyo3::ffi::Py_INCREF(raw as *mut pyo3::ffi::PyObject);
        write_dict_ptr(p, raw);
    }
    Ok(dict)
}

/// Serialize dict to JSON string for backup.
fn dict_to_json(py: Python<'_>, dict: &Bound<'_, PyDict>) -> Result<String, ExtensionError> {
    let code = format!("__import__('json').dumps({})", dict);
    let c = CString::new(code.as_bytes()).map_err(|e| x(format!("null byte: {}", e)))?;
    py.eval(&c, None, None).and_then(|r| r.extract::<String>()).map_err(|e| x(pe(e)))
}

/// Load JSON string into a PyDict.
fn json_to_dict<'py>(py: Python<'py>, json: &str) -> Result<Bound<'py, PyDict>, ExtensionError> {
    let code = format!(
        "__import__('json').loads('{}')",
        json.replace('\\', "\\\\").replace('\'', "\\'")
    );
    let c = CString::new(code.as_bytes()).map_err(|e| x(format!("null byte: {}", e)))?;
    let st = py.eval(&c, None, None).map_err(|e| x(pe(e)))?;
    st.downcast_into::<PyDict>().map_err(|_| x("JSON is not a dict"))
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
    unsafe { write_dict_ptr(p, std::ptr::null_mut()) };
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
        if unsafe { rs(p, FMAX) }.is_empty() {
            unsafe { ws(p, &fin, FMAX) };
        }
    }

    let _ = ensure_python();
    let _ = Python::with_gil(|py| -> Result<(), ExtensionError> {
        for i in 0..vr.row_count() {
            if !unsafe { vr.is_valid(i) } { continue; }
            let v = unsafe { vr.read_str(i) };
            let c_json = unsafe { rs(p.add(JOFF), JMAX) };

            let dict = unsafe {
                if let Some(d) = take_dict(py, p) {
                    let raw = d.as_ptr() as *mut std::ffi::c_void;
                    pyo3::ffi::Py_INCREF(raw as *mut pyo3::ffi::PyObject);
                    write_dict_ptr(p, raw);
                    d
                } else {
                    init_dict(py, p, &c_json)?
                }
            };

            let l = PyDict::new(py);
            l.set_item("state", &dict).map_err(|e| x(pe(e)))?;
            l.set_item("x", v).map_err(|e| x(pe(e)))?;
            let c = CString::new(upd.as_bytes()).map_err(|e| x(format!("null byte: {}", e)))?;
            py.run(&c, Some(&l), None).map_err(|e| x(format!("update: {}", pe(e))))?;

            let json = dict_to_json(py, &dict)?;
            unsafe { ws(p.add(JOFF), &json, JMAX) };
        }
        Ok(())
    });
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

    let _ = ensure_python();
    let _ = Python::with_gil(|py| -> Result<(), ExtensionError> {
        let dict_a = unsafe { take_dict(py, pa) };
        let dict_b = unsafe { take_dict(py, pb) };

        if let (Some(da), Some(db)) = (dict_a.as_ref(), dict_b.as_ref()) {
            da.call_method1("update", (&**db,)).map_err(|e| x(pe(e)))?;
            let json = dict_to_json(py, da)?;
            unsafe { ws(pa.add(JOFF), &json, JMAX) };
            let raw = da.as_ptr() as *mut std::ffi::c_void;
            unsafe { pyo3::ffi::Py_INCREF(raw as *mut pyo3::ffi::PyObject); }
            unsafe { write_dict_ptr(pa, raw); }
        } else {
            let ja = unsafe { rs(pa.add(JOFF), JMAX) };
            let jb = unsafe { rs(pb.add(JOFF), JMAX) };
            let la = json_to_dict(py, &ja)?;
            let lb = json_to_dict(py, &jb)?;
            la.call_method1("update", (&lb,)).map_err(|e| x(pe(e)))?;
            let json = dict_to_json(py, &la)?;
            unsafe { ws(pa.add(JOFF), &json, JMAX) };
            let raw = la.as_ptr() as *mut std::ffi::c_void;
            unsafe { pyo3::ffi::Py_INCREF(raw as *mut pyo3::ffi::PyObject); }
            unsafe { write_dict_ptr(pa, raw); }
        }
        Ok(())
    });
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

    let _ = ensure_python();
    let result = Python::with_gil(|py| -> Result<String, ExtensionError> {
        let dict = unsafe { take_dict(py, p) };
        if let Some(d) = dict {
            let l = PyDict::new(py);
            l.set_item("state", &d).map_err(|e| x(pe(e)))?;
            let c = CString::new(fin.as_bytes()).map_err(|e| x(format!("null byte: {}", e)))?;
            let r = py.eval(&c, Some(&l), None).map_err(|e| x(format!("finalize: {}", pe(e))))?;
            if r.is_none() { return Err(x("finalize returned None")); }
            return r.str()
                .map(|ps| ps.to_string_lossy().into_owned())
                .or_else(|_| r.call_method0("__str__").and_then(|r| r.extract()))
                .map_err(|e| x(pe(e)));
        }
        let st = json_to_dict(py, &j)?;
        let l = PyDict::new(py);
        l.set_item("state", &st).map_err(|e| x(pe(e)))?;
        let c = CString::new(fin.as_bytes()).map_err(|e| x(format!("null byte: {}", e)))?;
        let r = py.eval(&c, Some(&l), None).map_err(|e| x(format!("finalize: {}", pe(e))))?;
        if r.is_none() { return Err(x("finalize returned None")); }
        r.str()
            .map(|ps| ps.to_string_lossy().into_owned())
            .or_else(|_| r.call_method0("__str__").and_then(|r| r.extract()))
            .map_err(|e| x(pe(e)))
    });

    match result {
        Ok(s) => unsafe { w.write_varchar(0, &s) },
        Err(_) => unsafe { w.set_null(0) },
    }
}

#[no_mangle]
pub unsafe extern "C" fn py_agg_destroy(
    state: *mut ffi::duckdb_aggregate_state,
    _count: u64,
) {
    let p = unsafe { *state as *mut u8 };
    let _ = ensure_python();
    if let Ok(()) = Python::with_gil(|py| {
        let _ = unsafe { take_dict(py, p) };
        Ok::<(), ()>(())
    }) {}
}

// ── Test helpers ───────────────────────────────────────────

#[doc(hidden)]
pub fn __test_upd_json(expr: &str, json: &str, val: &str) -> Result<String, String> {
    let _ = ensure_python();
    Python::with_gil(|py| -> Result<String, String> {
        let st = json_to_dict(py, json).map_err(|e| e.to_string())?;
        let l = PyDict::new(py);
        l.set_item("state", &st).map_err(|e| pe(e))?;
        l.set_item("x", val).map_err(|e| pe(e))?;
        py.run(&CString::new(expr).unwrap(), Some(&l), None)
            .map_err(|e| format!("update: {}", pe(e)))?;
        let state_obj = l.get_item("state").map_err(|e| pe(e))?.ok_or("state disappeared")?;
        let r: String = dict_to_json(py, &state_obj.downcast_into::<PyDict>().unwrap())
            .map_err(|e| e.to_string())?;
        Ok(r)
    })
}
#[doc(hidden)]
pub fn __test_merge_json(a: &str, b: &str) -> Result<String, String> {
    let _ = ensure_python();
    Python::with_gil(|py| -> Result<String, String> {
        let la = json_to_dict(py, a).map_err(|e| e.to_string())?;
        let lb = json_to_dict(py, b).map_err(|e| e.to_string())?;
        la.call_method1("update", (&lb,)).map_err(|e| pe(e))?;
        let r: String = dict_to_json(py, &la).map_err(|e| e.to_string())?;
        Ok(r)
    })
}
#[doc(hidden)]
pub fn __test_fin_json(expr: &str, json: &str) -> Result<String, String> {
    let _ = ensure_python();
    Python::with_gil(|py| -> Result<String, String> {
        let st = json_to_dict(py, json).map_err(|e| e.to_string())?;
        let l = PyDict::new(py);
        l.set_item("state", &st).map_err(|e| pe(e))?;
        let r = py
            .eval(&CString::new(expr).unwrap(), Some(&l), None)
            .map_err(|e| format!("finalize: {}", pe(e)))?;
        if r.is_none() { return Err("finalize returned None".to_string()); }
        r.str()
            .map(|ps| ps.to_string_lossy().into_owned())
            .or_else(|_| r.call_method0("__str__").and_then(|r| r.extract()))
            .map_err(|e| pe(e))
    })
}
