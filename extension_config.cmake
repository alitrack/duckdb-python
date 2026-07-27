# Required by DuckDB extension-ci-tools even for pure-Rust extensions.
# See https://github.com/duckdb/extension-ci-tools

duckdb_extension_load(python
	LOAD_TESTS
	GIT_URL https://github.com/alitrack/duckdb-python
	GIT_TAG master
)
