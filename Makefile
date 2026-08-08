RUSTYBENCH ?= cargo run --quiet --manifest-path ../rustybench/Cargo.toml --

.PHONY: bench lint

bench:
	@$(RUSTYBENCH) baseline --root "$(CURDIR)" --baseline "$(CURDIR)/benches/baseline.json" -- cargo bench --bench bench

lint:
	cargo fmt --all
	cargo clippy --fix --allow-dirty --all-targets --all-features -- --deny warnings
