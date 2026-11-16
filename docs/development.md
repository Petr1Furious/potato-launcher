# Development

## Configuration

Launcher name, catalog URLs, the update source and other parameters are baked in at **compile time** by `crates/build-config`. The build reads `build.env` from the repository root automatically (non-empty environment variables take priority). If you change config values, rebuild the launcher.

If you are not using `build.env`, export the variables yourself before building. See the [Launcher configuration](/setting-up/launcher#variables-list) page for the full list.

## Building

The launcher and `potato` are regular Rust binaries. Install rustup from [rustup.rs](https://rustup.rs), then:

- `cargo run --bin launcher` runs the launcher in debug mode
- `cargo build --bin launcher --release` builds a release binary
- `cargo run -p potato -- --help` runs the pack CLI

Checks CI runs:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

`cargo test -p potato -- --ignored` runs the online tests: they build real
instances and talk to Modrinth and Mojang.

## Testing end to end

`docker-compose-dev.yml` serves `./dev-site` on port 8000 with the same nginx
config as production:

```bash
docker compose -f docker-compose-dev.yml up -d
```

In a pack repo, set `[publish] target = "file:///<repo>/dev-site/data"` and
`base_url = "http://localhost:8000/data"`, then `potato publish`. Build the
launcher with `INSTANCE_MANIFEST_URLS=http://localhost:8000/data/instance_manifest.json`.
