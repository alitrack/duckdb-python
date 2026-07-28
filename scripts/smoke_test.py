#!/usr/bin/env python3
import duckdb

con = duckdb.connect(config={"allow_unsigned_extensions": "true"})
con.execute("LOAD 'build/release/extension/python_udf/python_udf.duckdb_extension'")

result = con.execute("SELECT py_eval('x.upper()', 'hello')").fetchall()
print(result)
assert result[0][0] == "HELLO", f"Expected HELLO, got {result}"

result = con.execute("SELECT py_register('double_it', 'str(int(x)*2)', 1)").fetchall()
print(result)

result = con.execute("SELECT py_call('double_it', '21')").fetchall()
print(result)
assert result[0][0] == "42", f"Expected 42, got {result}"

print("All smoke tests passed!")
