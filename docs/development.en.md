# Development guide

[User guide](user-guide.en.md) · [API reference](api.en.md) · [简体中文](development.zh-CN.md)

- [Environment](#environment)
- [Build and run](#build)
- [Architecture and workspace](#architecture)
- [Code boundaries](#boundaries)
- [Configuration and data directories](#data)
- [Instances and transactions](#transactions)
- [Updates, compatibility and diagnostics](#maintenance)
- [Microsoft application registration and acceptance](#microsoft-application)
- [Validation and preview packages](#validation)
- [Current feature scope](#scope)

<a id="environment"></a>

## Environment

Install the stable Rust toolchain and Cargo. The workspace uses Rust 2024 edition. Target platforms are Windows x86_64, macOS arm64, and Linux x86_64.

<a id="build"></a>

## Build and run

Run from the repository root:

```sh
cargo build --workspace
cargo run -p ccl-cli -- --help
```

User workflows are in the [user guide](user-guide.en.md); commands, output and core services are in the [API reference](api.en.md).

<a id="architecture"></a>

## Architecture and workspace

### Goals

CialloCraftLauncher starts with a command-line interface. The reusable Rust core provides configuration, directory, network, download, and installation logic; the CLI and a future graphical interface call the same core services. The core does not depend on a terminal or graphical interface, so adding a GUI later will not require moving business logic into the interface layer.

Initial target platforms:

- Windows x86_64
- macOS arm64
- Linux x86_64

### Workspace

- `crates/ccl-core`: shared services, configuration, errors, HTTP transport, task events, installation, Java discovery, and launch orchestration.
- `crates/ccl-cli`: command-line arguments, text or JSON output, exit codes, and calls into the core.
- `docs`: public documentation for users and contributors.

The core reports progress through task contexts and provides a cooperative cancellation handle. An interface displays events and collects input; the core performs work, validates configuration, and returns structured errors. The core must not depend on an interface.

<a id="boundaries"></a>

## Code boundaries

`loaders`, `instance`, and `launch --instance` reuse the same core services as a future GUI. `modpack_format` owns Modrinth archive parsing, validation, and writing; `import_formats` detects CurseForge and MultiMC/Prism archives and converts supported inputs through the transactional pack pipeline; `modpacks` coordinates import/export and ownership receipts; `content` manages local files and userdata backups. Online provider integrations live in `online_content`. Pack imports and content edits use the same staged instance transaction and cross-process lock as instance changes. Modern Forge/NeoForge processors are launched by core with output delivered through task events; they must not write directly to CLI stdout. NeoForge SHA-512 fallback verifies installer bytes before the install pipeline derives its SHA-1 value.

- Put reusable domain logic in `crates/ccl-core`.
- `crates/ccl-core` must not depend on terminal or graphical-interface libraries.
- The CLI parses arguments, formats output, and maps exit codes; it does not duplicate core business rules.
- Tasks report progress through core events and respond to cancellation through the cancellation handle.
- Configuration errors should include understandable context; secrets must not be written to logs or output. Settings changes go through the core's lock-serialized `ConfigStore::update` transaction and validated atomic save. Keep OS credential storage mandatory for account tokens; Linux requires an available persistent Secret Service/D-Bus session, with no plaintext fallback.

<a id="data"></a>

## Configuration and data directories

Configuration keys and default paths are maintained in the [user guide](user-guide.en.md#configuration).

Default game directory priority is: explicit `--game-dir`, an existing `.minecraft` in the current working directory, the platform-conventional directory, `.minecraft` in the home directory, and finally a new `.minecraft` in the current working directory. An existing conventional macOS directory can be reused. `doctor` is read-only; `init` creates the selected directory.

The version-list and file-source preferences are independent and accept `auto`, `official`, or `mirror`. The mirror root is configurable and must be HTTPS without URL credentials. The global `--network-offline` option disables HTTP and uses only verified local metadata and artifacts; it is separate from offline player identity. Interrupted transfers resume only when the saved strong ETag and server range response validate the partial file. Version listing, installation, launch preparation, and process capture are implemented in the shared core; keep CLI-specific formatting and exit-code mapping in the CLI.

Version lists and game files have separate source settings, each supporting `auto`, `official`, and `mirror`. Both default to `auto`; the mirror root can be set in the user configuration. The current version listing and installation workflows use the official manifest and version metadata. Installation downloads the client, libraries, native libraries, asset index, and assets. It validates files against available upstream hashes and sizes, bounds concurrent downloads, retries transient failures, resumes transfers only after ETag and range checks, reuses valid cached files, and writes through temporary files before replacement. A cross-process version lock prevents competing installations from writing the same version simultaneously. Tasks support cooperative cancellation. Existing version JSON that conflicts with the requested metadata is reported as a conflict instead of being replaced. NeoForge installer verification can use its official SHA-512 checksum when the SHA-1 sidecar is unavailable; the pipeline derives SHA-1 only after verifying those bytes.

<a id="transactions"></a>

## Instances and transactions

The `loaders` service discovers compatible Fabric, Quilt, Legacy Fabric, Forge, NeoForge, and OptiFine releases and installs supported profiles. The `forge` engine interprets modern installer client processors, reports output through task events, and checks declared output hashes. Forge supports processor based and supported legacy metadata formats; NeoForge uses processor based installers. Inherited profiles retain the vanilla client archive's owning directory; child library versions override matching parent artifacts while keeping distinct native classifiers. Provider compatibility metadata is a preflight check, not proof that all third-party mod combinations launch successfully. See [loaders and instances](user-guide.en.md#loaders).

The `instances` service owns `<game root>/.ccl/instances/<id>/game`. Installation and update build a separate staging revision and commit only after success. A journal supports recovery from an interrupted revision switch; the previous successful revision remains available for rollback. Loader updates and loader removal preserve current user data. Pack imports/updates and content edits use the same staged-generation transaction and cross-process instance lock as launches, so a running game cannot be replaced underneath a mutation.

Instance lifecycle operations can rename, copy, delete, list and restore retained revisions, report logical disk usage, and preview or apply cleanup of archived revisions and unfinished stages. Cleanup keeps active worlds/content out of scope by design; hard links mean reported logical bytes are not physical disk usage or guaranteed reclaimed space. See [instance lifecycle](user-guide.en.md#instances).

Archive import recognizes Modrinth, CurseForge and MultiMC/Prism exports. CurseForge remote file resolution uses the official API and requires a user supplied API key; fixture tests for request scoping do not establish live API authorization. MultiMC/Prism import accepts supported vanilla/loader components and local game content, and rejects unsupported custom patches. These conversions do not execute archive hooks or import account databases. See [archive imports](user-guide.en.md#imports).

The online content service supports Modrinth and CurseForge search, version lookup, dependency resolution, update checks, conflict analysis, and transactional install/update operations. CurseForge key availability or header-scoping fixtures are not evidence that a real key has production authorization. See [online content](user-guide.en.md#online).

The `modpack_format` module reads and writes Modrinth format version 1 archives, validates manifest fields and destination paths, limits ZIP expansion, and applies `overrides/` with client overrides layered on top. `modpacks` imports selected files and overrides transactionally and stores a receipt with hashes of pack-owned paths. Updates compare current files with those ownership hashes and stop on modified/deleted owned files or collisions with unowned local data unless the caller explicitly authorizes replacement. After a pack or content change, `instance rollback` restores the prior full instance snapshot, including userdata and saves; users should back up first when they need to retain newer saves. `content` manages local content and portable userdata backups. Backups include user data and saves but exclude game/loader files, Java runtimes, and account credentials. Modrinth exports are separate: they exclude saves and private server lists, and local binary files require explicit redistribution authorization. See [Modrinth packs and instance content](user-guide.en.md#packs) for the commands and safety details.

<a id="maintenance"></a>

## Updates, compatibility and diagnostics

`update::UpdateService` checks only `WinstonMeursault/CialloCraftLauncher`. Stable packages use a `v<VERSION>` tag on `main`. Every `dev` push starts validation and packaging on Windows x64, macOS arm64 and Linux x86_64; the matching commit is published as immutable `dev-<full-SHA>` only after all jobs pass. The release workflow keeps the ten newest developer releases. Stable tags must match the CLI crate version and point to a commit on `main`. Manifests bind repository, channel, branch, full commit, target, executable digest and data reader range (`1..=1`).

`UpdateService::check` distinguishes an available package from a branch head whose build is not published yet. `stage` downloads and validates the bounded ZIP, its paths and hashes, then stages the candidate. `Installation` owns channel preference and the persistent transaction journal; `launch_helper` starts the restricted old-image helper, and `execute_helper` completes replacement, health check or rollback after the parent exits. `prepare_rollback` checks current data compatibility before changing transaction state. The binary updater does not restore game data. There is no data migration in the current format; readers preserve schema-less config/account/instance metadata and accept receipt schema 1. Unknown future config or instance fields and receipt schemas above 1 are refused. The initial regression uses config and account metadata generated by a git archive of baseline `2edbf0e`; those fixtures are sanitized, and the fixture account secrets were never persisted.

Release archives are unsigned. HTTPS, size and SHA-256 checks detect corruption but do not authenticate the publisher. Do not describe these checks as signatures or verified publisher identity. OS signing/notarization is not implemented. Update `status`, `rollback` and `recover` do not need network access; update check/apply do, and `--network-offline` blocks them.

`compatibility::inspect_compatibility(config_path, game_root)` is read-only, does not access the OS credential vault, and only inspects CCL-owned metadata. A future migration must lock and back up affected metadata, stage and atomically commit, recover safely after interruption, and refuse rollback if the retained reader cannot consume the new data. Never roll back the launcher by restoring a whole game directory.

`DiagnosticsService::doctor` reports Java candidates, directory access and whether a credential adapter is compiled in; vault unlock state is not probed. Network and write probes are explicit. `diagnose_run` checks a bounded run log plus timestamp-correlated crash reports and classifies only supported patterns. Export serializes an allowlist and substitutes paths; raw output preview requires a separate opt-in, is limited to 16 KiB and applies best-effort redaction. It can still contain secrets written by the game or mods and is never uploaded by CCL.

P7 is undergoing three-platform release acceptance. The performance workflow uses the real installation service and fixed local HTTP samples for cold/warm installation, verification/cache reuse, CLI startup/ZIP parsing, and real helper replacement/rollback. It records requests/transferred bytes, logical disk use and available platform peak memory. Regression gates require at least three prior observations matching platform, toolchain and fixtures, retaining five rolling observations. Reports are written before failure to preserve evidence. Public-network throughput, Minecraft main-menu readiness and allocated disk blocks are not measured. Weekly or manual upstream metadata regression covers vanilla release/snapshot catalogs and loader catalogs for validated Minecraft samples.

<a id="microsoft-application"></a>

## Microsoft application registration and acceptance

> **Preview status:** Microsoft sign-in is implemented but has not been verified with a real account. A CCL OAuth application has not yet been registered. The complete Microsoft, Xbox, and Minecraft flow must pass with that application before this feature can meet the CLI preview acceptance gate.

### Register a public desktop application

The launcher uses Microsoft's device authorization flow. It is a public desktop client and must not use a client secret. Microsoft documents desktop app registration and public-client configuration in the [desktop app registration guide](https://learn.microsoft.com/en-us/entra/identity-platform/scenario-desktop-app-registration) and its [device code flow reference](https://learn.microsoft.com/en-us/entra/identity-platform/v2-oauth2-device-code).

1. Sign in to the [Microsoft Entra admin center](https://entra.microsoft.com/) with an account allowed to create app registrations.
2. Open **Identity → Applications → App registrations → New registration**.
3. Give the application a clear launcher name. For the account type, select **Personal Microsoft accounts only** for the currently implemented `/consumers` authority. Microsoft personal accounts are the intended account type for a Minecraft Java owner. If the portal does not offer this choice, stop and confirm the registration design before using a broader type.
4. Register the application and copy its **Application (client) ID**. This GUID is public configuration, not a password.
5. Open **Authentication**. Under **Advanced settings**, set **Allow public client flows** to **Yes**, then save. Device-code clients do not use a client secret or a browser redirect URI.
6. Do not create a client secret, and do not put access or refresh tokens in launcher settings.
7. Confirm the current Xbox and Minecraft service access requirements for your application. Registration alone does not prove that the complete sign-in flow will work. If a service rejects the application, resolve access using the returned error and that service’s official support process.

Microsoft's [app registration quickstart](https://learn.microsoft.com/en-us/entra/identity-platform/quickstart-register-app) describes supported account types. The desktop guide explains when a desktop client is a public client and how to enable public-client flows. This project has not yet validated whether its application registration and API access are approved for public users.

### Validation checklist for the application owner

Before calling account sign-in ready for a public preview, record evidence that:

- the registered public client ID is the one shipped/configured for the launcher and has completed required Microsoft review;
- device-code login works for a Minecraft Java account with an entitlement;
- denial, expiry, cancellation, refresh, sign-out, and unavailable credential-vault paths behave correctly;
- no tokens appear in CLI output, logs, error text, ordinary configuration, or account metadata;
- the Linux build works with a real persistent Secret Service/D-Bus session;
- the service/API permissions and distribution comply with current Microsoft, Xbox, and Minecraft requirements.

Until those checks are complete, the authentication feature and preview package must be described as **unverified and not ready for public distribution**.

<a id="validation"></a>

## Validation and preview packages

Run the same checks used by CI:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --all-targets --locked
cargo build --workspace --release --locked
```

CI covers Windows x64, macOS arm64 and Linux x86_64. Windows also runs the opt-in OS credential-vault probe. Live provider and real-account checks must be reported separately from service fixtures. Preview scripts package the binary, license, READMEs and all paired public guides; when reorganizing docs, update links, CLI diagnostic paths and both scripts together.

Unit fixtures cover provider search and version payload mapping, dependency types and pins, CurseForge file metadata and relationships, key redaction and header scoping, update selection, transactional ownership, rollback, and conflicts. A loopback HTTP fixture exercises Modrinth search, version listing, and recursive pinned dependency resolution. A separate opt-in test queries the public Modrinth API.

<a id="scope"></a>

## Current feature scope

The core provides configuration, directory selection, HTTP downloads, version lookup and installation, managed Java runtime installation under `.ccl/runtimes`, Microsoft account authentication, launch planning, native extraction, child-process output capture, Modrinth pack handling, CurseForge and MultiMC/Prism archive conversion, Modrinth/CurseForge online content operations, isolated instance lifecycle and retained revision cleanup, Fabric/Quilt/Legacy Fabric/Forge/NeoForge/OptiFine loaders, local content management, and userdata backups. Pack receipts record hashes for pack-owned paths; updates use those hashes to detect local edits, deletions, and collisions before replacing data. Disk-usage reporting counts logical bytes and does not promise physical space reclaimed when hard links exist. Userdata backups are separate from Modrinth exports, which omit saves and private server lists and require explicit redistribution consent for local binaries. CurseForge API key support does not imply that a real key has production authorization. Microsoft account approval and live end-to-end validation remain outstanding; don't describe account login as ready for public use until those gates are complete. Keep the core independent from interfaces and update the English and Chinese user documentation together when changing public behavior.
