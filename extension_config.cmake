# Required by DuckDB extension-ci-tools even for pure-Rust extensions.
# See https://github.com/duckdb/extension-ci-tools

duckdb_extension_load(pyudf
	LOAD_TESTS
	GIT_URL https://github.com/alitrack/pyudf
	GIT_TAG master
)
