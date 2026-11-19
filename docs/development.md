# Development

## Configuration

Launcher name, catalog URLs, the update source and other parameters are baked in at **compile time** by `crates/build-config`. The build reads `build.env` from the repository root automatically (non-empty environment variables take priority). If you change config values, rebuild the launcher.

If you are not using `build.env`, export the variables yourself before building. See the [Launcher configuration](/setting-up/launcher#variables-list) page for the full list.

## Building

The launcher and `potato` can be built like any other Rust project.

If you aren't familiar with Rust tooling, start by installing rustup from [rustup.rs](https://rustup.rs). Then, use `cargo run --bin launcher` to build and run the launcher in debug configuration, or `cargo build --bin launcher --release` to create a release binary. To run `potato`, use `cargo run -p potato -- --help`.
