#!/usr/bin/env python3
"""
duckdb-python: comprehensive format r/w test.
Tests ORC write, Feather/IPC r/w, Arrow Stream r/w.
"""
import duckdb, os, shutil, sys, time, hashlib

DEMO = "/tmp/duckdb-python-fmt-test"
shutil.rmtree(DEMO, ignore_errors=True)
os.makedirs(DEMO)
sys.path.insert(0, DEMO)

con = duckdb.connect(config={'allow_unsigned_extensions': 'true'})
con.execute("LOAD 'build/release/extension/python_udf/python_udf.duckdb_extension'")

orc_ext = os.path.expanduser("~/.duckdb/extensions/v1.5.5/osx_arm64/orc.duckdb_extension")
has_orc_native = os.path.exists(orc_ext)
if has_orc_native:
    con.execute(f"LOAD '{orc_ext}'")

ROWS = 10000
con.execute(f"""
    CREATE TABLE src AS SELECT range id,
        CASE range%4 WHEN 0 THEN 'alpha' WHEN 1 THEN 'beta'
                     WHEN 2 THEN 'gamma' ELSE 'delta' END cat,
        (random()*10000)::DECIMAL(12,2) val,
        (random()*100)::DOUBLE score,
        DATE '2024-01-01' + (range%365)::INTEGER dt
    FROM range({ROWS})
""")
con.execute(f"COPY src TO '{DEMO}/src.csv'")
con.execute(f"COPY src TO '{DEMO}/src.parquet' (FORMAT PARQUET, COMPRESSION ZSTD)")

print("=" * 64)
print("  duckdb-python: format r/w test")
print("=" * 64)
print(f"  {ROWS:,} rows | CSV {os.path.getsize(f'{DEMO}/src.csv')/1024:.0f}KB | Parquet {os.path.getsize(f'{DEMO}/src.parquet')/1024:.0f}KB")

def md5(sql, cols=None):
    if cols is None:
        cols = ["id","cat","val","score","dt"]
    sel = ",".join(cols)
    ordr = ",".join(f"{c}" for c in cols)
    rows = con.execute(f"SELECT {sel} FROM ({sql}) ORDER BY {ordr}").fetchall()
    return hashlib.md5(str(rows).encode()).hexdigest()[:8]

ref = md5("FROM src")

# ── readers.py ──
with open(f"{DEMO}/readers.py", "w") as f:
    f.write('''
import os, pyarrow as pa
def _t(p):
    e = p.rsplit(".",1)[-1]
    if e == "orc":
        import pyarrow.orc; return pyarrow.orc.read_table(p)
    elif e == "arrow":
        return pa.ipc.open_stream(p).read_all()
    elif e in ("feather","ipc","parquet"):
        if e == "parquet":
            import pyarrow.parquet as pq; return pq.read_table(p)
        return pa.ipc.open_file(p).read_all()
    raise ValueError(e)
def _rows(t):
    return [[str(t.column(c)[i].as_py()) for c in t.column_names] for i in range(t.num_rows)]
def read_orc():
    return _rows(_t(os.environ["FMT_ORC_PATH"]))
def read_feather():
    return _rows(_t(os.environ["FMT_FEATHER_PATH"]))
def read_arrow_stream():
    return _rows(_t(os.environ["FMT_STREAM_PATH"]))
''')

os.environ["FMT_ORC_PATH"] = f"{DEMO}/src.parquet"
ref_pyscan = md5("FROM py_scan('readers', 'read_orc')", cols=[f"col{i}" for i in range(5)])
os.environ["FMT_ORC_PATH"] = f"{DEMO}/src.orc"

# ── Writers ──
PQ = f"{DEMO}/src.parquet"

con.execute("SELECT py_register('orc_w', ?, 1, '')", [(
    "(__import__('pyarrow.orc',fromlist=['orc']).write_table("
    f"__import__('pyarrow.parquet',fromlist=['parquet']).read_table('{PQ}'),"
    f"'{DEMO}/src.orc',compression='ZSTD',compression_strategy='SPEED')"
    " or str(__import__('os').path.getsize("
    f"'{DEMO}/src.orc')))"
)])

