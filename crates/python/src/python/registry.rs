//! Python function registry.
//!
//! Stores named Python functions registered via `py_register()`
//! and invokes them via `py_call()` or directly by name (after `py_register`
//! dynamically registers a DuckDB scalar function with the correct return type).

use std::collections::HashMap;
use std::ffi::{c_char, c_void, CStr, CString};
use std::sync::atomic::Ordering;
use std::sync::Mutex;

use libduckdb_sys as ffi;
use once_cell::sync::Lazy;
use quack_rs::scalar::{ScalarFunctionBuilder, ScalarFunctionInfo};
use quack_rs::types::TypeId;
use quack_rs::vector::{VectorReader, VectorWriter};

use super::bridge::eval_py_expr;

/// SQL reserved words that cannot be used as function names.
const RESERVED: &[&str] = &[
    "add", "all", "alter", "and", "any", "as", "asc", "between", "by", "case",
    "check", "column", "constraint", "create", "cross", "default", "delete",
    "desc", "distinct", "double", "drop", "else", "end", "except", "exists",
    "false", "float", "foreign", "from", "full", "group", "having", "if", "in",
    "inner", "insert", "int", "integer", "intersect", "into", "is", "join",
    "key", "left", "like", "limit", "not", "null", "of", "offset", "on", "or",
    "order", "outer", "over", "primary", "real", "references", "replace",
    "right", "select", "set", "some", "struct", "table", "then", "true",
    "union", "unique", "update", "using", "values", "varchar", "view", "when",
    "where", "with",
];

fn is_reserved(name: &str) -> bool {
    RESERVED.iter().any(|&r| r.eq_ignore_ascii_case(name))
}

/// Entry in the function registry.
#[derive(Clone)]
struct RegistryEntry {
    expr: String,
    nargs: usize,
    return_type: Option<TypeId>,
}

/// Global registry: name → (expression, arg_count, return_type).
static REGISTRY: Lazy<Mutex<HashMap<String, RegistryEntry>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

/// Register a named Python function with optional return type hint.
pub fn register(name: &str, expr: &str, nargs: usize, return_type: Option<TypeId>) {
    REGISTRY.lock().unwrap().insert(
        name.to_string(),
        RegistryEntry {
            expr: expr.to_string(),
            nargs,
            return_type,
        },
    );
}

/// Look up a registered function's expression.
pub fn lookup(name: &str) -> Option<String> {
    REGISTRY
        .lock()
        .unwrap()
        .get(name)
        .map(|e| e.expr.clone())
}

/// Look up a registered function's return type.
pub fn lookup_return_type(name: &str) -> Option<TypeId> {
    REGISTRY.lock().unwrap().get(name).and_then(|e| e.return_type)
}

/// List all registered function names.
pub fn list_names() -> Vec<String> {
    REGISTRY.lock().unwrap().keys().cloned().collect()
}

// ── Parse return type string ──

/// Parse a return type string to TypeId.
/// Accepted: "BIGINT", "INTEGER", "DOUBLE", "FLOAT", "VARCHAR", "BOOLEAN", "DATE", "TIMESTAMP".
/// Case-insensitive.
fn parse_return_type(s: &str) -> Option<TypeId> {
    match s.to_uppercase().as_str() {
        "BIGINT" | "INT8" | "INTEGER" | "INT" | "INT4" => Some(TypeId::BigInt),
        "DOUBLE" | "FLOAT8" | "FLOAT" | "REAL" | "FLOAT4" => Some(TypeId::Double),
        "BOOLEAN" | "BOOL" => Some(TypeId::Boolean),
        "DATE" => Some(TypeId::Date),
        "TIMESTAMP" => Some(TypeId::Timestamp),
        "VARCHAR" | "TEXT" | "STRING" | "" => Some(TypeId::Varchar),
        _ => None, // unknown type → return None (caller can default to Varchar)
    }
}

// ── Raw DuckDB vector helpers (for typed output) ──

