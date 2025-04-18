test-all:
	ulimit -n 4096 && cargo test --all --release

test-lib:
	cargo test --lib -- --test-threads=1

test-integration:
	ulimit -n 4096 && cargo test --test vector_db_integration -- --test-threads=1

clean-cache:
	rm -rf .fastembed_cache

fix:
	cargo fix --allow-dirty --allow-staged

build-index:
	run --bin build_index -- --index-name=test_index
