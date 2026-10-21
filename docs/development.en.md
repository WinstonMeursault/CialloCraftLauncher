# Development guide

## Environment

Install the stable Rust toolchain and Cargo. The workspace uses Rust 2024 edition. Target platforms are Windows x86_64, macOS arm64, and Linux x86_64.

## Build and run

Run from the repository root:

```sh
cargo build --workspace
cargo run -p ccl-cli -- --help
```

The CLI provides the basic `doctor` and `init` commands; the full installation workflow is not available yet.

## Code boundaries

- Put reusable domain logic in `crates/ccl-core`.
- `crates/ccl-core` must not depend on terminal or graphical-interface libraries.
- The CLI parses arguments, formats output, and maps exit codes; it does not duplicate core business rules.
- Tasks report progress through core events and respond to cancellation through the cancellation handle.
- Configuration errors should include understandable context; secrets must not be written to logs or output.

## Configuration and data directories

See the [architecture guide](architecture.en.md#download-sources) for default configuration paths. Configuration JSON fields are `downloads.version_list_source`, `downloads.file_source`, `downloads.mirror_root`, `downloads.http_proxy`, and `downloads.max_parallel`. Missing fields use defaults; an explicitly selected but missing configuration file is an error.

Default game directory priority is: explicit `--game-dir`, an existing `.minecraft` in the current working directory, the platform-conventional directory, `.minecraft` in the home directory, and finally a new `.minecraft` in the current working directory. An existing conventional macOS directory can be reused. `doctor` is read-only; `init` creates the selected directory.

The version-list and file-source preferences are independent and accept `auto`, `official`, or `mirror`. The mirror root is configurable and must be HTTPS without URL credentials; actual downloads and source selection are not implemented yet.

## Current feature scope

The core provides configuration, directory selection, an HTTP client, and task event/cancellation foundations. Game installation, file downloads, version lookup, and authentication are not implemented yet. Keep the core independent from interfaces and update the relevant user documentation when adding features.
