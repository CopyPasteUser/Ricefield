# Contributing

Issues and pull requests are welcome. For behavior changes, include a reproducible scenario and tests where practical. For a theme, keep the filesystem logic untouched and document which RiceBowl profile it belongs to.

Before submitting a PR, run `cargo fmt --all -- --check`, `cargo clippy --all-targets -- -D warnings`, and `cargo test --all-targets` on Linux. Never attach private filesystem data to an issue.
