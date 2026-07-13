//! Python interpreter bridge — CPython lifecycle management.
//!
//! Wraps PyO3 for safe calls from DuckDB extension callbacks.

use std::ffi::CString;

use pyo3::prelude::*;
use pyo3::prepare_freethreaded_python;
use pyo3::types::PyDict;
use quack_rs::error::ExtensionError;

use super::loader;

/// Ensure Python is initialized (idempotent).
/// First loads libpython dynamically (RTLD_GLOBAL) so that pyo3's
/// unresolved symbols are resolvable, then initializes the interpreter.
pub fn ensure_initialized() -> Result<(), ExtensionError> {
    // dlopen libpython before calling any PyO3 function — the
    // extension-module feature means libpython is NOT linked at build
    // time, so we must provide the symbols at runtime.
    loader::ensure_libpython_loaded().map_err(|e| {
        ExtensionError::new(format!("Failed to load libpython: {}", e))
    })?;
    prepare_freethreaded_python();
    Python::with_gil(|_py| Ok(())).map_err(|e: PyErr| pyerr_to_ext(e))
}

/// Convert PyErr → ExtensionError with traceback.
pub fn pyerr_to_ext(err: PyErr) -> ExtensionError {
    let _ = ensure_initialized();
    let msg = Python::with_gil(|py| {
        let tb = err
            .traceback(py)
            .and_then(|tb| tb.format().ok())
            .unwrap_or_else(|| err.to_string());
        format!("Python error: {}", tb)
    });
    ExtensionError::new(msg)
}

/// Evaluate `expr` with variable `x` bound to `value`.
/// Returns the string representation of the result.
pub fn eval_py_expr(expr: &str, x: &str) -> Result<String, ExtensionError> {
    ensure_initialized()?;
    Python::with_gil(|py| {
        let locals = PyDict::new(py);
        locals.set_item("x", x).map_err(pyerr_to_ext)?;

        let code = CString::new(expr).map_err(|e| {
            ExtensionError::new(format!("Invalid expression (null byte): {}", e))
        })?;

        let result: Bound<'_, PyAny> = py.eval(&code, Some(&locals), None).map_err(|e| {
            ExtensionError::new(format!(
                "py_eval failed for expr '{}' with x='{}': {}",
                expr,
                x,
                pyerr_to_ext_msg(e, py)
            ))
        })?;

        if result.is_none() {
            return Err(ExtensionError::new(
                "Python expression returned None",
            ));
        }

        // Convert result to string
        let s: String = result
            .str()
            .map(|py_str| py_str.to_string_lossy().into_owned())
            .or_else(|_| {
                result
                    .call_method0("__str__")
                    .and_then(|s| s.extract::<String>())
            })
            .map_err(|_| {
                ExtensionError::new("Failed to convert Python result to string")
            })?;

        Ok(s)
    })
}

/// Extract just the error message.
fn pyerr_to_ext_msg(err: PyErr, py: Python<'_>) -> String {
    err.traceback(py)
        .and_then(|tb| tb.format().ok())
        .unwrap_or_else(|| err.to_string())
}
