# duckdb-python — Python-In-DuckDB Extension

DuckDB loadable extension that embeds CPython inside DuckDB's process, enabling
`py_eval()`, `py_agg()`, `py_scan()`, and **named Python UDFs callable directly
by name** (e.g., `SELECT my_func('hello')`).

Built with [quack-rs](https://quack-rs.com) + [libduckdb-sys](https://crates.io/crates/libduckdb-sys) + [PyO3](https://pyo3.rs).

## Functions

| Function | Type | Signature | Description |
|----------|------|-----------|-------------|
| `py_activate_venv` | Scalar | `(path VARCHAR) -> VARCHAR` | Activate Python virtual environment at `path` |
| `py_register` | Scalar | `(name VARCHAR, expr VARCHAR, nargs INTEGER) -> VARCHAR` | Register a named Python function **and make it callable directly by name** |
| `py_call` | Scalar | `(name VARCHAR, arg VARCHAR) -> VARCHAR` | Invoke a registered Python function (also works for functions not callable directly) |
| `py_map` | Scalar | `(expr VARCHAR, value VARCHAR) -> VARCHAR` | Batch column eval |
| `py_eval` | Scalar | `(expr VARCHAR, value VARCHAR) -> VARCHAR` | Evaluate Python expression per row; `x` bound to value |
| `py_agg` | Aggregate | `(value VARCHAR, update VARCHAR, finalize VARCHAR) -> VARCHAR` | JSON state dict; `state` + `x` in scope |
| `py_scan` | Table | `(module VARCHAR, func VARCHAR) -> TABLE(...)` | Call Python function and return rows |

### Direct function calling (v1.2+)

After `py_register`, the function is registered as a native DuckDB scalar
function and can be called **directly by name** — no `py_call` wrapper needed.

```sql
SELECT py_register('double', 'str(int(x)*2)', 1);
SELECT double('5');           -- '10' ✨ direct call!
SELECT double('hello');       -- error (int conversion fails → NULL)
SELECT py_call('double', '5'); -- '10' (backward-compatible)
```

## Quick Start (from source)

### Prerequisites

- Rust toolchain
- Python 3.12 (with `python3.12-venv` on Linux)
- `make`, `ninja`
- Git

Clone with submodules:

```bash
git clone --recurse-submodules git@github.com:alitrack/duckdb-python.git
cd duckdb-python
```

If you already cloned without submodules:

```bash
git submodule update --init --recursive
```

### Build and package the extension

The final loadable artifact is **not** the raw Cargo `dylib`/`so`. You must build
through the project Makefile so DuckDB extension metadata is appended and a
`python.duckdb_extension` file is produced.

On Linux CI/build hosts, `make configure`, `make debug`, and `make release` attempt to
install Python development headers/libraries (`python3-dev` or distro equivalent) when a
system package manager is available, because PyO3 embedding builds need the Python
development package in addition to the interpreter itself.

```bash
make configure
make release
```

This produces:

- raw library: `target/release/libpython_ext.dylib` (or `.so` / `.dll`)
- packaged extension: `build/release/extension/python/python.duckdb_extension`

Local configure/test installs DuckDB `1.5.4` to match the packaged extension ABI.

### Test

```bash
make unit-test
make test_release
make smoke-test
```

`make test_release` runs SQLLogicTests through DuckDB's extension test runner.
`make smoke-test` loads the packaged extension in DuckDB and executes real Python-backed calls.
The bundled SQLLogicTest currently covers load gating, `py_eval`, `py_register`, and `py_call`.

## Example usage

After building, load the packaged extension file:

```sql
LOAD './build/release/extension/python/python.duckdb_extension';

SELECT py_eval('x.upper()', 'hello');
SELECT py_register('double_it', 'str(int(x)*2)', 1);
SELECT double_it('21');   -- direct call!
SELECT py_call('double_it', '21');  -- also works
```

Expected results:

```text
HELLO
OK
42
42
```

## Virtual Env Support

```sql
SET py_venv = '/path/to/venv';
SELECT py_activate_venv('/path/to/venv');
```

Or via environment variable before launching DuckDB:

```bash
PYUD_VENV=/path/to/venv duckdb -unsigned -c "LOAD './build/release/extension/python/python.duckdb_extension'; SELECT py_eval('x.upper()', 'hello')"
```

## Python selection notes

On macOS, Apple-provided `/usr/bin/python3` often does not provide a usable shared-library
layout for this project. The Makefile prefers `/opt/homebrew/bin/python3.12` when available
and exports `PYO3_PYTHON` automatically.

If your Python 3.12 lives elsewhere, override it explicitly:

```bash
PYO3_PYTHON=/path/to/python3.12 PYTHON_BIN=/path/to/python3.12 make configure release
```

## Why `make release` instead of plain `cargo build --release`?

A raw Cargo build only produces the shared library. DuckDB loadable extensions also need a
metadata footer appended to the binary. The `extension-ci-tools` workflow in this repo handles
that packaging step and emits the real `.duckdb_extension` artifact that DuckDB can load.

## CI

GitHub Actions currently runs a **macOS arm64 smoke workflow** that verifies:
- `make configure`
- `make release`
- `make test_release`
- `make smoke-test`

This intentionally avoids artifact upload because the repository hit GitHub Actions
artifact storage quota during the earlier multi-platform distribution workflow.

## License

MIT
