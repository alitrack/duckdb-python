//! Python virtual environment support.
//!
//! Provides `py_activate_venv(path)` to activate a Python venv by adding
//! its site-packages to sys.path. Also checks PYUD_VENV env var at load.

use std::ffi::CString;
use std::sync::Mutex;

use once_cell::sync::Lazy;
use pyo3::prelude::*;
use quack_rs::error::ExtensionError;
use quack_rs::vector::{VectorReader, VectorWriter};

static VENV_PATH: Lazy<Mutex<Option<String>>> = Lazy::new(|| Mutex::new(None));

/// Activate a Python virtual environment by adding its site-packages to sys.path.
pub fn activate_venv(path: &str) -> Result<(), ExtensionError> {
    Python::with_gil(|py| {
        let sys = py.import("sys").map_err(|e| {
            ExtensionError::new(format!("Failed to import sys: {}", e))
        })?;
        let sys_path = sys.getattr("path").map_err(|e| {
            ExtensionError::new(format!("Failed to get sys.path: {}", e))
        })?;

        // Find site-packages under the venv
        let site_pkg = format!("{}/lib/python3/site-packages", path);
        let alt = format!("{}/lib/python3.12/site-packages", path);

        for sp in &[site_pkg.as_str(), alt.as_str()] {
            let code = format!("__import__('os').path.isdir('{}')", sp);
            match py.eval(&CString::new(code.as_bytes()).unwrap(), None, None) {
                Ok(r) if r.extract::<bool>().unwrap_or(false) => {
                    sys_path.call_method1("insert", (0, sp)).map_err(|e| {
                        ExtensionError::new(format!("Failed to insert path: {}", e))
                    })?;
                    // Store path
                    *VENV_PATH.lock().unwrap() = Some(path.to_string());
                    return Ok(());
                }
                _ => continue,
            }
        }
        Err(ExtensionError::new(format!(
            "No site-packages found in {}",
            path
        )))
    })
}

/// Check PYUD_VENV env var on extension load.
pub fn check_env_venv() {
    if let Ok(path) = std::env::var("PYUD_VENV") {
        if !path.is_empty() {
            let _ = activate_venv(&path);
        }
    }
}

/// `py_activate_venv(path VARCHAR) → VARCHAR`
///
/// Activates a Python virtual environment at the given path.
/// Returns 'OK' on success, or an error message on failure.
///
/// ```sql
/// SELECT py_activate_venv('/home/user/.venv');
/// SET py_venv = '/home/user/.venv';   -- DuckDB config (for current_setting)
/// ```
#[no_mangle]
pub unsafe extern "C" fn py_activate_venv(
    _info: libduckdb_sys::duckdb_function_info,
    input: libduckdb_sys::duckdb_data_chunk,
    output: libduckdb_sys::duckdb_vector,
) {
    let reader = unsafe { VectorReader::new(input, 0) };
    let row_count = reader.row_count();

    let path = if unsafe { reader.is_valid(0) } {
        unsafe { reader.read_str(0) }
    } else {
        let mut writer = unsafe { VectorWriter::new(output) };
        for row in 0..row_count {
            unsafe {
                writer.set_null(row);
            }
        }
        return;
    };

    let mut writer = unsafe { VectorWriter::new(output) };
    for row in 0..row_count {
        match activate_venv(path) {
            Ok(()) => unsafe {
                writer.write_varchar(row, "OK");
            },
            Err(e) => unsafe {
                writer.write_varchar(row, &format!("ERROR: {}", e));
            },
        }
    }
}

/// Get current venv path (for testing).
#[allow(dead_code)]
pub fn get_venv_path() -> Option<String> {
    VENV_PATH.lock().unwrap().clone()
}
