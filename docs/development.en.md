# Development guide

## Environment

Install the stable Rust toolchain and Cargo. The workspace uses Rust 2024 edition. Target platforms are Windows x86_64, macOS arm64, and Linux x86_64.

## Build and run

Run from the repository root:

```sh
cargo build --workspace
cargo run -p ccl-cli -- --help
```

The CLI provides `doctor`, `init`, `settings`, `account`, `versions`, `install`, `instances`, `loaders`, `instance`, `pack`, `content`, `online`, `launch`, and `logs`. Run `cargo run -p ccl-cli -- --help` for the current arguments. See [instance lifecycle](instances.en.md), [archive imports](imports.en.md), [networking and offline use](networking.en.md), [online content](online-content.en.md), [loaders and instances](loaders.en.md), and [Modrinth pack and content](modpacks.en.md) for user workflows and limits. Launch supports Microsoft or offline identity, compatible Java runtime discovery and managed installation, dry-run planning, process log capture, and cancellation. Microsoft account authorization and live end-to-end validation remain outstanding; see the [account registration guide](accounts.en.md).

## Code boundaries

For loader discovery and isolated instance workflows, see [modded instances and loaders](loaders.en.md). `loaders`, `instance`, and `launch --instance` reuse the same core services as a future GUI. `modpack_format` owns Modrinth archive parsing, validation, and writing; `import_formats` detects CurseForge and MultiMC/Prism archives and converts supported inputs through the transactional pack pipeline; `modpacks` coordinates import/export and ownership receipts; `content` manages local files and userdata backups. Online provider integrations live in `online_content`. Pack imports and content edits use the same staged instance transaction and cross-process lock as instance changes. Modern Forge/NeoForge processors are launched by core with output delivered through task events; they must not write directly to CLI stdout. NeoForge SHA-512 fallback verifies installer bytes before the install pipeline derives its SHA-1 value.

- Put reusable domain logic in `crates/ccl-core`.
- `crates/ccl-core` must not depend on terminal or graphical-interface libraries.
- The CLI parses arguments, formats output, and maps exit codes; it does not duplicate core business rules.
- Tasks report progress through core events and respond to cancellation through the cancellation handle.
- Configuration errors should include understandable context; secrets must not be written to logs or output. Settings changes go through the core's lock-serialized `ConfigStore::update` transaction and validated atomic save. Keep OS credential storage mandatory for account tokens; Linux requires an available persistent Secret Service/D-Bus session, with no plaintext fallback.

## Configuration and data directories

See the [architecture guide](architecture.en.md#download-sources) for default configuration paths. Configuration JSON fields cover `downloads.*`, `auth.microsoft_client_id`, and `launch.java`, `launch.max_memory_mb`, and `launch.auto_install_java`. Missing fields use defaults so older files remain readable; an explicitly selected but missing configuration file is an error. The client ID is public configuration. Proxy credentials are retained for network use but redacted in settings output.

Default game directory priority is: explicit `--game-dir`, an existing `.minecraft` in the current working directory, the platform-conventional directory, `.minecraft` in the home directory, and finally a new `.minecraft` in the current working directory. An existing conventional macOS directory can be reused. `doctor` is read-only; `init` creates the selected directory.

The version-list and file-source preferences are independent and accept `auto`, `official`, or `mirror`. The mirror root is configurable and must be HTTPS without URL credentials. The global `--network-offline` option disables HTTP and uses only verified local metadata and artifacts; it is separate from offline player identity. Interrupted transfers resume only when the saved strong ETag and server range response validate the partial file. Version listing, installation, launch preparation, and process capture are implemented in the shared core; keep CLI-specific formatting and exit-code mapping in the CLI.

## Current feature scope

The core provides configuration, directory selection, HTTP downloads, version lookup and installation, managed Java runtime installation under `.ccl/runtimes`, Microsoft account authentication, launch planning, native extraction, child-process output capture, Modrinth pack handling, CurseForge and MultiMC/Prism archive conversion, Modrinth/CurseForge online content operations, isolated instance lifecycle and retained revision cleanup, Fabric/Quilt/Legacy Fabric/Forge/NeoForge/OptiFine loaders, local content management, and userdata backups. Pack receipts record hashes for pack-owned paths; updates use those hashes to detect local edits, deletions, and collisions before replacing data. Disk-usage reporting counts logical bytes and does not promise physical space reclaimed when hard links exist. Userdata backups are separate from Modrinth exports, which omit saves and private server lists and require explicit redistribution consent for local binaries. CurseForge API key support does not imply that a real key has production authorization. Microsoft account approval and live end-to-end validation remain outstanding; don't describe account login as ready for public use until those gates are complete. Keep the core independent from interfaces and update the English and Chinese user documentation together when changing public behavior.