unsafe fn set_null(vec: ffi::duckdb_vector, idx: usize) {
    unsafe {
        ffi::duckdb_vector_ensure_validity_writable(vec);
        let validity = ffi::duckdb_vector_get_validity(vec);
        if !validity.is_null() {
            ffi::duckdb_validity_set_row_invalid(validity, idx as u64);
        }
    }
}

unsafe fn write_bigint(vec: ffi::duckdb_vector, idx: usize, val: i64) {
    unsafe {
        let data = ffi::duckdb_vector_get_data(vec) as *mut i64;
        *data.add(idx) = val;
    }
}

unsafe fn write_double(vec: ffi::duckdb_vector, idx: usize, val: f64) {
    unsafe {
        let data = ffi::duckdb_vector_get_data(vec) as *mut f64;
        *data.add(idx) = val;
    }
}

unsafe fn write_bool(vec: ffi::duckdb_vector, idx: usize, val: bool) {
    unsafe {
        let data = ffi::duckdb_vector_get_data(vec) as *mut u8;
        *data.add(idx) = val as u8;
    }
}

// ── Typed write: dispatch based on TypeId ──

/// Write a string value to a DuckDB vector as the specified type.
/// Returns false if parsing fails (caller should write null).
unsafe fn write_typed(vec: ffi::duckdb_vector, idx: usize, val: &str, tid: TypeId) -> bool {
    match tid {
        TypeId::BigInt => {
            if let Ok(i) = val.parse::<i64>() {
                unsafe { write_bigint(vec, idx, i) };
                true
            } else {
                false
            }
        }
        TypeId::Double => {
            if let Ok(f) = val.parse::<f64>() {
                unsafe { write_double(vec, idx, f) };
                true
            } else {
                false
            }
        }
        TypeId::Boolean => {
            let b = val.eq_ignore_ascii_case("true") || val == "1";
            unsafe { write_bool(vec, idx, b) };
            true
        }
        TypeId::Date => {
            // Parse YYYY-MM-DD → days since epoch
            if val.len() == 10
                && val.as_bytes().get(4) == Some(&b'-')
                && val.as_bytes().get(7) == Some(&b'-')
            {
                let y: i32 = val[..4].parse().unwrap_or(1970);
                let m: u32 = val[5..7].parse().unwrap_or(1);
                let d: u32 = val[8..10].parse().unwrap_or(1);
                let days = days_since_epoch(y, m, d);
                let data = ffi::duckdb_vector_get_data(vec) as *mut i32;
                unsafe { *data.add(idx) = days };
                true
            } else {
                false
            }
        }
        _ => {
            // Varchar / unknown: use VectorWriter
            let mut w = VectorWriter::new(vec);
            unsafe { w.write_varchar(idx, val) };
            true
        }
    }
}

