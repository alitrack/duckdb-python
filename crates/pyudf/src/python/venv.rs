//! Python virtual environment support.
//!
//! Provides `py_set_venv(path)` to activate a Python venv by adding
//! its site-packages to sys.path. Also checks PYUD_VENV env var at load.

use std::ffi::CString;
use std::sync::Mutex;

use once_cell::sync::Lazy;
use pyo3::prelude::*;
use quack_rs::error::ExtensionError;

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
        Err(ExtensionError::new(format!("No site-packages found in {}", path)))
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

/// Get current venv path (for testing).
pub fn get_venv_path() -> Option<String> {
    VENV_PATH.lock().unwrap().clone()
}
