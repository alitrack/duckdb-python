//! Unit tests for the Python bridge module.
//!
//! Tests Python expression evaluation, error handling, and type conversion.
//! DuckDB FFI functions (scalar callbacks) cannot be tested in cargo test
//! (loadable-extension feature panics on uninitialized dispatch table).

#[cfg(test)]
mod tests {
    use super::super::bridge;

    #[test]
    fn test_ensure_initialized() {
        assert!(bridge::ensure_initialized().is_ok());
        // Idempotent
        assert!(bridge::ensure_initialized().is_ok());
    }

    #[test]
    fn test_eval_simple_upper() {
        let result = bridge::eval_py_expr("x.upper()", "hello");
        assert!(result.is_ok(), "Expected Ok, got {:?}", result);
        assert_eq!(result.unwrap(), "HELLO");
    }

    #[test]
    fn test_eval_reverse() {
        let result = bridge::eval_py_expr("x[::-1]", "hello");
        assert_eq!(result.unwrap(), "olleh");
    }

    #[test]
    fn test_eval_len() {
        let result = bridge::eval_py_expr("str(len(x))", "rust");
        assert_eq!(result.unwrap(), "4");
    }

    #[test]
    fn test_eval_math() {
        let result = bridge::eval_py_expr("str(int(x) * 3)", "7");
        assert_eq!(result.unwrap(), "21");
    }

    #[test]
    fn test_eval_empty_string() {
        let result = bridge::eval_py_expr("x.upper()", "");
        assert_eq!(result.unwrap(), "");
    }

    #[test]
    fn test_eval_with_unicode() {
        let result = bridge::eval_py_expr("x + ' 世界'", "你好");
        assert_eq!(result.unwrap(), "你好 世界");
    }

    #[test]
    fn test_eval_none_return() {
        let result = bridge::eval_py_expr("None", "ignored");
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("None"), "Expected 'None' in error: {}", err);
    }

    #[test]
    fn test_eval_syntax_error() {
        let result = bridge::eval_py_expr("x..upper()", "hello");
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("py_eval"), "Expected 'py_eval' in error: {}", err);
    }

    #[test]
    fn test_eval_name_error() {
        let result = bridge::eval_py_expr("undefined_var + x", "hello");
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("py_eval"), "Expected 'py_eval' in error: {}", err);
    }
}
