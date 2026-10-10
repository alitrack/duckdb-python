# duckdb-python-udf — Claude Code Plugin

One skill giving your AI assistant verified knowledge of the
[duckdb-python](https://github.com/alitrack/duckdb-python) python_udf
extension: named Python UDFs callable directly by name in SQL, py_eval /
py_map / py_agg / py_scan, and venv activation.

## Install

```text
/plugin marketplace add alitrack/duckdb-python
/plugin install duckdb-python-udf@duckdb-python-udf
```

## Skills

| Skill | Covers |
|---|---|
| `duckdb-python-udf` | py_register direct-call pattern, row/batch/aggregate/table surfaces, gotchas |

```sql
INSTALL python_udf FROM community; LOAD python_udf;   -- DuckDB 1.5.4+
```

MIT, alitrack.
