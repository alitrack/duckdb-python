#!/bin/bash
# pyudf/duckdb-python verification smoke test
# Usage: bash scripts/verify.sh

set -e
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJ_DIR="$(dirname "$SCRIPT_DIR")"
cd "$PROJ_DIR"

RED='\033[0;31m'
GREEN='\033[0;32m'
NC='\033[0m'

pass() { echo -e "${GREEN}[PASS]${NC} $1"; }
fail() { echo -e "${RED}[FAIL]${NC} $1"; exit 1; }

echo "=== pyudf/duckdb-python verification ==="
echo ""

# 1. cargo check
echo "[1/4] cargo check --lib ..."
if cargo check --lib 2>&1 | tail -1 | grep -q "Finished"; then
    pass "cargo check passes (zero errors)"
else
    fail "cargo check failed"
fi

# 2. Verify no format!() injection vectors remain in production code
echo ""
echo "[2/4] security: scan for code injection patterns ..."
INJECTIONS=0
# map.rs: should NOT have format with {} + expr together
grep -n 'format!(".*{}.*for x in __input__' crates/pyudf/src/python/map.rs 2>/dev/null && INJECTIONS=1 || true
# table.rs: should NOT have format with __import__
grep -n "^.*format.*__import__" crates/pyudf/src/python/table.rs 2>/dev/null && INJECTIONS=1 || true
# venv.rs: should NOT eval os.path.isdir
grep -n "os.*path.*isdir" crates/pyudf/src/python/venv.rs 2>/dev/null && INJECTIONS=1 || true
if [ $INJECTIONS -eq 0 ]; then
    pass "no code injection patterns found"
else
    fail "code injection patterns still present!"
fi

# 3. Verify AtomicPtr ordering is Release/Acquire (not Relaxed)
echo ""
echo "[3/4] correctness: verify atomic ordering ..."
if grep -q "Ordering::Release" crates/pyudf/src/lib.rs && grep -q "Ordering::Acquire" crates/pyudf/src/python/registry.rs; then
    pass "atomic ordering: Release/Acquire"
else
    fail "atomic ordering still uses Relaxed"
fi

# 4. Verify init errors are propagated (not silently swallowed)
echo ""
echo "[4/4] correctness: verify init error propagation ..."
SILENCED=$(grep -c "let _ = ensure_initialized()" crates/pyudf/src/python/scalar.rs 2>/dev/null || true)
SILENCED=$(echo "$SILENCED" | tr -d '\n\r')
SILENCED2=$(grep -c "^fn ensure_python" crates/pyudf/src/python/aggregate.rs 2>/dev/null || true)
SILENCED2=$(echo "$SILENCED2" | tr -d '\n\r')
SILENCED3=$(grep -c "^fn ensure_python" crates/pyudf/src/python/map.rs 2>/dev/null || true)
SILENCED3=$(echo "$SILENCED3" | tr -d '\n\r')
SILENCED4=$(grep -c "^fn ensure_python" crates/pyudf/src/python/table.rs 2>/dev/null || true)
SILENCED4=$(echo "$SILENCED4" | tr -d '\n\r')
TOTAL=$((SILENCED + SILENCED2 + SILENCED3 + SILENCED4))
if [ "$TOTAL" -eq 0 ]; then
    pass "no silenced init errors ($TOTAL found)"
else
    fail "$TOTAL silenced init errors still present!"
fi

echo ""
echo "=== All checks passed ==="
echo ""
echo "Next steps:"
echo "  make configure    # install Python dev headers"
echo "  make release      # build python.duckdb_extension"
echo "  make test_release # run SQLLogicTests"
echo "  make smoke-test   # end-to-end Python UDF test"
