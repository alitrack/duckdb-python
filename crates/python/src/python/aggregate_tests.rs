#[cfg(test)]
mod tests {
    use super::super::aggregate as A;

    fn upd(e: &str, j: &str, v: &str) -> Result<String, String> { A::__test_upd_json(e, j, v) }
    fn mrg(a: &str, b: &str) -> Result<String, String> { A::__test_merge_json(a, b) }
    fn fin(e: &str, j: &str) -> Result<String, String> { A::__test_fin_json(e, j) }

    #[test] fn test_update_sum() {
        let r = upd("state['sum']=state.get('sum',0)+int(x)", r#"{"sum":0}"#, "5");
        assert!(r.is_ok(), "{:?}", r);
        assert!(r.unwrap().contains("sum"));
    }
    #[test] fn test_accumulate() {
        let mut s = r#"{"sum":0,"n":0}"#.to_string();
        for v in &["3","7","2"] {
            s = upd("state['sum']=state.get('sum',0)+int(x);state['n']=state.get('n',0)+1",&s,v).unwrap();
        }
        assert!(s.contains("sum"));
        assert!(s.contains("n"));
    }
    #[test] fn test_merge() {
        let r = mrg(r#"{"sum":10}"#, r#"{"sum":6,"extra":1}"#);
        assert!(r.is_ok(), "{:?}", r);
        assert!(r.unwrap().contains("sum"));
    }
    #[test] fn test_finalize_mean() {
        assert_eq!(fin("str(state['sum']/state['n'])", r#"{"sum":12,"n":3}"#).unwrap(), "4.0");
    }
    #[test] fn test_finalize_custom() {
        assert_eq!(fin("str(sum(state['values'])/len(state['values']))", r#"{"values":[1,2,3,4,5]}"#).unwrap(), "3.0");
    }
    #[test] fn test_empty_state() {
        let r = upd("state['count']=state.get('count',0)+1", "{}", "x");
        assert!(r.is_ok(), "{:?}", r);
    }
    #[test] fn test_update_error() {
        let r = upd("undefined_var+x", "{}", "5");
        assert!(r.is_err());
    }
    #[test] fn test_finalize_error() {
        assert!(fin("state['nope']", r#"{"a":1}"#).is_err());
    }
}
