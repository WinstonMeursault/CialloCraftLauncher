# Development guide

[User guide](user-guide.en.md) · [API reference](api.en.md) · [简体中文](development.zh-CN.md)

- [Environment](#environment)
- [Build and run](#build)
- [GUI alpha preview](#gui-preview)
- [Architecture and workspace](#architecture)
- [Code boundaries](#boundaries)
- [Configuration and data directories](#data)
- [Instances and transactions](#transactions)
- [Updates, compatibility and diagnostics](#maintenance)
- [Microsoft application setup and optional sign-in validation](#microsoft-application)
- [Validation and preview packages](#validation)
- [Release checklist and commands](#release)
- [First stable release notes draft](#release-notes)
- [Current feature scope](#scope)

<a id="environment"></a>

## Environment

Install the stable Rust toolchain and Cargo; the workspace uses Rust 2024 edition. The release workflows currently use Rust 1.99.0, while ordinary CI follows the Rust stable channel. Install the target's native linker and SDK as well:

- Windows x86_64: MSVC C++ Build Tools and the Windows SDK (`x86_64-pc-windows-msvc`).
- macOS arm64: an arm64 Mac with Xcode Command Line Tools (`aarch64-apple-darwin`).
- Linux x86_64: a C compiler, linker, `make`, `pkg-config`, and glibc development headers (for example, `build-essential` on Ubuntu; target `x86_64-unknown-linux-gnu`).

The release workflow builds each package on its matching GitHub-hosted runner. Cross-compiling locally also requires a working linker and SDK for the selected target. Python 3 is required to assemble packages and generate dependency notices. The Unix ZIP packaging path needs `zip`, `unzip`, and `sha256sum` or `shasum`; the tar.gz path needs `tar` and a SHA-256 utility. Linux account and provider credentials additionally need a persistent, unlocked Secret Service over D-Bus at runtime; no plaintext fallback is provided.

<a id="build"></a>

## Build and run

Run from the repository root:

```sh
cargo build --workspace
cargo run -p ccl-cli -- --help
```

`Cargo.lock` pins workspace dependencies. Use `--locked` for release checks and builds so Cargo fails if manifests and the committed lockfile disagree. HTTP uses Rustls; the selected `keyring` backends include native credential-store dependencies and may compile vendored native sources, so install the platform build tools listed above.

User workflows are in the [user guide](user-guide.en.md); commands, output and core services are in the [API reference](api.en.md).

<a id="gui-preview"></a>

## GUI alpha preview

The desktop adapter is `ccl-gui` version `0.2.0-alpha.1`; `ccl-core` and `ccl-cli` remain `0.1.0`. This is an alpha adapter, not completion of the 0.2 product scope. Validation is in progress as of 2026-10-24. The desktop app is not included in the CLI release archives, and the three platform targets describe build intent rather than completed validation.

`crates/ccl-gui` is a Rust adapter/library with `default = []`; its optional `desktop` feature enables Tauri 2. `apps/ccl-gui` is the React/TypeScript/Vite frontend. The CLI and default Rust builds do not require Node.js or a WebView. The frontend uses Node 24 and pnpm 11.25.0.

The app requires a confirmed game root before opening the library. The setup screen can choose a directory with the native picker or confirm the proposed directory; only confirmation creates a missing directory. Existing roots and managed instances are reused. Vanilla and managed instances can be created, including shared vanilla installs and managed instances with supported loaders. A native file picker inspects supported Modrinth, CurseForge, and MultiMC/Prism archives before import. Launch accepts an offline profile or a saved Microsoft account, with Java path, memory, managed Java installation, and dry-run controls.

Microsoft account login is experimental. The adapter requests the OS credential vault check before login and stores tokens only through the OS vault; account metadata is kept in `accounts.json` beside the core config. Linux needs an unlocked Secret Service available through the session D-Bus. The GUI never returns device or access tokens to the frontend. The task page shows at most one active task and the eight most recent completed snapshots. Backend task logs are capped at 1,000 lines of 512 characters; the task view shows the latest 100. Progress and log notifications are throttled to 10 per second, with lifecycle updates sent immediately. Install/import work can be cancelled cooperatively; stopping a game is a separate confirmed action. The task registry continues running if the user changes pages. The app asks before closing during active work and on a second launch raises the existing window. Live and saved game logs are available on the Logs page. Production tasks come from connected operations; the synthetic task probe is test-only.

GUI appearance preferences (`theme`, `language`, and selected root) live in a separate `gui.json`. The network-offline switch is different: it updates and persists the core `downloads.offline` setting in the shared launcher config, so CLI and GUI observe the same value. Offline player identity is a launch choice and does not disable networking. `CCL_GUI_DATA_DIR` selects GUI preference data, `CCL_GUI_CONFIG_PATH` selects the shared core config, and `CCL_GUI_GAME_DIR` pins the game root and disables root changes. Use distinct temporary values for tests. Reading an existing library may acquire instance locks or recover an interrupted instance transaction.

### Build and check the desktop alpha

From the repository root, with Node 24, pnpm 11.25.0, Rust, and the target's native dependencies installed:

```sh
cd apps/ccl-gui
pnpm install --frozen-lockfile
pnpm build
node --test tests/task-snapshot.test.ts
cd ../..
cargo test -p ccl-gui --locked
cargo run -p ccl-gui --features desktop --release --locked
```

The Node test covers stale/new task snapshot ordering; `.github/workflows/gui.yml` runs it before building the frontend. The workflow also runs workspace formatting, adapter tests, desktop Clippy, and an optimized desktop build on Windows x64, macOS arm64, and Linux x86_64. These jobs are validation work in progress, not a claim that all desktop targets or game launches have passed. The Linux job installs `libwebkit2gtk-4.1-dev`, `build-essential`, `libssl-dev`, `libxdo-dev`, `librsvg2-dev`, `libayatana-appindicator3-dev`, and `patchelf`. Tauri uses WebView2 on Windows, WKWebView on macOS, and WebKitGTK 4.1 on Linux; see [Tauri's v2 prerequisites](https://v2.tauri.app/start/prerequisites/). CCL's minimum supported OS versions and full platform behavior are not yet validated.

<a id="architecture"></a>

## Architecture and workspace

### Goals

CialloCraftLauncher started with a command-line interface. The reusable Rust core provides configuration, directory, network, download, and installation logic; the CLI and the current desktop alpha call the same core services. The core does not depend on either interface, so desktop features can grow without moving business logic into the UI layer.

Initial target platforms:

- Windows x86_64
- macOS arm64
- Linux x86_64

### Workspace

- `crates/ccl-core`: shared services, configuration, errors, HTTP transport, task events, installation, Java discovery, and launch orchestration.
- `crates/ccl-cli`: command-line arguments, text or JSON output, exit codes, and calls into the core.
- `crates/ccl-gui` and `apps/ccl-gui`: optional desktop adapter and React/TypeScript/Vite frontend; see [GUI alpha preview](#gui-preview) for current scope and checks.
- `docs`: public documentation for users and contributors.

The core reports progress through task contexts and provides a cooperative cancellation handle. An interface displays events and collects input; the core performs work, validates configuration, and returns structured errors. The core must not depend on an interface.

<a id="boundaries"></a>

## Code boundaries

`loaders`, `instance`, and `launch --instance` reuse the same core services in the CLI and desktop alpha. `modpack_format` owns Modrinth archive parsing, validation, and writing; `import_formats` detects CurseForge and MultiMC/Prism archives and converts supported inputs through the transactional pack pipeline; `modpacks` coordinates import/export and ownership receipts; `content` manages local files and userdata backups. Online provider integrations live in `online_content`. Pack imports and content edits use the same staged instance transaction and cross-process lock as instance changes. Modern Forge/NeoForge processors are launched by core with output delivered through task events; they must not write directly to CLI stdout. NeoForge SHA-512 fallback verifies installer bytes before the install pipeline derives its SHA-1 value.

- Put reusable domain logic in `crates/ccl-core`.
- `crates/ccl-core` must not depend on terminal or graphical-interface libraries.
- The CLI parses arguments, formats output, and maps exit codes; it does not duplicate core business rules.
- Tasks report progress through core events and respond to cancellation through the cancellation handle.
- Configuration errors should include understandable context; secrets must not be written to logs or output. Settings changes go through the core's lock-serialized `ConfigStore::update` transaction and validated atomic save. Keep OS credential storage mandatory for account tokens; Linux requires an available persistent Secret Service/D-Bus session, with no plaintext fallback.

<a id="data"></a>

## Configuration and data directories

Configuration keys and default paths are maintained in the [user guide](user-guide.en.md#configuration).

`auth.microsoft_client_id` is a public application ID, not a secret. Account metadata (such as account ID, name, profile IDs and selected account) is stored separately from OAuth/Minecraft tokens, which must stay in the OS credential vault scoped to the account-index path. The CurseForge API key is held in a separate OS-vault entry scoped to the configuration path; the optional `CCL_CURSEFORGE_API_KEY` environment override takes precedence. Neither credential type belongs in JSON configuration, logs, source control, or command arguments.

`auth.microsoft_client_id` is a public application ID, not a secret. Account metadata (such as account ID, name, profile IDs and selected account) is stored separately from OAuth/Minecraft tokens, which must stay in the OS credential vault scoped to the account-index path. The CurseForge API key is held in a separate OS-vault entry scoped to the configuration path; the optional `CCL_CURSEFORGE_API_KEY` environment override takes precedence. Neither credential type belongs in JSON configuration, logs, source control, or command arguments.

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

The CLI accepts a CurseForge key through `ccl credentials curseforge set`, which reads from a hidden terminal prompt and stores it in the OS credential vault scoped to the selected config path. `CCL_CURSEFORGE_API_KEY` is an environment override. CCL does not write the key to JSON settings or account metadata, and sends it only to the approved CurseForge API/CDN hosts. Vault availability and a successful local save do not establish that CurseForge has authorized a real key for production access; confirm that with the key owner/provider before release. See [credential handling](user-guide.en.md#configuration) and [API behavior](api.en.md).

The `modpack_format` module reads and writes Modrinth format version 1 archives, validates manifest fields and destination paths, limits ZIP expansion, and applies `overrides/` with client overrides layered on top. `modpacks` imports selected files and overrides transactionally and stores a receipt with hashes of pack-owned paths. Updates compare current files with those ownership hashes and stop on modified/deleted owned files or collisions with unowned local data unless the caller explicitly authorizes replacement. After a pack or content change, `instance rollback` restores the prior full instance snapshot, including userdata and saves; users should back up first when they need to retain newer saves. `content` manages local content and portable userdata backups. Backups include user data and saves but exclude game/loader files, Java runtimes, and account credentials. Modrinth exports are separate: they exclude saves and private server lists, and local binary files require explicit redistribution authorization. See [Modrinth packs and instance content](user-guide.en.md#packs) for the commands and safety details.

<a id="maintenance"></a>

## Updates, compatibility and diagnostics

`update::UpdateService` checks only `WinstonMeursault/CialloCraftLauncher`. Stable packages use a `v<VERSION>` tag on `main`. Every `dev` push starts validation and packaging on Windows x64, macOS arm64 and Linux x86_64; the matching commit is published as immutable `dev-<full-SHA>` only after all jobs pass. The release workflow keeps the ten newest developer releases. Stable tags must match the CLI crate version and point to a commit on `main`. Manifests bind repository, channel, branch, full commit, target, executable digest and data reader range (`1..=1`).

`UpdateService::check` distinguishes an available package from a branch head whose build is not published yet. `stage` downloads and validates the bounded ZIP, its paths and hashes, then stages the candidate. `Installation` owns channel preference and the persistent transaction journal; `launch_helper` starts the restricted old-image helper, and `execute_helper` completes replacement, health check or rollback asynchronously. `prepare_rollback` checks current data compatibility before changing transaction state. The binary updater does not restore game data. There is no data migration in the current format; readers preserve schema-less config/account/instance metadata and accept receipt schema 1. Unknown future config or instance fields and receipt schemas above 1 are refused. The initial regression uses config and account metadata generated by a git archive of baseline `2edbf0e`; those fixtures are sanitized, and the fixture account secrets were never persisted.

Release archives are unsigned. HTTPS, size and SHA-256 checks detect corruption but do not authenticate the publisher. Do not describe these checks as signatures or verified publisher identity. OS signing/notarization is not implemented. Update `status`, `rollback` and `recover` do not need network access; update check/apply do, and `--network-offline` blocks them.

`compatibility::inspect_compatibility(config_path, game_root)` is read-only, does not access the OS credential vault, and only inspects CCL-owned metadata. A future migration must lock and back up affected metadata, stage and atomically commit, recover safely after interruption, and refuse rollback if the retained reader cannot consume the new data. Never roll back the launcher by restoring a whole game directory.

`DiagnosticsService::doctor` reports Java candidates, directory access and whether a credential adapter is compiled in; vault unlock state is not probed. Network and write probes are explicit. `diagnose_run` checks a bounded run log plus timestamp-correlated crash reports and classifies only supported patterns. Export serializes an allowlist and substitutes paths; raw output preview requires a separate opt-in, is limited to 16 KiB and applies best-effort redaction. It can still contain secrets written by the game or mods and is never uploaded by CCL.

P7 provides three-platform release validation and transactional updater tests, including Windows executable sharing locks, failed health checks and recovery of a missing entrypoint. The first P7 installation is manual because earlier binaries have no updater protocol. Current data formats require no migration; old-writer compatibility samples do not constitute every historical receipt generation. The regular performance workflow measures fixed local HTTP install fixtures, cache reuse, CLI startup and ZIP parsing, and real updater commit/rollback on all three platforms. It records requests and transferred bytes, logical disk use, and platform-available peak memory. Its historical regression gate starts after three compatible prior observations and compares up to five rolling samples; reports are uploaded even when a gate fails. Public-network throughput, Minecraft main-menu readiness and allocated disk blocks are not measured.

For direct investigation, manually dispatch `.github/workflows/performance.yml` with `compare_ref` set to a baseline branch, tag, or full 40-character commit SHA. The paired comparison runs both revisions on the same Ubuntu 24.04 runner with Rust 1.99.0 and the same Linux target. It copies the candidate `p7_benchmark.rs` and `update_process.rs` into the baseline checkout so both revisions use the same measurement harness, records both commits, runner identity and harness hashes, and builds each revision's CLI, benchmark example, and updater test. The harness runs five alternating baseline/candidate pairs. Each pair records three cold/warm installation and updater measurements per build (15 each in total), plus seven CLI/parser measurements (35 in total). It compares paired medians for cold and warm installation, metadata parsing, updater commit, and health-failure rollback; a median slowdown above 25% fails the gate. The report is retained for 90 days. This comparison is Linux-only and complements the scheduled/push history workflow.

Weekly or manual upstream metadata regression covers vanilla release/snapshot catalogs and loader catalogs for validated Minecraft samples.

<a id="microsoft-application"></a>

## Microsoft application setup and optional online sign-in validation

> **Owner decision (2026-10-23):** Offline identity and username launch remain supported for the first stable release. Microsoft device sign-in is implemented as an experimental online integration; the owner has deferred genuine-account validation and acceptance, so it is not a first-release gate. The latest recorded live retry passed Microsoft OAuth, Xbox Live, and XSTS, then received HTTP 403 at Minecraft login before profile/entitlement data. The cause was not established. Describe online sign-in as experimental until future authenticated-account acceptance is completed.

### Register a public desktop application

The launcher uses Microsoft's device authorization flow. It is a public desktop client and must not use a client secret. Microsoft documents desktop app registration and public-client configuration in the [desktop app registration guide](https://learn.microsoft.com/en-us/entra/identity-platform/scenario-desktop-app-registration) and its [device code flow reference](https://learn.microsoft.com/en-us/entra/identity-platform/v2-oauth2-device-code).

1. Sign in to the [Microsoft Entra admin center](https://entra.microsoft.com/) with an account allowed to create app registrations.
2. Open **Identity → Applications → App registrations → New registration**.
3. Give the application a clear launcher name. For the account type, select **Personal Microsoft accounts only** for the currently implemented `/consumers` authority. Microsoft personal accounts are the intended account type for a Minecraft Java owner. If the portal does not offer this choice, stop and confirm the registration design before using a broader type.
4. Register the application and copy its **Application (client) ID**. This GUID is public configuration, not a password.
5. Open **Authentication**. Under **Advanced settings**, set **Allow public client flows** to **Yes**, then save. Device-code clients do not use a client secret or a browser redirect URI.
6. Do not create a client secret, and do not put access or refresh tokens in launcher settings.
7. Confirm the current Xbox and Minecraft service access requirements for your application. Registration alone does not prove that the complete sign-in flow will work. If a service rejects the application, resolve access using the returned error and that service’s official support process.

Microsoft's [app registration quickstart](https://learn.microsoft.com/en-us/entra/identity-platform/quickstart-register-app) describes supported account types. The desktop guide explains when a desktop client is a public client and how to enable public-client flows. Registration and successful earlier exchange stages do not establish public-user API approval.

### Optional checklist for future authenticated-login acceptance

If the owner later seeks to mark Microsoft online sign-in as accepted for authenticated accounts, record evidence that:

- the registered public client ID is the one shipped/configured for the launcher and has completed required Microsoft review;
- device-code login works for a Minecraft Java account with an entitlement;
- denial, expiry, cancellation, refresh, sign-out, and unavailable credential-vault paths behave correctly;
- no tokens appear in CLI output, logs, error text, ordinary configuration, or account metadata;
- the Linux build works with a real persistent Secret Service/D-Bus session;
- current Microsoft, Xbox, and Minecraft service/API requirements for authenticated sign-in and distribution have been checked.

The latest recorded validation had no Minecraft Java entitled account available. Keep that limitation attached to claims about online authenticated sign-in. The owner chose to preserve the offline identity flow while deferring this optional acceptance work.

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

The CI matrix runs formatting, Clippy with warnings denied, all workspace tests, and optimized release builds on all three targets. Windows additionally runs ignored credential-vault round-trip probes for account credentials and provider keys. The weekly/manual upstream metadata workflow is a separate read-only check (Linux by default; optionally all three platforms). The performance workflow is also separate and records benchmark reports; it is not a substitute for release validation. The standalone preview workflow builds and packages artifacts but does not publish a release.

Unit fixtures cover provider search and version payload mapping, dependency types and pins, CurseForge file metadata and relationships, key redaction and header scoping, update selection, transactional ownership, rollback, and conflicts. A loopback HTTP fixture exercises Modrinth search, version listing, and recursive pinned dependency resolution. A separate opt-in test queries the public Modrinth API.

<a id="release"></a>

## Release checklist and commands

### Before a stable release

1. Describe authentication and provider scope accurately. Offline identity remains supported; the owner deferred genuine-account Microsoft online acceptance on 2026-10-23, so it is not a first-stable-release gate. Describe the Microsoft online integration as experimental, and don't infer live CurseForge authorization from fixtures or a local key save. Inspect current all-platform performance artifacts; use the paired Linux comparison against a selected baseline when investigating a suspected regression.
2. Start from a clean working tree, fetch the remote refs, and integrate the accepted `dev` commit into `main`. Preserve any independent local-main changes and resolve/review merge conflicts; do not reset or force-push the branch. Once the release source is on `main`, set the `version` fields in both `crates/ccl-core/Cargo.toml` and `crates/ccl-cli/Cargo.toml` to the release's three-part version. Refresh and commit `Cargo.lock`, then confirm the versions agree:

   ```sh
   git fetch origin
   git switch main
   git pull --ff-only origin main
   git merge --no-ff origin/dev
   # Resolve/review any conflicts, then edit both package versions if needed.
   cargo check --workspace
   python3 scripts/check-package-versions.py --expected X.Y.Z
   ```

Review `git diff -- Cargo.lock` after the check and keep only lockfile changes required by the workspace version change. Do not refresh unrelated dependency versions as part of release preparation.

3. Run the local validation suite and check the diff. These are the release workflow's source checks for the host target; the hosted workflow repeats them on every release target.

   ```sh
   cargo fmt --all -- --check
   cargo clippy --workspace --all-targets --locked -- -D warnings
   cargo test --workspace --all-targets --locked
   cargo build --workspace --release --locked
   git diff --check
   ```

4. Commit and push the version change and release-ready source to `main`. If the selected version is already correct and the merge contains no additional version changes, skip the version-only add/commit commands below. Stage only reviewed paths, including the package version manifests and lockfile. Confirm the intended commit is reachable from `origin/main`. Then create and push the exact matching tag:

   ```sh
   git add Cargo.lock crates/ccl-core/Cargo.toml crates/ccl-cli/Cargo.toml
   git commit -m "chore: prepare vX.Y.Z"
   git push origin main
   git fetch origin main
   git merge-base --is-ancestor HEAD origin/main
   # Wait for the main-branch CI to pass before creating the version tag.
   git tag -a vX.Y.Z -m "CCL vX.Y.Z"
   git push origin vX.Y.Z
   ```

The tag starts `.github/workflows/release-stable.yml`. It checks the tag is on `main`, verifies both crate versions match, runs formatting, Clippy, tests and the Windows vault probes, then builds and smoke-checks Windows x64, macOS arm64 and Linux x86_64 ZIPs. The publish job refuses an existing GitHub release with that tag, creates a draft release, generates three updater manifests, downloads the assets again, checks archive checksums and manifest-bound executable hashes/identities, and only then publishes it as the latest stable release. Stable version tags must be unique and must never be moved to a different commit.

Before tagging, package one host build locally with the corresponding script if useful. The updater package is a ZIP on all three platforms:

```sh
# Linux native host; use aarch64-apple-darwin on an Apple-silicon Mac.
TARGET=x86_64-unknown-linux-gnu
cargo build --release --locked --target "$TARGET" -p ccl-cli
TARGET="$TARGET" ARCHIVE_EXT=zip bash scripts/package-preview.sh
```

```powershell
$target = 'x86_64-pc-windows-msvc'
cargo build --release --locked --target $target -p ccl-cli
$env:TARGET = $target
./scripts/package-preview.ps1
```

An ordinary local build has an unpublished commit identity unless the release build environment is supplied; use it for local checks, not as a substitute for the workflow-produced update package. The package scripts require the appropriate `target/<triple>/release/ccl` executable (`ccl.exe` on Windows). At the archive root, they include the project license, both READMEs, each paired English/Simplified Chinese guide, generated `THIRD-PARTY.en.md` / `THIRD-PARTY.zh-CN.md` inventories, and paired `THIRD-PARTY-LICENSES.en.md` / `THIRD-PARTY-LICENSES.zh-CN.md` bundles containing the original license and notice texts. `scripts/third-party-notices.py` walks the target-specific normal and build dependency graph of `ccl-cli`; each inventory links to deterministic anchors in its matching language bundle. The bundles localize their introductory labels while retaining identical original license text bytes; third-party texts are consolidated into these two files. The packaging scripts verify all four notice files are present and the ZIP has no more than 256 entries, keeping it installable by current readers. The updater's 256-entry ZIP limit remains unchanged. Package scripts also smoke-check the archive and write a SHA-256 file. A checksum detects accidental corruption but the archive and executable are unsigned, so it does not authenticate the publisher.

### Developer builds

Push a validated commit to `dev` to start `.github/workflows/release-dev.yml`:

```sh
git switch dev
git pull --ff-only origin dev
# make and commit the intended changes
git push origin dev
```

The workflow requires the tested commit to remain the current `dev` tip while all three platform jobs validate and package. On success it creates immutable tag `dev-<full commit SHA>`, verifies the three ZIPs, checksums and manifests, publishes a prerelease, and retains the newest ten matching developer releases. Do not create or push that tag manually. If another commit reaches `dev` during validation, no release for the stale tip should be published; validate the new tip instead.

### Troubleshooting release gates

- **Stable tag rejected:** confirm the tag commit is an ancestor of `origin/main`, both package versions equal the tag without the `v`, and `Cargo.lock` is committed. The checker is `python3 scripts/check-package-versions.py --expected X.Y.Z`.
- **`--locked` reports a lockfile change:** update the two package versions, run `cargo check --workspace`, inspect and commit only required `Cargo.lock` changes, then repeat locked checks.
- **Native link/build failure:** install the target-specific compiler/linker and SDK listed under [Environment](#environment). A host build does not prove another target can link.
- **Credential vault probe fails:** unlock the OS credential service. On Linux, ensure a persistent Secret Service/D-Bus session is available; CCL deliberately returns an error instead of saving credentials in plaintext.
- **Developer release is absent:** inspect all three validation jobs and check whether `dev` moved past the tested commit. A failed or stale-tip job does not produce an immutable package.
- **Hash check passes:** this confirms byte integrity against the manifest/checksum. It does not prove that the package was signed or published by a verified identity.

<a id="release-notes"></a>

## First stable release notes draft

<!-- release-notes:start -->
# CialloCraftLauncher 0.1.0

This release provides a Rust CLI and shared core for installing and launching Minecraft, managing isolated instances, installing supported loaders, importing supported Modrinth/CurseForge/MultiMC/Prism packs, and managing local content and backups. The launcher can stage, health-check, and roll back its own executable update; it does not migrate game data.

**Authentication and provider status:** Offline identity launch is supported. Microsoft online sign-in is experimental; the owner deferred genuine Java account validation and acceptance on 2026-10-23. CurseForge keys are supplied by users and stored in the OS credential vault; production API access depends on provider authorization.

**Compatibility and security:** Packages are unsigned. SHA-256 checks detect changed package bytes but do not authenticate the publisher. Back up important worlds before pack operations that replace content.

Packages are provided for Windows x86_64, macOS arm64, and Linux x86_64. Review the included guides for setup, supported formats, and current platform requirements.
<!-- release-notes:end -->

`scripts/release-notes.py` extracts the marked English and Chinese bodies, checks that their titles match the requested package version, and combines them for `.github/workflows/release-stable.yml` to use as the GitHub release notes. Keep both marked sections accurate and their version titles synchronized with the Cargo package version. Review current all-platform performance artifacts before publication; use the paired Linux comparison with a selected baseline when investigating a suspected regression. Historical P7 checks do not establish performance for the release commit.

<a id="scope"></a>

## Current feature scope

The core provides configuration, directory selection, HTTP downloads, version lookup and installation, managed Java runtime installation under `.ccl/runtimes`, Microsoft account authentication, offline identity launch, launch planning, native extraction, child-process output capture, Modrinth pack handling, CurseForge and MultiMC/Prism archive conversion, Modrinth/CurseForge online content operations, isolated instance lifecycle and retained revision cleanup, Fabric/Quilt/Legacy Fabric/Forge/NeoForge/OptiFine loaders, local content management, and userdata backups. Pack receipts record hashes for pack-owned paths; updates use those hashes to detect local edits, deletions, and collisions before replacing data. Disk-usage reporting counts logical bytes and does not promise physical space reclaimed when hard links exist. Userdata backups are separate from Modrinth exports, which omit saves and private server lists and require explicit redistribution consent for local binaries. CurseForge API key support does not imply that a real key has production authorization. Microsoft online sign-in is experimental; the owner deferred genuine-account acceptance on 2026-10-23, while offline identity remains supported. Keep the core independent from interfaces and update the English and Chinese user documentation together when changing public behavior.
