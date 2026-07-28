#!/usr/bin/env python3
"""
duckdb-python: ORC compression demo.
Shows Python UDF write + native ORC extension read.
"""
import duckdb, os, shutil, time

DEMO = "/tmp/duckdb-python-orc-demo"
shutil.rmtree(DEMO, ignore_errors=True)
os.makedirs(DEMO)

con = duckdb.connect(config={'allow_unsigned_extensions': 'true'})
con.execute("LOAD 'build/release/extension/python_udf/python_udf.duckdb_extension'")
orc_ext = os.path.expanduser("~/.duckdb/extensions/v1.5.5/osx_arm64/orc.duckdb_extension")
if os.path.exists(orc_ext):
    con.execute(f"LOAD '{orc_ext}'")

print("=" * 58)
print("  ORC: Python UDF write + native extension read")
print("=" * 58)

print("\n── py_eval ──")
print("  ", con.execute("SELECT py_eval('x[::-1]', 'helloduckdb')").fetchone()[0])

print("\n── py_register: typed direct call ──")
con.execute("SELECT py_register('dbl', 'str(int(x)*2)', 1, 'BIGINT')")
r = con.execute("SELECT dbl('21')").fetchone()[0]
print("  dbl('21') ->", r, type(r).__name__)

print("\n── 1M-row table ──")
con.execute("""
    CREATE TABLE t AS SELECT range id,
           CASE range%4 WHEN 0 THEN 'elec' WHEN 1 THEN 'cloth'
                        WHEN 2 THEN 'food' ELSE 'books' END cat,
           (random()*1000)::DECIMAL(10,2) amt,
           DATE '2024-01-01' + (range%365)::INTEGER dt
    FROM range(1000000)
""")

con.execute(f"COPY t TO '{DEMO}/data.csv'")
con.execute(f"COPY t TO '{DEMO}/data.zst.parquet' (FORMAT PARQUET, COMPRESSION ZSTD)")

print("\n── Python UDF: write ORC via pyarrow ──")
code = (
    "(lambda p: "
    "__import__('pyarrow.orc',fromlist=['orc']).write_table("
    "__import__('pyarrow.parquet',fromlist=['parquet']).read_table(p+'/data.zst.parquet'),"
    "p+'/data.zst.orc',compression='ZSTD',compression_strategy='SPEED')"
    " or str(__import__('os').path.getsize(p+'/data.zst.orc'))"
    f")('{DEMO}')"
)
con.execute("SELECT py_register('orc_write', ?, 1, '')", [code])
t0 = time.time()
sz = int(con.execute("SELECT orc_write('go')").fetchone()[0])
print(f"  wrote: {sz:,} bytes  ({time.time()-t0:.1f}s)")

if os.path.exists(orc_ext):
    print("\n── Native ORC read ──")
    n = con.execute(f"SELECT count(*) FROM read_orc('{DEMO}/data.zst.orc')").fetchone()[0]
    print(f"  read_orc(): {n:,} rows  ✓")
    con.sql(f"FROM read_orc('{DEMO}/data.zst.orc') LIMIT 3").show()

print("\n── Size comparison ──")
csv = os.path.getsize(f"{DEMO}/data.csv")
pq  = os.path.getsize(f"{DEMO}/data.zst.parquet")
orc = os.path.getsize(f"{DEMO}/data.zst.orc")
for n, s in [("CSV", csv), ("Parquet ZSTD", pq), ("ORC ZSTD", orc)]:
    print(f"  {n:15s} {s/1e6:6.1f} MB  {'|' * int(s / csv * 40)}")
print(f"\n  CSV → ORC ZSTD:  {csv/orc:.1f}x  (Python UDF write)")

print(f"\n✅ Done.  {DEMO}/")
