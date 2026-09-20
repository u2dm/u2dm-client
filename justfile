MODES := "target/modes"
INTERPRETED := "--no-default-features --features interpreted,video"

default:
    @just --list

build *ARGS:
    cargo build {{ ARGS }}

run *ARGS:
    cargo run {{ ARGS }}

interpreted *ARGS:
    CARGO_TARGET_DIR={{ MODES }}/interpreted cargo build {{ INTERPRETED }} {{ ARGS }}

run-interpreted *ARGS:
    CARGO_TARGET_DIR={{ MODES }}/interpreted cargo run {{ INTERPRETED }} {{ ARGS }}

demo *ARGS:
    CARGO_TARGET_DIR={{ MODES }}/demo cargo run --features demo {{ ARGS }}

screenshot *ARGS:
    ./scripts/gen-screenshot.sh {{ ARGS }}

drive *ARGS:
    ./scripts/u2dm-drive {{ ARGS }}

scenario *ARGS:
    ./scripts/scenario {{ ARGS }}

scenarios:
    ./scripts/scenario list

clippy *ARGS:
    cargo clippy {{ ARGS }}

fmt:
    cargo +nightly fmt

test *ARGS:
    cargo test {{ ARGS }}

disk:
    @du -sh target/ 2>/dev/null || echo "target/ does not exist"
    @echo "default (cargo build/run, kept by clean-isolated):"
    @du -sh target/debug target/release 2>/dev/null | sort -rh || true
    @echo "isolated mode/feature artifacts (dropped by clean-isolated):"
    @du -sh target/inspect {{ MODES }}/* 2>/dev/null | sort -rh || true
    @echo "incremental dirs (one per configuration ever built):"
    @find target -name incremental -type d -exec sh -c 'printf "  %3s  %s\n" "$(ls "$1" | wc -l)" "$1"' _ {} \; 2>/dev/null || true

clean-modes:
    rm -rf {{ MODES }}

clean-inspect:
    rm -rf target/inspect

clean-scenarios:
    rm -rf target/scenarios

clean-isolated: clean-modes clean-inspect clean-scenarios

clean-all:
    cargo clean