fn days_since_epoch(y: i32, m: u32, d: u32) -> i32 {
    let (y, m) = if m <= 2 {
        (y as i64 - 1, m as i64 + 12)
    } else {
        (y as i64, m as i64)
    };
    let d = d as i64;
    let era = if y >= 0 { y / 400 } else { (y - 399) / 400 };
    let yoe = y - era * 400;
    let doy = (153 * (m - 3) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    (era * 146097 + doe - 719468) as i32
}

// ── Dynamic dispatch: one callback for all user-registered functions ──

/// Destroy callback for extra_info: frees the CString holding the function name.
unsafe extern "C" fn free_string(ptr: *mut c_void) {
    if !ptr.is_null() {
        drop(CString::from_raw(ptr as *mut c_char));
    }
}

/// Generic dispatch callback for dynamically registered Python UDFs.
///
/// Reads the function name from `extra_info`, looks up the Python body
/// and return type in REGISTRY, evaluates against input, writes typed output.
unsafe extern "C" fn py_dispatch(
    info: ffi::duckdb_function_info,
    input: ffi::duckdb_data_chunk,
    output: ffi::duckdb_vector,
) {
    // 1. Get function name from extra_info
    let func_info = unsafe { ScalarFunctionInfo::new(info) };
    let name_ptr = func_info.get_extra_info() as *const c_char;
    let name = if name_ptr.is_null() {
        fill_null(output, 0);
        return;
    } else {
        match unsafe { CStr::from_ptr(name_ptr) }.to_str() {
            Ok(n) => n.to_string(),
            Err(_) => {
                fill_null(output, 0);
                return;
            }
        }
    };

    // 2. Look up function body + return type
    let (body, ret_type) = match REGISTRY.lock().unwrap().get(&name) {
        Some(e) => (e.expr.clone(), e.return_type),
        None => {
            fill_null(output, 0);
            return;
        }
    };

    // 3. Read input column as `x`
    let arg_reader = unsafe { VectorReader::new(input, 0) };
    let row_count = arg_reader.row_count();

    for row in 0..row_count {
        if unsafe { !arg_reader.is_valid(row) } {
            unsafe { set_null(output, row) };
            continue;
        }
        let arg = unsafe { arg_reader.read_str(row) };
        match eval_py_expr(&body, &arg) {
            Ok(result) => {
                if let Some(tid) = ret_type {
                    if unsafe { !write_typed(output, row, &result, tid) } {
                        unsafe { set_null(output, row) };
                    }
                } else {
                    let mut writer = unsafe { VectorWriter::new(output) };
                    unsafe { writer.write_varchar(row, &result) };
                }
            }
            Err(_) => unsafe { set_null(output, row) },
        }
    }
}

// ── py_register: stores in REGISTRY + registers native DuckDB function ──

/// `py_register(name VARCHAR, expr VARCHAR, nargs INTEGER [, rettype VARCHAR]) → VARCHAR`
///
/// The optional `rettype` parameter specifies the return type of the registered
/// function. Supported: BIGINT, DOUBLE, BOOLEAN, DATE, TIMESTAMP, VARCHAR (default).
///
/// ```sql
/// SELECT py_register('dbl', 'int(x)*2', 1, 'BIGINT');
/// SELECT dbl('21');  -- 42 (BIGINT, not '42'!)
/// ```
#[no_mangle]
pub unsafe extern "C" fn py_register(
    _info: ffi::duckdb_function_info,
    input: ffi::duckdb_data_chunk,
    output: ffi::duckdb_vector,
) {
    let name_reader = unsafe { VectorReader::new(input, 0) };
    let expr_reader = unsafe { VectorReader::new(input, 1) };
    let nargs_reader = unsafe { VectorReader::new(input, 2) };
    let row_count = name_reader.row_count();

    let name = if unsafe { name_reader.is_valid(0) } {
        unsafe { name_reader.read_str(0) }
    } else {
        fill_null(output, row_count);
        return;
    };

    let expr = if unsafe { expr_reader.is_valid(0) } {
        unsafe { expr_reader.read_str(0) }
    } else {
        fill_null(output, row_count);
        return;
    };

    let nargs = if unsafe { nargs_reader.is_valid(0) } {
        (unsafe { nargs_reader.read_i32(0) }) as usize
    } else {
        1
    };

    // Read optional 4th param: return type (empty string = default VARCHAR)
    let rt_reader = unsafe { VectorReader::new(input, 3) };
    let ret_type_str = if row_count > 0 && unsafe { rt_reader.is_valid(0) } {
        unsafe { rt_reader.read_str(0) }.to_string()
    } else {
        String::new()
    };
    let ret_type = parse_return_type(&ret_type_str);

    // Store in REGISTRY
    register(&name, &expr, nargs, ret_type);

    // Reject SQL reserved words
    if is_reserved(&name) {
        let mut writer = unsafe { VectorWriter::new(output) };
        for row in 0..row_count {
            unsafe { writer.write_varchar(row, "RESERVED"); }
        }
        return;
    }

    // Open long-lived connection for dynamic registration
    let con = crate::LONG_LIVED_CON.load(Ordering::Acquire);
    if con.is_null() {
        let mut writer = unsafe { VectorWriter::new(output) };
        for row in 0..row_count {
            unsafe { writer.write_varchar(row, "NOCON"); }
        }
        return;
    }

    // Register DuckDB scalar function with the correct return type
    let builder = match ScalarFunctionBuilder::try_new(&name) {
        Ok(b) => b,
        Err(e) => {
            let msg = format!("BADNAME: {}", e);
            let mut writer = unsafe { VectorWriter::new(output) };
            for row in 0..row_count {
                unsafe { writer.write_varchar(row, &msg); }
            }
            return;
        }
    };

    let name_c = CString::new(&name[..]).unwrap();
    let name_ptr = name_c.into_raw() as *mut c_void;

    let result = unsafe {
        let mut b = builder.param(TypeId::Varchar);
        if let Some(tid) = ret_type {
            b = b.returns(tid);
        } else {
            b = b.returns(TypeId::Varchar);
        }
        b.function(py_dispatch)
            .extra_info(name_ptr, Some(free_string))
            .register(con)
    };

    if result.is_err() {
        drop(unsafe { CString::from_raw(name_ptr as *mut c_char) });
    }

    let mut writer = unsafe { VectorWriter::new(output) };
    for row in 0..row_count {
        unsafe { writer.write_varchar(row, "OK"); }
    }
}

// ── py_call: backward-compatible explicit invocation ──

/// `py_call(name VARCHAR, arg VARCHAR) → <registered return type>`
///
/// Invokes a previously registered Python function with one argument.
#[no_mangle]
pub unsafe extern "C" fn py_call(
    _info: ffi::duckdb_function_info,
    input: ffi::duckdb_data_chunk,
    output: ffi::duckdb_vector,
) {
    let name_reader = unsafe { VectorReader::new(input, 0) };
    let arg_reader = unsafe { VectorReader::new(input, 1) };
    let row_count = name_reader.row_count();

    let (expr, ret_type) = if unsafe { name_reader.is_valid(0) } {
        let name = unsafe { name_reader.read_str(0) };
        match REGISTRY.lock().unwrap().get(&name as &str) {
            Some(e) => (e.expr.clone(), e.return_type),
            None => {
                let mut writer = unsafe { VectorWriter::new(output) };
                for row in 0..row_count {
                    unsafe {
                        writer.write_varchar(
                            row,
                            &format!("ERROR: function '{}' not registered", name),
                        );
                    }
                }
                return;
            }
        }
    } else {
        fill_null(output, row_count);
        return;
    };

    for row in 0..row_count {
        if unsafe { !arg_reader.is_valid(row) } {
            unsafe { set_null(output, row) };
            continue;
        }
        let arg = unsafe { arg_reader.read_str(row) };
        match eval_py_expr(&expr, &arg) {
            Ok(result) => {
                if let Some(tid) = ret_type {
                    if unsafe { !write_typed(output, row, &result, tid) } {
                        unsafe { set_null(output, row) };
                    }
                } else {
                    let mut writer = unsafe { VectorWriter::new(output) };
                    unsafe { writer.write_varchar(row, &result) };
                }
            }
            Err(_) => unsafe { set_null(output, row) },
        }
    }
}

// ── py_list_functions ──

fn fill_null(output: ffi::duckdb_vector, count: usize) {
    let mut writer = unsafe { VectorWriter::new(output) };
    for row in 0..count {
        unsafe { writer.set_null(row); }
    }
}

#[no_mangle]
pub unsafe extern "C" fn py_list_functions(
    _info: ffi::duckdb_function_info,
    _input: ffi::duckdb_data_chunk,
    output: ffi::duckdb_vector,
) {
    let names = list_names();
    let result = names.join(",");
    let mut writer = unsafe { VectorWriter::new(output) };
    unsafe { writer.write_varchar(0, &result) };
}
