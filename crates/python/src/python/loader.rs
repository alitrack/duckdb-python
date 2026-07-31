//! Runtime dynamic loading of libpython.
//!
//! Because the python extension is built with `pyo3/extension-module` (which does NOT link
//! to libpython at build time), the Python C API symbols are left unresolved
//! in the resulting shared library.  At runtime, before PyO3 is initialized,
//! we `dlopen` the appropriate `libpython3.x.so/dylib/dll` with `RTLD_GLOBAL`
//! so that those symbols become resolvable.
//!
//! The library to load is determined at extension LOAD time, in priority order:
//!   1. `PYUD_VENV` env var → ask that venv's own interpreter where its libpython
//!      lives (version always matches — dynamic per-environment binding)
//!   2. `PYUD_PYTHON_LIB` env var – explicit path to the shared library
//!   3. Auto-detect via `PYO3_PYTHON` / `python3` on `$PATH`
//!   4. Common system library names
//!
//! NOTE: DuckDB dlopens extensions with RTLD_NOW, so every symbol (incl. PyO3's
//! `_Py_DecRef`/`_Py_IncRef`) must be resolvable the moment the extension loads —
//! libpython therefore has to be in the global symbol table BEFORE the extension
//! is dlopen'd. That's why binding happens at LOAD time (via `PYUD_VENV`), not on
//! first `py_activate_venv` call. `py_activate_venv` then verifies the version and
//! switches site-packages.

#[cfg(feature = "embed")]
use std::ffi::CString;
#[cfg(feature = "embed")]
use std::os::raw::c_int;
#[cfg(feature = "embed")]
use std::sync::Mutex;

#[cfg(feature = "embed")]
use once_cell::sync::Lazy;

// dlopen flags: RTLD_LAZY | RTLD_GLOBAL
#[cfg(all(not(target_os = "windows"), feature = "embed"))]
const RTLD_LAZY: c_int = 1;
#[cfg(all(not(target_os = "windows"), feature = "embed"))]
const RTLD_GLOBAL: c_int = 0x100;

#[cfg(all(not(target_os = "windows"), feature = "embed"))]
extern "C" {
    fn dlopen(filename: *const std::os::raw::c_char, flag: c_int) -> *mut std::ffi::c_void;
    fn dlerror() -> *mut std::os::raw::c_char;
}

/// Tracks whether libpython has been successfully loaded.
#[cfg(feature = "embed")]
static LOAD_STATUS: Lazy<Mutex<Result<(), String>>> =
    Lazy::new(|| Mutex::new(load_libpython_internal()));

/// Try to load libpython at runtime so that pyo3's unresolved symbols
/// (Py_Initialize, PyObject_Call, etc.) are resolved from the loaded
/// shared library.
///
/// Returns `Ok(())` if the library was loaded successfully or was already
/// loaded; returns `Err` with a descriptive message on failure.
///
/// Returns `Ok(())` if the library was loaded successfully or was already
/// loaded; returns `Err` with a descriptive message on failure.
///
/// When NOT building with the `embed` feature, libpython is linked at
/// build time and this function is a no-op (returns Ok immediately).
pub fn ensure_libpython_loaded() -> Result<(), String> {
    // When not using extension-module, libpython is already linked —
    // symbols are available without dlopen.
    #[cfg(not(feature = "embed"))]
    {
        return Ok(());
    }
    #[cfg(feature = "embed")]
    {
        LOAD_STATUS.lock().unwrap().clone()
    }
}

/// Ensure libpython is loaded, resolving the target interpreter from a venv.
///
/// Called by `py_activate_venv(path)`: libpython is normally already loaded at
/// extension LOAD time (RTLD_NOW constraint) — this only loads it if that failed
/// (e.g. no default Python found). The venv is used as the resolution hint.
/// The extension itself is version-agnostic thanks to `abi3`.
#[cfg(feature = "embed")]
pub fn ensure_libpython_for_venv(venv: &str) -> Result<(), String> {
    let mut guard = LOAD_STATUS.lock().unwrap();
    let status = guard.clone();
    if status.is_ok() {
        return status;
    }
    let result = load_libpython_for_venv(venv);
    *guard = result.clone();
    result
}

