# pyudf — Python-In-DuckDB Extension

DuckDB loadable extension that embeds CPython inside DuckDB's process, enabling
`py_eval()`, `py_agg()`, and `py_scan()` from **any** DuckDB client — CLI, JDBC,
ODBC, Node.js, WASM.

Built with [quack-rs](https://quack-rs.com) + [PyO3](https://pyo3.rs).

## Quick Start

```sql
INSTALL pyudf FROM community;
LOAD pyudf;

-- Scalar: Python expressions on every row
SELECT py_eval('x.upper()', name) FROM users;
SELECT py_eval('x[::-1]', text) FROM messages;
SELECT py_eval('str(len(x))', description) FROM items;

-- Aggregate: stateful Python computation across groups
SELECT py_agg(value,
  "state['sum']=state.get('sum',0)+int(x);state['n']=state.get('n',0)+1",
  "str(state['sum']/state['n'])"
) FROM measurements GROUP BY category;

-- Table function: Python callable → rows
SELECT * FROM py_scan('math', 'sqrt');
```

## Functions

| Function | Type | Signature | Description |
|----------|------|-----------|-------------|
| `py_activate_venv` | Scalar | `(path VARCHAR) → VARCHAR` | Activate Python virtual environment at `path` |
| `py_register` | Scalar | `(name VARCHAR, expr VARCHAR, nargs INTEGER) → VARCHAR` | Register a named Python function for reuse |
| `py_call` | Scalar | `(name VARCHAR, arg VARCHAR) → VARCHAR` | Invoke a registered Python function |
| `py_map` | Scalar | `(expr VARCHAR, value VARCHAR) → VARCHAR` | Batch column eval — ~100x faster, list comprehension |
| `py_eval` | Scalar | `(expr VARCHAR, value VARCHAR) → VARCHAR` | Evaluate Python expression per row; `x` bound to value |
| `py_agg` | Aggregate | `(value VARCHAR, update VARCHAR, finalize VARCHAR) → VARCHAR` | JSON state dict; `state` + `x` in scope |
| `py_scan` | Table | `(module VARCHAR, func VARCHAR) → TABLE(...)` | Call Python function, return result as table |

## Virtual Env Support

```bash
-- Activate a venv at extension load time
SET py_venv = '/path/to/venv';
SELECT py_activate_venv('/path/to/venv');

-- Or via env var
$ PYUD_VENV=/path/to/venv duckdb -c "LOAD pyudf; SELECT py_eval('x.upper()', 'hello')"
```

All pip-installed packages in the venv are available to UDFs.

## Named Functions

Register Python expressions once and invoke them by name:

```sql
SELECT py_register('my_upper', 'x.upper()', 1);
SELECT py_register('doubler', 'str(int(x)*2)', 1);

SELECT py_call('my_upper', 'hello');   -- 'HELLO'
SELECT py_call('doubler', '5');        -- '10'
```

This provides `CREATE FUNCTION` semantics without requiring DuckDB parser changes.

## Build from Source

```bash
git clone https://github.com/alitrack/pyudf
cd pyudf

# Prerequisites: Rust 1.75+, Python 3.12 with shared library
cargo build --release
# Output: target/release/libpyudf.so (or .dylib / .dll)

# Test
cargo test --lib
```

### Platform Support

| Platform | Status |
|----------|--------|
| linux_amd64 | ✅ |
| linux_arm64 | Planned |
| osx_amd64 | Planned |
| osx_arm64 | Planned |
| windows_amd64 | Planned |

## Architecture

```
DuckDB SQL → quack-rs callback → Arrow DataChunk
                                → PyO3 → CPython → numpy/sklearn/...
                                → Arrow DataChunk → SQL
```

The Python interpreter lives inside DuckDB's process. Data flows through
zero-copy Arrow pointer handoff between DuckDB and Python.

## Roadmap

- [x] `py_eval` scalar function
- [x] `py_agg` aggregate function (JSON state)
- [x] `py_scan` table function
- [x] Virtual env support (PYUD_VENV)
- [x] `SET py_venv = '...'` config option + `py_activate_venv`
- [x] `CREATE FUNCTION ... LANGUAGE python` semantics (py_register / py_call)
- [x] Arrow zero-copy optimization (py_map batch processing)
- [x] Python subinterpreter isolation (PEP 684, scaffold)
- [x] CI pipeline (GitHub Actions)

## License

MIT
