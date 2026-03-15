# ─────────────────────────────────────────────────────────────────────────────
# OpenSOVD-native-server — CI Quality Gates
#
# Usage:
#   make check          — full quality gate (lint + test + build)
#   make lint           — clippy pedantic (workspace lints from Cargo.toml)
#   make test           — run all 227+ tests, assert zero failures
#   make coverage       — generate test coverage report (requires cargo-llvm-cov)
#   make audit          — dependency vulnerability scan (requires cargo-audit)
#   make build-release  — release build (standard + gateway-only)
#   make ci             — full CI pipeline (all of the above)
# ─────────────────────────────────────────────────────────────────────────────

.PHONY: check lint test coverage audit build-release ci clean

# ── Configurable thresholds ─────────────────────────────────────────────────
MIN_TESTS      ?= 227
MIN_COVERAGE   ?= 60

# ── Lint (clippy pedantic — enforced via [workspace.lints] in Cargo.toml) ──
lint:
	@echo "══ Clippy pedantic (workspace lints — lib + bin) ══"
	cargo clippy --workspace -- -D warnings
	@echo "✓ Clippy pedantic clean"

# ── Tests ───────────────────────────────────────────────────────────────────
test:
	@echo "══ Running workspace tests ══"
	@cargo test --workspace 2>&1 | tee /tmp/opensovd-test-output.txt
	@TOTAL=$$(grep -c '^test .* ok$$' /tmp/opensovd-test-output.txt 2>/dev/null || echo 0); \
	PASSED=$$(grep 'test result' /tmp/opensovd-test-output.txt | awk '{sum += $$4} END {print sum+0}'); \
	FAILED=$$(grep 'test result' /tmp/opensovd-test-output.txt | awk '{sum += $$6} END {print sum+0}'); \
	echo "Tests passed: $$PASSED, failed: $$FAILED"; \
	if [ "$$FAILED" -gt 0 ]; then echo "✗ FAILED: $$FAILED test(s) failed" && exit 1; fi; \
	if [ "$$PASSED" -lt $(MIN_TESTS) ]; then echo "✗ FAILED: only $$PASSED tests (minimum: $(MIN_TESTS))" && exit 1; fi; \
	echo "✓ $$PASSED tests passed (minimum: $(MIN_TESTS))"

# ── Test coverage (requires: cargo install cargo-llvm-cov) ──────────────────
coverage:
	@echo "══ Test coverage ══"
	@command -v cargo-llvm-cov >/dev/null 2>&1 || { echo "Install: cargo install cargo-llvm-cov"; exit 1; }
	cargo llvm-cov --workspace --fail-under-lines $(MIN_COVERAGE) --hide-instantiations
	@echo "✓ Coverage ≥ $(MIN_COVERAGE)%"

# ── Dependency audit (requires: cargo install cargo-audit) ──────────────────
audit:
	@echo "══ Dependency vulnerability scan ══"
	@command -v cargo-audit >/dev/null 2>&1 || { echo "Install: cargo install cargo-audit"; exit 1; }
	cargo audit
	@echo "✓ No known vulnerabilities"

# ── Release builds ──────────────────────────────────────────────────────────
build-release:
	@echo "══ Release build (standard) ══"
	cargo build --workspace --release
	@echo "══ Release build (gateway-only, no local-uds) ══"
	cargo build -p opensovd-native-server --release --no-default-features
	@echo "✓ Both release builds succeeded"

# ── Combined quality gate ───────────────────────────────────────────────────
check: lint test

# ── Full CI pipeline ────────────────────────────────────────────────────────
ci: lint test coverage audit build-release
	@echo ""
	@echo "════════════════════════════════════════"
	@echo "  ✓ All CI quality gates passed"
	@echo "════════════════════════════════════════"

clean:
	cargo clean
