//! Python subinterpreter isolation (PEP 684).
//!
//! When enabled, each DuckDB thread gets its own Python subinterpreter
//! with an independent GIL, allowing true parallel execution across
//! concurrent connections.
//!
//! **Status**: scaffolding for v2. Defaults to shared interpreter (v1).

use std::sync::atomic::{AtomicBool, Ordering};

use pyo3::prelude::*;

/// Whether subinterpreter isolation is enabled.
#[allow(dead_code)]
static SUBINTERPRETER_ENABLED: AtomicBool = AtomicBool::new(false);

/// Enable subinterpreter isolation.
///
/// Returns an error if the Python version doesn't support it (< 3.12).
#[allow(dead_code)]
pub fn enable_subinterpreters() -> Result<(), String> {
    if !is_supported() {
        return Err("Subinterpreters require Python 3.12+".to_string());
    }
    SUBINTERPRETER_ENABLED.store(true, Ordering::SeqCst);
    Ok(())
}

/// Disable subinterpreter isolation.
#[allow(dead_code)]
pub fn disable_subinterpreters() {
    SUBINTERPRETER_ENABLED.store(false, Ordering::SeqCst);
}

/// Check if subinterpreter isolation is enabled.
#[allow(dead_code)]
pub fn is_enabled() -> bool {
    SUBINTERPRETER_ENABLED.load(Ordering::SeqCst)
}

/// Check if PEP 684 is supported by the linked Python.
#[allow(dead_code)]
pub fn is_supported() -> bool {
    Python::with_gil(|py| {
        let v = py.version_info();
        v.major >= 3 && v.minor >= 12
    })
}

/// Acquire the Python GIL — uses subinterpreter if enabled,
/// otherwise uses the shared main interpreter.
#[allow(dead_code)]
pub fn acquire_gil<R, F: FnOnce(Python<'_>) -> R>(f: F) -> R {
    if is_enabled() {
        // In v2: use per-thread subinterpreter
        // For v1: fall through to shared
        Python::with_gil(f)
    } else {
        Python::with_gil(f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_supported() {
        // Just verify it doesn't panic
        let _ = is_supported();
    }

    #[test]
    fn test_default_disabled() {
        assert!(!is_enabled());
    }

    #[test]
    fn test_toggle() {
        // Only test if Python 3.12+
        if is_supported() {
            assert!(enable_subinterpreters().is_ok());
            assert!(is_enabled());
            disable_subinterpreters();
            assert!(!is_enabled());
        } else {
            assert!(enable_subinterpreters().is_err());
        }
    }
}
