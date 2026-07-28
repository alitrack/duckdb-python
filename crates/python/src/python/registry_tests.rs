#[cfg(test)]
mod tests {
    use crate::python::registry::{register, lookup, list_names};

    #[test]
    fn test_register_and_lookup() {
        register("my_func", "x.upper()", 1, None);
        let expr = lookup("my_func");
        assert_eq!(expr, Some("x.upper()".to_string()));
    }

    #[test]
    fn test_lookup_missing() {
        assert_eq!(lookup("nonexistent"), None);
    }

    #[test]
    fn test_list_names() {
        register("func_a", "x+1", 1, None);
        register("func_b", "x*2", 1, None);
        let names = list_names();
        assert!(names.contains(&"func_a".to_string()));
        assert!(names.contains(&"func_b".to_string()));
    }

    #[test]
    fn test_register_overwrite() {
        register("same_name", "x+1", 1, None);
        register("same_name", "x*2", 2, None);
        let expr = lookup("same_name");
        assert_eq!(expr, Some("x*2".to_string()));
    }

    #[test]
    fn test_eval_registered_function() {
        register("double", "str(int(x)*2)", 1, None);
        // Use bridge eval to test end-to-end
        let result = super::super::bridge::eval_py_expr("str(int(x)*2)", "5");
        assert_eq!(result.unwrap(), "10");
    }
}
