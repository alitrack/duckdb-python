---
name: duckdb-python-udf
description: >
  Call Python from DuckDB SQL with the python_udf extension: py_register
  named functions callable directly by name (SELECT my_func('x')), per-row
  py_eval, batch py_map, JSON-state py_agg aggregates, py_scan table
  functions, and virtualenv activation. Use when an algorithm exists only
  as Python code and the query must stay in SQL. Requires DuckDB 1.5.4+.
version: 0.2.1
---

# python_udf: CPython inside DuckDB

Extension: `python_udf` (PyO3 + quack-rs; embeds CPython in-process).
`INSTALL python_udf FROM community; LOAD python_udf;` (DuckDB 1.5.4+)

## Named UDFs — callable directly by name

```sql
SELECT py_register('double', 'str(int(x)*2)', 1);  -- → 'OK'
SELECT double('5');                                -- → '10'  direct call!
SELECT py_call('double', '5');                     -- backward-compatible wrapper
```

After `py_register(name, expr, nargs)` the function is a **native DuckDB
scalar function** — call it bare like any built-in. Works for any Python
expression over `x` (args bind positionally up to nargs).

## Row / batch / aggregate

```sql
-- Per-row eval, x bound to value
SELECT py_eval('x.upper()', name) FROM customers;

-- Batch column eval
SELECT py_map('x.split("@")[1]', email) FROM users;

-- Aggregate with JSON state: state + x in scope each row, finalize at end
SELECT py_agg(amount, 'state["s"] += float(x)', 'str(state["s"])')
FROM orders;
```

## Table function — rows from Python

```sql
SELECT * FROM py_scan('mymodule', 'generate_rows');   -- calls module.func()
```

The Python function returns an iterable of rows; DuckDB types them by
first batch.

## Virtualenv

```sql
SELECT py_activate_venv('/path/to/venv');   -- site-packages from your venv
```

Activate before registering functions that import third-party packages
(pandas, etc.).

## Gotchas

- All values cross the boundary as VARCHAR (JSON for state) — parse/cast
  inside the expression.
- py_register opens a fresh connection from the cached DB handle for
  registration (bypasses the init-lifecycle segfault); never register inside
  other extension callbacks.
- Heavy per-row Python is slow by nature: prefer py_map batching or push
  work into a py_scan generator.