#[cfg(feature = "embed")]
fn load_libpython_for_venv(venv: &str) -> Result<(), String> {
    match resolve_libpython_from_venv(venv) {
        Some(path) => dlopen_path(&path),
        None => load_libpython_internal(),
    }
}

/// Ask the venv's own interpreter where its libpython lives — the version
/// always matches the venv. Falls back to pyvenv.cfg / venv/lib scanning.
#[cfg(feature = "embed")]
fn resolve_libpython_from_venv(venv: &str) -> Option<String> {
    use std::path::Path;
    use std::process::Command;

    let python = format!("{}/bin/python", venv);
    if !Path::new(&python).exists() {
        return None;
    }

    // 1. Ask the venv interpreter itself (most reliable)
    let script = r#"
import sysconfig, os
libdir = sysconfig.get_config_var("LIBDIR")
ld = sysconfig.get_config_var("LDLIBRARY")
if libdir and ld:
    p = os.path.join(libdir, ld)
    if os.path.exists(p):
        print(p, end="")
"#;
    if let Ok(out) = Command::new(&python).args(["-c", script]).output() {
        if out.status.success() {
            let p = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !p.is_empty() && Path::new(&p).exists() {
                return Some(p);
            }
        }
    }

    // 2. Scan <venv>/lib/ for libpython3.X.so*
    if let Ok(entries) = std::fs::read_dir(format!("{}/lib", venv)) {
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if (name.starts_with("libpython") && name.ends_with(".so"))
                || (name.starts_with("libpython") && name.ends_with(".so.1.0"))
            {
                let p = e.path().to_string_lossy().to_string();
                if Path::new(&p).exists() {
                    return Some(p);
                }
            }
        }
    }

    // 3. pyvenv.cfg [home] → <home>/../lib/libpython3.X.so* (uv puts home under */bin)
    let cfg = std::fs::read_to_string(format!("{}/pyvenv.cfg", venv)).ok()?;
    for line in cfg.lines() {
        let Some(home) = line.strip_prefix("home =") else { continue };
        let home = Path::new(home.trim());
        let mut lib_dirs = vec![home.join("lib")];
        if let Some(parent) = home.parent() {
            lib_dirs.push(parent.join("lib"));
        }
        for dir in &lib_dirs {
            if let Ok(entries) = std::fs::read_dir(dir) {
                for e in entries.flatten() {
                    let name = e.file_name().to_string_lossy().to_string();
                    if name.starts_with("libpython") {
                        let p = e.path().to_string_lossy().to_string();
                        if Path::new(&p).exists() {
                            return Some(p);
                        }
                    }
                }
            }
        }
        break;
    }

    None
}

#[cfg(feature = "embed")]
fn load_libpython_internal() -> Result<(), String> {
    // 0. venv from env — dynamic per-environment binding (version always matches)
    if let Ok(venv) = std::env::var("PYUD_VENV") {
        if !venv.is_empty() {
            if let Some(path) = resolve_libpython_from_venv(&venv) {
                return dlopen_path(&path);
            }
        }
    }

    // 1. Explicit path from env var
    if let Ok(path) = std::env::var("PYUD_PYTHON_LIB") {
        if !path.is_empty() {
            return dlopen_path(&path);
        }
    }

    // 2. Auto-detect from PYO3_PYTHON or python3 on PATH
    if let Some(path) = autodetect_libpython() {
        return dlopen_path(&path);
    }

    // 3. Try common library names
    let mut candidates: Vec<&str> = Vec::new();
    #[cfg(target_os = "macos")]
    {
        // macOS framework paths
        candidates.extend_from_slice(&[
            "/opt/homebrew/Frameworks/Python.framework/Versions/3.12/Python",
            "/opt/homebrew/Frameworks/Python.framework/Versions/3.11/Python",
            "/Library/Frameworks/Python.framework/Versions/3.12/Python",
            "/Library/Frameworks/Python.framework/Versions/3.11/Python",
            "/usr/lib/libpython3.dylib",
        ]);
    }
    #[cfg(target_os = "linux")]
    {
        candidates.extend_from_slice(&[
            "libpython3.12.so.1.0",
            "libpython3.12.so",
            "libpython3.11.so.1.0",
            "libpython3.11.so",
            "libpython3.10.so.1.0",
            "libpython3.10.so",
            "libpython3.so",
        ]);
    }
    // Generic fallbacks
    candidates.extend_from_slice(&[
        "libpython3.12.dylib",
        "libpython3.11.dylib",
        "libpython3.10.dylib",
    ]);

    let mut last_err = String::new();
    for name in candidates {
        match dlopen_name(name) {
            Ok(()) => return Ok(()),
            Err(e) => last_err = e,
        }
    }

    Err(format!(
        "Could not load libpython. Set PYUD_PYTHON_LIB to the full path of \
         your libpython3.x.so/dylib, or ensure Python is installed. \
         Last error: {}",
        last_err
    ))
}

