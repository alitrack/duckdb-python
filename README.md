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
| `py_eval` | Scalar | `(expr VARCHAR, value VARCHAR) → VARCHAR` | Evaluate Python expression per row; `x` bound to value |
| `py_agg` | Aggregate | `(value VARCHAR, update VARCHAR, finalize VARCHAR) → VARCHAR` | JSON state dict; `state` + `x` in scope |
| `py_scan` | Table | `(module VARCHAR, func VARCHAR) → TABLE(...)` | Call Python function, return result as table |

## Virtual Env Support

```bash
# Activate a venv at extension load time
PYUD_VENV=/path/to/venv duckdb -c "LOAD pyudf; SELECT py_eval('...', val) FROM data"
```

All pip-installed packages in the venv are available to UDFs.

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
- [ ] `CREATE FUNCTION ... LANGUAGE python` syntax
- [ ] `SET py_venv = '...'` config option
- [ ] Arrow zero-copy optimization
- [ ] Python subinterpreter isolation (PEP 684)
- [ ] Multi-platform CI builds

## License

MIT
