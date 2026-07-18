SHELL := /bin/sh
.PHONY: all build debug release test golden clean install help

all: release

help:
	@printf '%s\n' \
	  'targets:' \
	  '  make release   - cargo build --release  (default; produces target/release/libstryke_terminal.{dylib,so})' \
	  '  make debug     - cargo build' \
	  '  make test      - cargo test then `s test t/`' \
	  '  make golden    - regenerate tests/golden.json from the reference pyte (needs Python + pyte)' \
	  '  make install   - `s pkg install -g .` (copies source + cdylib into ~/.stryke/store/terminal@<ver>/)' \
	  '  make clean     - cargo clean'

release:
	cargo build --release

debug build:
	cargo build

test:
	cargo test
	s test t/ || true

# Regenerate the golden fixtures from the installed reference emulator. Only run
# when intentionally re-baselining against a new pyte version; the committed
# tests/golden.json is what CI checks against.
golden:
	python3 tests/gen_golden.py

install: release
	s pkg install -g .

clean:
	cargo clean
