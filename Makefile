test-all: cache-template
	cargo test --all --release -- --test-threads=1

test-lib: cache-template
	find .fastembed_cache -name '*.lock' -delete || true
	cargo test --lib -- --test-threads=1

test-integration: cache-template
	cargo test --test vector_db_integration -- --test-threads=1

clean-cache:
	rm -rf .fastembed_cache

fix:
	cargo fix --allow-dirty --allow-staged

build-index:
	run --bin build_index -- --index-name=test_index

cache-template:
	@if [ ! -d .cache/mc-template/.git ]; then \
		git clone --depth 1 https://github.com/metacontract/template.git .cache/mc-template; \
	else \
		cd .cache/mc-template && git pull; \
	fi

test-setup:
	make cache-template
	cp -r .cache/mc-template ./test-tmp-template
