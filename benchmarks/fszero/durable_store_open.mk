.PHONY: durable-store-open

durable-store-open:
	./scripts/profile_build.sh --cargo-command bench --bench perf_harness -- store_open_benchmark "$${CARGO_TARGET_DIR:-target}/durable-store-open"
