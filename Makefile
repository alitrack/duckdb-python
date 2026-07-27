.PHONY: all configure debug release test test_debug test_release unit-test smoke-test clean clean_all

PROJ_DIR := $(dir $(abspath $(lastword $(MAKEFILE_LIST))))

EXTENSION_NAME=python

# pyudf uses quack-rs modules gated behind DuckDB 1.5 APIs (e.g. config options),
# so package binaries for a concrete DuckDB 1.5 release.
USE_UNSTABLE_C_API=1
TARGET_DUCKDB_VERSION=v1.5.4
DUCKDB_TEST_VERSION=1.5.4
EXTENSION_VERSION=0.1.0

# Prefer a shared-library Python 3.12 on macOS so PyO3 links cleanly.
# Override in the environment if your Python lives elsewhere.
PYO3_PYTHON ?= $(shell if [ -x /opt/homebrew/bin/python3.12 ]; then echo /opt/homebrew/bin/python3.12; elif [ -x /usr/local/bin/python3.12 ]; then echo /usr/local/bin/python3.12; else command -v python3; fi)
PYTHON_BIN ?= $(PYO3_PYTHON)
export PYO3_PYTHON

all: configure debug

include extension-ci-tools/makefiles/c_api_extensions/base.Makefile
include extension-ci-tools/makefiles/c_api_extensions/rust.Makefile

# Override: inject --features embed when LINUX_CI_IN_DOCKER=1.
# Expected harmless warning: "overriding recipe for target" — we intentionally
# extend the base rust.Makefile targets to support runtime dlopen builds.
CARGO_EMBED_FLAG =
ifeq ($(LINUX_CI_IN_DOCKER),1)
	CARGO_EMBED_FLAG = --features embed
endif

build_extension_library_debug: check_configure
	DUCKDB_EXTENSION_NAME=$(EXTENSION_NAME) DUCKDB_EXTENSION_MIN_DUCKDB_VERSION=$(TARGET_DUCKDB_VERSION) cargo build $(CARGO_OVERRIDE_DUCKDB_RS_FLAG) $(CARGO_EMBED_FLAG) $(TARGET_INFO)
	$(PYTHON_VENV_BIN) -c "from pathlib import Path;Path('$(EXTENSION_BUILD_PATH)/debug/extension/$(EXTENSION_NAME)').mkdir(parents=True, exist_ok=True)"
	$(PYTHON_VENV_BIN) -c "import shutil;shutil.copyfile('$(TARGET_PATH)/debug$(IS_EXAMPLE)/$(RUST_LIBNAME)', '$(EXTENSION_BUILD_PATH)/debug/$(EXTENSION_LIB_FILENAME)')"

build_extension_library_release: check_configure
	DUCKDB_EXTENSION_NAME=$(EXTENSION_NAME) DUCKDB_EXTENSION_MIN_DUCKDB_VERSION=$(TARGET_DUCKDB_VERSION) cargo build $(CARGO_OVERRIDE_DUCKDB_RS_FLAG) --release $(CARGO_EMBED_FLAG) $(TARGET_INFO)
	$(PYTHON_VENV_BIN) -c "from pathlib import Path;Path('$(EXTENSION_BUILD_PATH)/release/extension/$(EXTENSION_NAME)').mkdir(parents=True, exist_ok=True)"
	$(PYTHON_VENV_BIN) -c "import shutil;shutil.copyfile('$(TARGET_PATH)/release$(IS_EXAMPLE)/$(RUST_LIBNAME)', '$(EXTENSION_BUILD_PATH)/release/$(EXTENSION_LIB_FILENAME)')"

ensure_python_dev:
	@if [ "$$(uname -s)" = "Linux" ]; then \
		if ! dpkg -l python3-dev 2>/dev/null | grep -q '^ii'; then \
			SUDO=""; \
			if [ "$$(id -u)" -ne 0 ] && command -v sudo >/dev/null 2>&1; then SUDO="sudo"; fi; \
			if command -v apt-get >/dev/null 2>&1; then \
				$$SUDO apt-get update -y -qq && $$SUDO DEBIAN_FRONTEND=noninteractive apt-get install -y -qq python3-dev python3-venv python3.12-venv; \
			elif command -v dnf >/dev/null 2>&1; then \
				$$SUDO dnf install -y python3-devel python3; \
			elif command -v yum >/dev/null 2>&1; then \
				$$SUDO yum install -y python3-devel python3; \
			elif command -v apk >/dev/null 2>&1; then \
				$$SUDO apk add --no-cache python3 python3-dev py3-pip; \
			fi; \
		fi; \
	fi

# Use uv for faster venv creation if available (falls back to python -m venv).
# Override the base venv target from extension-ci-tools.
uv_venv: configure/venv
configure/venv:
	@if command -v uv >/dev/null 2>&1; then \
		uv venv configure/venv --python $(PYTHON_BIN); \
		cd configure && uv pip install --python venv/bin/python3 duckdb==$(DUCKDB_TEST_VERSION); \
	else \
		$(PYTHON_BIN) -m venv configure/venv; \
	fi

debug: ensure_python_dev build_extension_library_debug build_extension_with_metadata_debug
release: ensure_python_dev build_extension_library_release build_extension_with_metadata_release

test: test_release
test_debug: test_extension_debug
test_release: test_extension_release

unit-test:
	cargo test --lib

smoke-test: release
	$(PYTHON_VENV_BIN) -c "import duckdb; con = duckdb.connect(config={'allow_unsigned_extensions': 'true'}); con.execute(\"LOAD 'build/release/extension/python/python.duckdb_extension'\"); print(con.execute(\"SELECT py_eval('x.upper()', 'hello')\").fetchall()); print(con.execute(\"SELECT py_register('double_it', 'str(int(x)*2)', 1)\").fetchall()); print(con.execute(\"SELECT py_call('double_it', '21')\").fetchall())"

clean: clean_build clean_rust
clean_all: clean_configure clean