feather_code = (
    f"(lambda p,t: (w:=__import__('pyarrow').ipc.new_file(p+'/src.feather',t.schema),"
    "w.write_table(t),w.close(),"
    "str(__import__('os').path.getsize(p+'/src.feather')))[-1])"
    f"('{DEMO}',__import__('pyarrow.parquet',fromlist=['parquet']).read_table('{PQ}'))"
)
con.execute("SELECT py_register('feather_w', ?, 1, '')", [feather_code])

stream_code = (
    f"(lambda p,t: (w:=__import__('pyarrow').ipc.new_stream(p+'/src.arrow',t.schema),"
    "w.write_table(t),w.close(),"
    "str(__import__('os').path.getsize(p+'/src.arrow')))[-1])"
    f"('{DEMO}',__import__('pyarrow.parquet',fromlist=['parquet']).read_table('{PQ}'))"
)
con.execute("SELECT py_register('stream_w', ?, 1, '')", [stream_code])

# ── WRITE ──
print("\n── WRITE (Python UDF) ──")
results = {}
for name, func, ext in [
    ("ORC ZSTD", "orc_w", "orc"),
    ("Feather/IPC", "feather_w", "feather"),
    ("Arrow Stream", "stream_w", "arrow"),
]:
    path = f"{DEMO}/src.{ext}"
    t0 = time.time()
    r = con.execute(f"SELECT {func}('go')").fetchone()[0]
    elapsed = time.time() - t0
    ok = os.path.exists(path) and os.path.getsize(path) > 100
    sz = os.path.getsize(path) if ok else 0
    results[name] = {"w_ok": ok, "w_s": elapsed, "size": sz}
    print(f"  {'✅' if ok else '❌'} {name:18s} {sz/1024:7.1f} KB  {elapsed:.2f}s")

# ── READ ──
print("\n── READ ──")
os.environ["FMT_ORC_PATH"] = f"{DEMO}/src.orc"
os.environ["FMT_FEATHER_PATH"] = f"{DEMO}/src.feather"
os.environ["FMT_STREAM_PATH"] = f"{DEMO}/src.arrow"

if has_orc_native:
    t0 = time.time()
    m = md5(f"FROM read_orc('{DEMO}/src.orc')")
    results["ORC ZSTD"]["r_native"] = (m == ref)
    print(f"  {'✅' if m == ref else '❌'} ORC (native)     {ROWS:,} rows  md5={m}  {time.time()-t0:.2f}s")
else:
    results["ORC ZSTD"]["r_native"] = None

for name, func in [("ORC ZSTD", "read_orc"), ("Feather/IPC", "read_feather"), ("Arrow Stream", "read_arrow_stream")]:
    t0 = time.time()
    m = md5(f"FROM py_scan('readers', '{func}')", cols=[f"col{i}" for i in range(5)])
    ok = (m == ref_pyscan)
    results[name]["r_pyscan"] = ok
    print(f"  {'✅' if ok else '❌'} {name:18s} {ROWS:,} rows  md5={m}  {time.time()-t0:.2f}s")

# ── Sizes ──
print("\n── SIZE COMPARISON ──")
csv = os.path.getsize(f"{DEMO}/src.csv")
for name, ext in [("CSV", "csv"), ("Parquet ZSTD", "parquet"),
                   ("ORC ZSTD", "orc"), ("Feather/IPC", "feather"),
                   ("Arrow Stream", "arrow")]:
    path = f"{DEMO}/src.{ext}"
    if os.path.exists(path):
        sz = os.path.getsize(path)
        print(f"  {name:18s} {sz/1024:6.0f} KB  ({csv/sz:.1f}x vs CSV)")

# ── Summary ──
print("\n── SUMMARY ──")
all_ok = True
for name, r in results.items():
    w = "✅" if r["w_ok"] else "❌"
    nr = "✅" if r.get("r_native") else ("—" if r.get("r_native") is None else "❌")
    pr = "✅" if r.get("r_pyscan") else "❌"
    print(f"  {w} write | {nr} native | {pr} py_scan  — {name}")
    if not r["w_ok"] or not r.get("r_pyscan"):
        all_ok = False

print(f"\n  {'✅ ALL PASSED' if all_ok else '❌ SOME FAILED'}")
print(f"  files: {DEMO}/")