/// Auto-detect the libpython path by interrogating the Python executable.
#[cfg(feature = "embed")]
fn autodetect_libpython() -> Option<String> {
    use std::process::Command;

    let python = std::env::var("PYO3_PYTHON").unwrap_or_else(|_| "python3".to_string());

    // Ask Python where its shared library is
    let script = r#"
import sys, sysconfig, os, ctypes.util
libdir = sysconfig.get_config_var("LIBDIR")
ldlibrary = sysconfig.get_config_var("LDLIBRARY")
library = sysconfig.get_config_var("LIBRARY")
# On macOS, the framework path
if sys.platform == "darwin":
    fwk = sysconfig.get_config_var("PYTHONFRAMEWORKPREFIX")
    if fwk:
        for ver in ("3.12", "3.11", "3.10"):
            p = os.path.join(fwk, "Versions", ver, "Python")
            if os.path.exists(p):
                print(p, end="")
                raise SystemExit(0)
    # Homebrew
    for prefix in ("/opt/homebrew/Frameworks/Python.framework",
                   "/Library/Frameworks/Python.framework"):
        for ver in ("3.12", "3.11", "3.10"):
            p = os.path.join(prefix, "Versions", ver, "Python")
            if os.path.exists(p):
                print(p, end="")
                raise SystemExit(0)
# Try LDLIBRARY first
if ldlibrary and libdir:
    p = os.path.join(libdir, ldlibrary)
    if os.path.exists(p):
        print(p, end="")
        raise SystemExit(0)
# Linux: try ctypes.util.find_library
for d in ("python3.12","python3.11","python3.10","python3"):
    found = ctypes.util.find_library(d)
    if found:
        print(found, end="")
        raise SystemExit(0)
if libdir:
    for name in ("libpython3.so","libpython3.12.so","libpython3.11.so"):
        p = os.path.join(libdir, name)
        if os.path.exists(p):
            print(p, end="")
            raise SystemExit(0)
"#;

    let output = Command::new(&python).args(["-c", script]).output().ok()?;

    if !output.status.success() {
        return None;
    }

    let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if path.is_empty() {
        None
    } else {
        Some(path)
    }
}

#[cfg(feature = "embed")]
fn dlopen_path(path: &str) -> Result<(), String> {
    let c_path = CString::new(path).map_err(|e| format!("Invalid path (null byte): {}", e))?;
    unsafe { dlopen_helper(&c_path, Some(path)) }
}

#[cfg(feature = "embed")]
fn dlopen_name(name: &str) -> Result<(), String> {
    let c_name = CString::new(name).map_err(|e| format!("Invalid name (null byte): {}", e))?;
    unsafe { dlopen_helper(&c_name, None) }
}

#[cfg(feature = "embed")]
unsafe fn dlopen_helper(c_name: &CString, display: Option<&str>) -> Result<(), String> {
    // Clear any prior error
    dlerror();

    let handle = dlopen(c_name.as_ptr(), RTLD_LAZY | RTLD_GLOBAL);
    if handle.is_null() {
        let err = unsafe { dlerror() };
        let msg = if err.is_null() {
            "unknown error".to_string()
        } else {
            std::ffi::CStr::from_ptr(err).to_string_lossy().into_owned()
        };
        let label = display.unwrap_or_else(|| c_name.to_str().unwrap_or("?"));
        return Err(format!("dlopen('{}') failed: {}", label, msg));
    }
    Ok(())
}
