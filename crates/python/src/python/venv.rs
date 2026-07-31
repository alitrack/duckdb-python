//! Python virtual environment support.
//!
//! Provides `py_activate_venv(path)` to activate a Python venv by adding
//! its site-packages to sys.path. Also checks PYUD_VENV env var at load.

use std::sync::Mutex;

use once_cell::sync::Lazy;
use pyo3::prelude::*;
use quack_rs::error::ExtensionError;
use quack_rs::vector::{VectorReader, VectorWriter};

use super::{bridge, loader};

static VENV_PATH: Lazy<Mutex<Option<String>>> = Lazy::new(|| Mutex::new(None));

/// Activate a Python virtual environment by adding its site-packages to sys.path.
pub fn activate_venv(path: &str) -> Result<(), ExtensionError> {
    // Dynamic binding: resolve + dlopen the venv's own libpython (first load wins),
    // then initialize the interpreter with it — the Python version is determined by
    // the environment, not by build time or PATH probing.
    loader::ensure_libpython_for_venv(path).map_err(|e| {
        ExtensionError::new(format!("Failed to load libpython for venv '{}': {}", path, e))
    })?;
    bridge::ensure_initialized()?;

    Python::with_gil(|py| {
        let sys = py.import("sys").map_err(|e| {
            ExtensionError::new(format!("Failed to import sys: {}", e))
        })?;
        let sys_path = sys.getattr("path").map_err(|e| {
            ExtensionError::new(format!("Failed to get sys.path: {}", e))
        })?;

        // Version guard: the running interpreter must match the venv's Python version
        let venv_ver = venv_python_version(path);
        if let Some(vv) = venv_ver {
            let sysver = py
                .import("sys")
                .and_then(|s| s.getattr("version_info"))
                .map(|vi| {
                    let major: i32 = vi.getattr("major").and_then(|v| v.extract()).unwrap_or(-1);
                    let minor: i32 = vi.getattr("minor").and_then(|v| v.extract()).unwrap_or(-1);
                    format!("{}.{}", major, minor)
                })
                .unwrap_or_else(|_| "?".to_string());
            if !sysver.eq_ignore_ascii_case(&vv) && sysver != "?" {
                return Err(ExtensionError::new(format!(
                    "Interpreter version {} does not match venv Python {} — \
                     the interpreter was already initialized (e.g. py_eval ran before \
                     py_activate_venv). Activate the venv FIRST in a fresh connection.",
                    sysver, vv
                )));
            }
        }

        // Find site-packages under the venv (version-aware)
        let mut candidates = Vec::new();
        if let Some(sp) = venv_site_packages(path) {
            candidates.push(sp);
        }
        if let Ok(entries) = std::fs::read_dir(format!("{}/lib", path)) {
            for e in entries.flatten() {
                let name = e.file_name().to_string_lossy().to_string();
                if name.starts_with("python3") {
                    let sp = format!("{}/lib/{}/site-packages", path, name);
                    candidates.push(sp);
                }
            }
        }

        for sp in candidates {
            // Use Rust filesystem check instead of Python eval — eliminates both
            // the code injection vector and the GIL overhead of a Python call.
            if std::path::Path::new(&sp).is_dir() {
                sys_path.call_method1("insert", (0, sp)).map_err(|e| {
                    ExtensionError::new(format!("Failed to insert path: {}", e))
                })?;
                // Store path
                *VENV_PATH.lock().unwrap() = Some(path.to_string());
                return Ok(());
            }
        }
        Err(ExtensionError::new(format!(
            "No site-packages found in {}",
            path
        )))
    })
}

/// Ask the venv's own interpreter for its site-packages directory.
fn venv_site_packages(venv: &str) -> Option<String> {
    let python = format!("{}/bin/python", venv);
    if !std::path::Path::new(&python).exists() {
        return None;
    }
    let script = "import site; print(site.getsitepackages()[0], end='')";
    let out = std::process::Command::new(&python).args(["-c", script]).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let p = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if p.is_empty() || !std::path::Path::new(&p).is_dir() {
        None
    } else {
        Some(p)
    }
}

/// Ask the venv's own interpreter for its version ("X.Y").
fn venv_python_version(venv: &str) -> Option<String> {
    let python = format!("{}/bin/python", venv);
    if !std::path::Path::new(&python).exists() {
        return None;
    }
    let out = std::process::Command::new(&python)
        .args(["-c", "import sys; print(f'{sys.version_info.major}.{sys.version_info.minor}', end='')"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let v = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if v.is_empty() { None } else { Some(v) }
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
