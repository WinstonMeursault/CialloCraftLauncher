# Development guide

[User guide](user-guide.en.md) · [API reference](api.en.md) · [简体中文](development.zh-CN.md)

- [Environment](#environment)
- [Build and run](#build)
- [Desktop GUI and package updates](#gui-g3)
- [Architecture and workspace](#architecture)
- [Code boundaries](#boundaries)
- [Configuration and data directories](#data)
- [Instances and transactions](#transactions)
- [Updates, compatibility and diagnostics](#maintenance)
- [Microsoft application setup and optional sign-in validation](#microsoft-application)
- [Validation and preview packages](#validation)
- [Release checklist and commands](#release)
- [v0.2.3 release notes](#release-notes)
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

### Automatic memory (development branch)

`memory` owns the pure budget function, serializable policy/decision and platform probes. Windows uses GlobalMemoryStatusEx; macOS uses sysctl and Mach statistics through small native FFI calls; Linux reads procfs and resolves v1/v2 cgroup mounts/membership plus ancestor limits. No monitoring daemon, shell memory command or hardware inventory dependency is added. libc is a macOS-only direct dependency; windows-sys enables the memory-status feature.

macOS accepts older valid Mach statistics revisions by checking only the counter prefix it reads. XNU free_count already includes speculative pages, so they must not be added again. See [Apple host statistics](https://github.com/apple-oss-distributions/xnu/blob/main/osfmk/kern/host.c).

The core config retains the manual value separately from auto_memory=true and stores per-profile policies under instance_memory. ConfigStore provides atomic, process-locked edits; GUI expected-value checks prevent stale instance edits. The launch service resolves the final budget after resource preparation and before native extraction/plan construction. PlanOptions continues to accept a numeric resolved maximum. Default launch behavior is automatic in core config, CLI and GUI.

Tests cover reserve/rounding/cap/low-memory and fallback cases, Linux byte units/limits, native probes on each CI host, setting precedence and persistence, CLI auto/numeric/inherit, GUI request shape and per-instance stale-edit rejection, and conflicting JVM heap aliases. GUI acceptance should verify automatic defaults, hidden manual input, explicit save, global/instance switching and unchanged home layout. Keep test windows and data isolated; do not install a QA app into Applications.

### Performance branch: release builds and cache verification

The performance branch uses Thin LTO, one code-generation unit and symbol stripping for release builds. It keeps the default release optimization level and unwinding behavior; task panic handling, checksums, signatures, notices and supported archive formats remain enabled. Symbol stripping reduces executable symbol information, so use an unstripped local build for symbol-level debugging. ZIP uses the zlib-rs DEFLATE backend without the unused Zopfli encoder; normal DEFLATE import/export remains supported.

Cached artifacts are checked with a bounded 64 KiB buffer on Tokio's blocking pool, with a separate cancellation-aware gate capped by `downloads.max_parallel`. The verification permit stays with the worker even if its waiting future is dropped. Size mismatches fail before content reads; requested digests are checked in one streaming pass. Network and resumed transfers compute only the requested SHA-1/SHA-512 digests. The under-lock cache recheck remains to protect concurrent publication, and offline mode still rejects invalid cache files.

For a larger opt-in local measurement, run `cargo run --release --locked -p ccl-core --example artifact_benchmark -- --bytes 67108864 --iterations 3`. It generates a deterministic 64 MiB fixture, streams it from localhost, and measures cold/warm SHA-1, SHA-512, dual-hash and size-only requests. Every warm pass must reuse the file without network traffic. Run the identical harness in both revisions, alternate their order, and distinguish raw binaries, normalized compression and actual installer sizes. This fixture does not measure public-network performance or Minecraft gameplay.

GUI management pages use React memoization with stable page inputs while retaining their mounted state and scroll positions. Task/log updates no longer require rerendering an unchanged library. The default wallpaper avoids an unnecessary background-image IPC read. Browser fixture component counts are separate evidence from native WebView startup time or GPU load; keep test probes out of normal frontend bundles.

<a id="gui-preview"></a>
<a id="gui-g3"></a>

## Desktop GUI and package updates

The desktop application is a separate Tauri product named `ccl-gui`, sharing the core with the CLI while keeping a distinct product identity and updater manifest. CLI release ZIPs remain available alongside these desktop package formats:

| Target | Distribution package | In-app update payload | Install method |
| --- | --- | --- | --- |
| Windows x64 | Per-user NSIS `.exe` | NSIS `.exe` | Writable user installation; portable and machine-wide installations are unsupported by the GUI updater |
| macOS arm64 | `.dmg` | `.app.tar.gz` containing the app bundle | Writable `.app`; packages are not notarized |
| Linux x64 | `.AppImage` and `.deb` | AppImage | AppImage needs a writable file location; DEB updates use the system package manager |

The release workflows target Windows 2025, macOS 15 and Ubuntu 24.04. They launch the desktop app and confirm the frontend-ready marker on each host, using Xvfb with software rendering on Linux and WebView2 on Windows. These runners do not prove physical game launch or compatibility with older systems. Windows 10 with WebView2, macOS 13, and Linux with glibc 2.39 are intended minimums, but have not been certified.

`crates/ccl-gui` is a Rust adapter/library with `default = []`; its optional `desktop` feature enables Tauri 2. `apps/ccl-gui` is the React/TypeScript/Vite frontend. The workspace's default members are only `ccl-core` and `ccl-cli`, so a plain default `cargo build` or `cargo test` omits the GUI; use `--workspace` or target `ccl-gui` explicitly. Normal CLI and core builds do not require Node.js or a WebView. The frontend uses Node 24 and pnpm 11.25.0.

The `gui-validation` feature enables test-only GUI update drivers and hooks. The frontend maintenance performance harness is separately gated by `VITE_CCL_GUI_PERF_HARNESS=true`; the isolated performance acceptance scenario requires both gates to be enabled. Do not enable either for normal release packages.

macOS bundles explicitly use `bundle.macOS.signingIdentity: "-"` so Tauri seals the complete app, rather than relying on the executable’s linker signature. `scripts/package-gui-release.py` invokes `scripts/verify-macos-bundle.py` on macOS: `codesign --verify --deep --strict` must accept the build bundle and the apps extracted from both the updater archive and read-only DMG. A signature failure aborts packaging in GUI validation, dev and stable workflows. These checks verify bundle integrity, not Developer ID trust or notarization; browser-download Gatekeeper approval remains separate. The v0.2.1 correction originated on `dev` in commit `c366ce4`. The original v0.2.0 macOS DMG and updater archive still predate it and retain the incomplete linker-only signature; see the user guide for local recovery.

The app requires a confirmed game root before opening the library. The setup screen can choose a directory with the native picker or confirm the proposed directory; only confirmation creates a missing directory. The library combines discovered versions in the selected root with existing CCL game directories and does not move or convert them. Display-name changes are metadata only; remove from library hides an entry without deleting files, and Settings can restore hidden entries. Lifecycle actions that require CCL revision storage are offered only where that storage already exists. Language and appearance are configured in Settings, which is available before setup is complete. A native file picker inspects supported Modrinth, CurseForge, and MultiMC/Prism archives before import. Launch selects a saved offline account; Java path, memory, managed Java installation, and dry-run controls are available. GUI Microsoft sign-in is temporarily unavailable.

Saved GUI offline profiles use account metadata only and never touch the OS credential vault. Microsoft sign-in is temporarily unavailable in the GUI; the experimental CLI device-login protocol remains. The Settings advanced disclosure retains the public Microsoft client ID. The GUI keeps one active task and up to eight completed snapshots in its native task registry while showing work in a collapsible progress dialog with a bottom-right reopen entry; the dialog collapses when a game PID appears. Backend task logs are capped at 1,000 lines of 512 characters and each WebView snapshot shows at most the latest 100. Progress/log notifications are coalesced to 10 per second, with lifecycle updates sent immediately. Install/import work can be cancelled cooperatively; stopping a game is a separate confirmed action. Closing during active work prompts first, and a second launch raises the existing window. Game logs and launcher diagnostics have separate views.

GUI appearance preferences (`theme`, `language`, and selected root) live in a separate `gui.json`; language is changed in Settings, including before game-root setup is complete. Library display names and hidden state are stored separately at `<game-root>/.ccl/library.json` (schema 1). Renaming changes only the display name and keeps the same ID/path; hiding never deletes game files, and Restore hidden entries unhides discovered entries. The network-offline switch is different: it updates and persists the core `downloads.offline` setting in the shared launcher config, so CLI and GUI observe the same value. Offline player identity is a launch choice and does not disable networking. `CCL_GUI_DATA_DIR` selects GUI preference data, `CCL_GUI_CONFIG_PATH` selects the shared core config, and `CCL_GUI_GAME_DIR` pins the game root and disables root changes. Use distinct temporary values for tests. Reading an existing library may acquire instance locks or recover an interrupted instance transaction.

The Settings panel edits shared download and launch values independently. The Microsoft client ID remains in the shared config but is exposed only through the Settings advanced disclosure; it is a public application ID. GUI Microsoft sign-in is temporarily unavailable; the experimental CLI flow remains. The adapter supplies the loaded settings document as `expected`; the core config lock compares it during write and rejects stale edits, so reload after an external CLI edit. Reset is available per field and for all fields, including recovery from malformed config. The config picker accepts a validated local JSON config; `CCL_GUI_CONFIG_PATH` pins it and disables selection. Switching config clears native path grants and the selected dropped archive.

The Advanced page uses typed `InstanceRequest`, `OnlineQuery`, and `OnlineMutation` adapters. Provider installation creates the core context-aware `OnlineInstallRequest` with selected game root and instance ID, validating target identity and loader compatibility before applying downloads through the instance transaction. Picker grants bind file operations to paths selected for their purpose. Dropping one `.zip` or `.mrpack` inspects it and opens the editable import modal; import requires an explicit action, and an existing ID is rejected rather than overwritten. Synchronous filesystem mutations are not cancellable once begun; async operations with task context cooperate with cancellation and finish core cleanup.

Maintenance stores CurseForge keys only through the config-scoped OS vault; `CCL_CURSEFORGE_API_KEY` is an explicit override and UI/status responses never expose the value. Diagnostic raw log preview is opt-in, bounded and only best-effort redacted; exported reports stay local and require manual review before sharing. Maintenance provides distinct update controls for the installed GUI package and a selected standalone CLI. Both use stable (`main`) or dev (`dev`) channels.

The launcher log is separate from game/task output. It records an allowlisted startup version/platform/commit, task start and terminal events, `GuiError` codes, and optional fixed-category navigation events. It never persists arbitrary request/response payloads, credentials, paths or JavaScript exception text. Five files rotate at 5 MiB each. The Logs page reads at most 1,000 lines/256 KiB and can export the displayed content locally; it does not upload logs. `frontend_diagnostic` accepts only fixed event categories. Debug navigation logging is off by default. The log directory is `<GUI data directory>/logs`. GUI preference files and launcher logs are separate from the shared core config and account metadata; this UI work adds no config/account schema fields.

### Unreleased UI redesign implementation

The current UI branch splits Play from the full Library. `Home.tsx` owns the bottom-right launch cluster and searchable switcher; `Navigation.tsx` owns the overlay rail with hover/focus and an explicit touch toggle; `AppearanceSettings.tsx` edits local appearance. `App.tsx` continues to coordinate the real bootstrap, account selection, task snapshots, and launch commands. Main navigation surfaces remain mounted and own their scrolling; instance details are entered on demand. No frontend demo data is used for production state.

GUI-local `Preferences` additionally persist reduced transparency/motion, the recent stable instance key, and a managed background filename. Appearance saves update only appearance fields under the existing preference lock, preserving independently changed selection/background. Wallpaper imports bound input/decode/cache sizes and produce a JPEG copy; the frontend loads its data URL once per change, with default-image fallback. The default WebP wallpaper is static and its provenance is recorded in `apps/ccl-gui/src/assets/README.en.md`. No new fields are added to core configuration or account schemas. Navigation is the only backdrop-filter surface, with opaque fallback and system/user accessibility preferences. macOS keeps native window controls/title bar; wallpaper fills the WebView content, not OS-owned window chrome. Native window minimum is 520×480; small-screen layouts stack launch controls.

For final acceptance, use the exact branch commit's GUI workflow on all three supported targets, plus native visual/interaction checks on available hardware. Failed update acceptance retains isolated transaction journals and launcher logs as diagnostic artifacts; unexpected rollback reports its recorded reason immediately. Acceptance manifests use the actual `ccl-gui` Cargo package version, matching the candidate identity checked at startup. UI test bundles must have a distinct name/identifier, isolated config/data/game paths, and be stopped/unregistered/moved to Trash after use. Do not replace the user's real installation. Record performance measurement scope: system-wide GPU counters cannot attribute activity to CCL alone. Neither static images nor passing build checks establish a zero-cost blur effect.

### Build and launch the desktop application

From the repository root, with Node 24, pnpm 11.25.0, Rust, and the target's native dependencies installed:

```sh
cd apps/ccl-gui
pnpm install --frozen-lockfile
pnpm test
pnpm build
cd ../..
target="$(rustc -vV | sed -n 's/^host: //p')"
cargo fetch --locked --target "$target"
python3 scripts/gui-notices.py --target "$target"
cargo test -p ccl-gui --locked
cargo run -p ccl-gui --features desktop --release --locked
# Quit the development window before bundling.
cd crates/ccl-gui
pnpm --package=@tauri-apps/cli@2.12.4 dlx tauri build --features desktop
```

The example uses Bash (macOS/Linux or Git Bash). In PowerShell, set `$target = ((rustc -vV | Select-String '^host: ').Line -replace '^host: ', '')`, then pass `$target` to the same fetch/notice commands. Run frontend commands in `apps/ccl-gui`, native Cargo commands from the repository root, and `tauri build` in `crates/ccl-gui`. Generate notices before a desktop build: the Tauri resources include `target/gui-notices`, covering Rust, production JavaScript, project licenses and both public document editions. `pnpm` is used only to build/package, and is not a runtime dependency.


The Node test covers task snapshot ordering; `.github/workflows/gui.yml` runs it before building the frontend. The workflow also runs formatting, adapter tests, desktop Clippy, bundles the application, and launches the UI to confirm its frontend-ready marker on Windows x64, macOS arm64 and Linux x86_64. Release workflows additionally create distribution packages and updater payloads. A separate acceptance gate exercises a successful update followed by manual rollback, automatic rollback after a bad package, and recovery after an interrupted replacement; it also checks that a game-data sentinel remains intact. Inspect the exact commit's workflow results for acceptance evidence. Linux needs WebKitGTK 4.1 development libraries, Xvfb and FUSE for packaging/smoke launch. Tauri uses WebView2 on Windows, WKWebView on macOS and WebKitGTK 4.1 on Linux; see [Tauri's v2 prerequisites](https://v2.tauri.app/start/prerequisites/).

Both updaters consume a detached signature asset named `<manifest>.sig`, produced by Tauri CLI 2.12.4. CLI manifests are `manifest-<target>.json` with `product: "ccl-cli"`; GUI manifests are `gui-manifest-<target>.json` with `product: "ccl-gui"`. Each signature covers the exact manifest bytes and carries Tauri's signed version trusted comment. The manifests bind repository, product, channel, version, branch, full commit, target, data-schema range and package SHA-256/size/URL. The verifier public key is embedded in `ccl-core`; the `.sig` is not a signature of the OS installer itself. GUI `os_signed: false` explicitly records the absence of OS publisher signing.

The release build sets `CCL_BUILD_CHANNEL`, `CCL_BUILD_COMMIT` and `CCL_BUILD_TARGET` so the GUI reports the release's stable/dev channel, full commit and host target. An ordinary local build uses an unpublished commit identity; do not treat it as a publishable update.

The two workflows use the same repository Actions secrets, `CCL_UPDATE_SIGNING_PRIVATE_KEY` and `CCL_UPDATE_SIGNING_PRIVATE_KEY_PASSWORD`, for both manifest families. Treat this as a long-lived updater trust key: keep its contents out of source and logs, maintain an owner-controlled protected recovery copy, and do not casually rotate or delete it. Installed updaters trust the embedded public key; losing the matching private key prevents future releases from being verified by those installations. Never include secret values in documentation or release notes.

<a id="architecture"></a>

## Architecture and workspace

### Goals

CialloCraftLauncher started with a command-line interface. The reusable Rust core provides configuration, directory, network, download, and installation logic; the CLI and desktop GUI call the same core services. The core does not depend on either interface, so desktop features can grow without moving business logic into the UI layer.

Initial target platforms:

- Windows x86_64
- macOS arm64
- Linux x86_64

### Workspace

- `crates/ccl-core`: shared services, configuration, errors, HTTP transport, task events, installation, Java discovery, and launch orchestration.
- `crates/ccl-cli`: command-line arguments, text or JSON output, exit codes, and calls into the core.
- `crates/ccl-gui` and `apps/ccl-gui`: optional desktop adapter and React/TypeScript/Vite frontend; see [Desktop GUI and package updates](#gui-g3) for scope and checks.
- `docs`: public documentation for users and contributors.

The core reports progress through task contexts and provides a cooperative cancellation handle. An interface displays events and collects input; the core performs work, validates configuration, and returns structured errors. The core must not depend on an interface.

<a id="boundaries"></a>

## Code boundaries

`loaders`, `instance`, and `launch --instance` reuse the same core services in the CLI and desktop GUI. `modpack_format` owns Modrinth archive parsing, validation, and writing; `import_formats` detects CurseForge and MultiMC/Prism archives and converts supported inputs through the transactional pack pipeline; `modpacks` coordinates import/export and ownership receipts; `content` manages local files and userdata backups. Online provider integrations live in `online_content`. Pack imports and content edits use the same staged instance transaction and cross-process lock as instance changes. Modern Forge/NeoForge processors are launched by core with output delivered through task events; they must not write directly to CLI stdout. NeoForge SHA-512 fallback verifies installer bytes before the install pipeline derives its SHA-1 value.

- Put reusable domain logic in `crates/ccl-core`.
- `crates/ccl-core` must not depend on terminal or graphical-interface libraries.
- The CLI parses arguments, formats output, and maps exit codes; it does not duplicate core business rules.
- Tasks report progress through core events and respond to cancellation through the cancellation handle.
- Configuration errors should include understandable context; secrets must not be written to logs or output. Settings changes go through the core's lock-serialized `ConfigStore::update` transaction and validated atomic save. Keep OS credential storage mandatory for account tokens; Linux requires an available persistent Secret Service/D-Bus session, with no plaintext fallback.

<a id="data"></a>

## Configuration and data directories

Configuration keys and default paths are maintained in the [user guide](user-guide.en.md#configuration).

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

The CLI `update::UpdateService` and the GUI package updater check only `WinstonMeursault/CialloCraftLauncher`. Stable releases use a `v<VERSION>` tag on `main`; every `dev` push starts validation and packaging on Windows x64, macOS arm64 and Linux x86_64, then publishes an immutable `dev-<full-SHA>` prerelease only after all jobs pass. The workflow keeps the ten newest developer releases. Both products require a signed manifest with `product`, repository, channel, version, branch, full commit, target, payload digest, and data-reader range (`1..=1`). Stable GUI and CLI versions must match the stable tag.

`UpdateService::check` distinguishes an available package from a branch head whose build is not published yet. `stage` downloads and validates the bounded ZIP, its paths and hashes, then stages the candidate. `Installation` owns channel preference and the persistent transaction journal; `launch_helper` starts the restricted old-image helper, and `execute_helper` completes replacement, health check or rollback asynchronously. `prepare_rollback` checks current data compatibility before changing transaction state. The binary updater does not restore game data. There is no data migration in the current format; readers preserve schema-less config/account/instance metadata and accept receipt schema 1. Unknown future config or instance fields and receipt schemas above 1 are refused. The initial regression uses config and account metadata generated by a git archive of baseline `2edbf0e`; those fixtures are sanitized, and the fixture account secrets were never persisted.

Each manifest has a detached `.json.sig` asset. Tauri CLI 2.12.4 signs the exact JSON bytes and carries a signed version trusted comment. The CLI and GUI use the same long-lived private key from repository Actions secrets; `ccl-core` embeds the matching public key. Losing that private key, or rotating it without a planned client migration, strands existing installations because their embedded trust root cannot verify the new signatures. Keep both secret values out of source, logs, and docs, and retain a protected recovery copy under the release owner's control.

These signatures authenticate the manifest and its package digest; SHA-256 alone detects changes but does not authenticate who made the package. Manifest authentication is distinct from Windows Authenticode signing and macOS Developer ID signing/notarization. The current GUI installers are OS-unsigned and macOS packages are not notarized. The CLI ZIP is also not OS-signed. Update `status`, `rollback` and `recover` do not need network access; update check/apply do, and `--network-offline` blocks them. The first CLI release with the new `product` field and detached signature requires manual installation over older CLI versions, whose updater cannot parse that manifest format.

GUI updater rollback is separate from CLI executable rollback. It updates only a writable per-user NSIS installation on Windows, a writable `.app` on macOS, or a writable AppImage on Linux. Linux DEB installations stay under package-manager control. The helper preserves a checked previous application tree, waits up to 60 seconds for the updated GUI's startup health acknowledgment, and restores the previous tree if health confirmation fails. Recovery after an interrupted GUI transaction also returns to the previous tree where possible. The transaction never includes game data. The About response reports `os_signed: false` and `update_manifest_signatures_required: true`; its core version comes from the linked core build.

`compatibility::inspect_compatibility(config_path, game_root)` is read-only, does not access the OS credential vault, and only inspects CCL-owned metadata. A future migration must lock and back up affected metadata, stage and atomically commit, recover safely after interruption, and refuse rollback if the retained reader cannot consume the new data. Never roll back the launcher by restoring a whole game directory.

`DiagnosticsService::doctor` reports Java candidates, directory access and whether a credential adapter is compiled in; vault unlock state is not probed. Network and write probes are explicit. `diagnose_run` checks a bounded run log plus timestamp-correlated crash reports and classifies only supported patterns. Export serializes an allowlist and substitutes paths; raw output preview requires a separate opt-in, is limited to 16 KiB and applies best-effort redaction. It can still contain secrets written by the game or mods and is never uploaded by CCL.

Earlier CLI maintenance acceptance (historically called P7) covered three-platform release validation and transactional updater cases, including Windows executable sharing locks, failed health checks and recovery of a missing entrypoint. The first CLI build using that updater protocol required manual installation over older binaries. Current data formats require no migration; old-writer compatibility samples do not represent every historical receipt generation. The regular performance workflow measures fixed local HTTP install fixtures, cache reuse, CLI startup and ZIP parsing, and real CLI updater commit/rollback on all three platforms. It records requests and transferred bytes, logical disk use, and platform-available peak memory. Its historical regression gate starts after three compatible prior observations and compares up to five rolling samples; reports are uploaded even when a gate fails. Public-network throughput, Minecraft main-menu readiness and allocated disk blocks are not measured.

For direct investigation, manually dispatch `.github/workflows/performance.yml` with `compare_ref` set to a baseline branch, tag, or full 40-character commit SHA. The paired comparison runs both revisions on the same Ubuntu 24.04 runner with Rust 1.99.0 and the same Linux target. It copies the candidate `p7_benchmark.rs` and `update_process.rs` into the baseline checkout so both revisions use the same measurement harness, records both commits, runner identity and harness hashes, and builds each revision's CLI, benchmark example, and updater test. The harness runs five alternating baseline/candidate pairs. Each pair records three cold/warm installation and updater measurements per build (15 each in total), plus seven CLI/parser measurements (35 in total). It compares paired medians for cold and warm installation, metadata parsing, updater commit, and health-failure rollback; a median slowdown above 25% fails the gate. The report is retained for 90 days. This comparison is Linux-only and complements the scheduled/push history workflow.

Weekly or manual upstream metadata regression covers vanilla release/snapshot catalogs and loader catalogs for validated Minecraft samples.

<a id="microsoft-application"></a>

## Microsoft application setup and optional online sign-in validation

> **Current scope:** GUI Microsoft sign-in is temporarily unavailable. The existing CLI device-login protocol remains experimental; genuine-account validation and acceptance are still deferred. Saved GUI offline profiles and CLI offline launch remain supported. A recorded live retry passed Microsoft OAuth, Xbox Live and XSTS, then received HTTP 403 at Minecraft login before profile/entitlement data; the cause was not established. Do not imply the GUI can currently start Microsoft sign-in.

### Register a public desktop application

The launcher uses Microsoft's device authorization flow with the `/consumers` authority. It is a public desktop client and must not use a client secret. Microsoft's current [desktop application configuration guide](https://learn.microsoft.com/en-us/entra/identity-platform/scenario-desktop-app-configuration) explains public-client configuration; the [device code flow reference](https://learn.microsoft.com/en-us/entra/identity-platform/v2-oauth2-device-code) documents supported authorities.

1. Sign in to the [Microsoft Entra admin center](https://entra.microsoft.com/) with an account allowed to create app registrations.
2. Open **Identity → Applications → App registrations → New registration**.
3. Give the application a clear launcher name. Select the account type that supports personal Microsoft accounts, matching the launcher's `/consumers` authority. Microsoft's [app registration quickstart](https://learn.microsoft.com/en-us/entra/identity-platform/quickstart-register-app) describes the available account types. If the portal does not offer a compatible choice, confirm the registration design before selecting a broader type.
4. Register the application and copy its **Application (client) ID**. This GUID is public configuration, not a password.
5. Open **Authentication**. Under **Advanced settings**, set **Allow public client flows** to **Yes**, then save. Device-code clients do not use a client secret or a browser redirect URI.
6. Do not create a client secret, and do not put access or refresh tokens in launcher settings.
7. Confirm the current Xbox and Minecraft service access requirements for your application. Registration alone does not prove that the complete sign-in flow will work. If a service rejects the application, resolve access using the returned error and that service’s official support process.

The desktop configuration guide explains how to enable public-client flows. Device-code clients do not need a redirect URI. Registration and successful earlier exchange stages do not establish public-user API approval.

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

1. Describe authentication and provider scope accurately. Saved GUI offline profiles and CLI offline identity remain supported. Microsoft sign-in is temporarily unavailable in the GUI; the CLI integration remains experimental and genuine-account acceptance is deferred. Do not infer live CurseForge authorization from fixtures or a local key save. Inspect current all-platform performance artifacts; use the paired Linux comparison against a selected baseline when investigating a suspected regression.
2. Start from a clean working tree, fetch the remote refs, and integrate the accepted `dev` commit into `main` only when a formal stable release is explicitly authorized. Preserve any independent local-main changes and resolve/review merge conflicts; do not reset or force-push the branch. Set the `version` fields in `crates/ccl-core/Cargo.toml`, `crates/ccl-cli/Cargo.toml`, and `crates/ccl-gui/Cargo.toml` to the same release version; also synchronize `crates/ccl-gui/tauri.conf.json` and `apps/ccl-gui/package.json`. GUI and CLI versions must equal the stable tag. Refresh and commit `Cargo.lock`, then confirm all versions agree:

   ```sh
   git fetch origin
   git switch main
   git pull --ff-only origin main
   git merge --squash origin/dev
   # Resolve/review any conflicts, then edit all three package versions if needed.
   cargo check --workspace
   python3 scripts/check-package-versions.py --expected X.Y.Z
   test "$(python3 -c 'import tomllib; print(tomllib.load(open("crates/ccl-gui/Cargo.toml", "rb"))["package"]["version"])')" = X.Y.Z
   test "$(python3 -c 'import json; print(json.load(open("crates/ccl-gui/tauri.conf.json"))["version"])')" = X.Y.Z
   test "$(node -p 'require("./apps/ccl-gui/package.json").version')" = X.Y.Z
   ```

Review `git diff -- Cargo.lock` after the check and keep only lockfile changes required by the workspace version change. Do not refresh unrelated dependency versions as part of release preparation.

3. Before validating a stable release, update both README editions, the user guides and API/development availability statements to describe the intended published version; keep any unpublished draft clearly marked until publication. Run the local validation suite and check the diff. These are the release workflow's source checks for the host target; the hosted workflow repeats them on every release target.

   ```sh
   cargo fmt --all -- --check
   cargo clippy --workspace --all-targets --locked -- -D warnings
   cargo test --workspace --all-targets --locked
   cargo build --workspace --release --locked
   git diff --check
   ```

4. Commit and push the version change and release-ready source to `main`. If the selected version is already correct and the merge contains no additional version changes, skip the version-only add/commit commands below. Stage only reviewed paths, including the package version manifests and lockfile. Confirm the intended commit is reachable from `origin/main`. Then create and push the exact matching tag:

   ```sh
   git add Cargo.lock crates/ccl-core/Cargo.toml crates/ccl-cli/Cargo.toml crates/ccl-gui/Cargo.toml crates/ccl-gui/tauri.conf.json apps/ccl-gui/package.json
   git commit -m "chore: prepare vX.Y.Z"
   git push origin main
   git fetch origin main
   git merge-base --is-ancestor HEAD origin/main
   # Wait for the main-branch CI to pass before creating the version tag.
   git tag -a vX.Y.Z -m "CCL vX.Y.Z"
   git push origin vX.Y.Z
   ```

Pushing a stable tag does not start or publish a release. After the release owner has explicitly approved the stable release and pushed its unique `vX.Y.Z` tag, open Actions → Stable Release → Run workflow with that tag selected and enter the exact confirmation `PUBLISH`. That manually dispatched run checks the tag is on `main`, requires `ccl-core`, `ccl-cli`, and `ccl-gui` versions to match, validates sources, builds both CLI ZIPs and GUI installers on the three target hosts, and attempts a GUI smoke launch on each. Only after these jobs pass does it create a draft, sign both manifest families, download and verify the release assets, and publish. Stable version tags must be unique and must never be moved to a different commit. A dev prerelease does not constitute approval for a future stable release; do not merge unreleased changes to `main`, create a new stable tag, or publish without the user's explicit approval.

Before tagging, package one host build locally with the corresponding script if useful. The CLI update package is a ZIP on all three platforms; GUI packages use the formats listed in [Desktop GUI and package updates](#gui-g3):

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

An ordinary local build has an unpublished commit identity unless the release build environment is supplied; use it for local checks, not as a substitute for the workflow-produced update package. The CLI package scripts require the appropriate `target/<triple>/release/ccl` executable (`ccl.exe` on Windows). At the archive root, they include the project license, both READMEs, each paired English/Simplified Chinese guide, generated `THIRD-PARTY.en.md` / `THIRD-PARTY.zh-CN.md` inventories, and paired `THIRD-PARTY-LICENSES.en.md` / `THIRD-PARTY-LICENSES.zh-CN.md` bundles containing the original license and notice texts. `scripts/third-party-notices.py` walks the target-specific normal and build dependency graph of `ccl-cli`; each inventory links to deterministic anchors in its matching language bundle. The bundles localize their introductory labels while retaining identical original license text bytes; third-party texts are consolidated into these two files. The packaging scripts verify all four notice files are present and the ZIP has no more than 256 entries, keeping it installable by current readers. The updater's 256-entry ZIP limit remains unchanged. GUI packages also embed generated third-party license and notice material for Rust and production frontend dependencies. Package scripts write SHA-256 files. CLI ZIPs and GUI installers are unsigned by OS publisher certificates; the separate signed manifests authenticate metadata and package digests for updater clients.

### Developer builds

These guides target the v0.2.3 GUI and CLI release. GitHub Releases is authoritative for package availability. Developer prereleases may contain newer changes.

Push a validated commit to `dev` to start `.github/workflows/release-dev.yml`:

```sh
git switch dev
git pull --ff-only origin dev
# make and commit the intended changes
git push origin dev
```

The workflow requires the tested commit to remain the current `dev` tip while all three platform jobs validate, package and attempt the GUI smoke launch. On success it creates immutable tag `dev-<full commit SHA>`, verifies the CLI ZIPs, GUI installers, checksums and signed manifests, publishes a prerelease, and retains the newest ten matching developer releases. Do not create or push that tag manually. If another commit reaches `dev` during validation, no release for the stale tip should be published; validate the new tip instead. A dev prerelease is not formal stable-release approval.

### Troubleshooting release gates

- **Stable tag rejected:** confirm the tag commit is an ancestor of `origin/main`, core/CLI/GUI Cargo versions and GUI package metadata equal the tag without the `v`, and `Cargo.lock` is committed. The checker is `python3 scripts/check-package-versions.py --expected X.Y.Z` plus the GUI metadata checks in the release checklist.
- **`--locked` reports a lockfile change:** update all three Rust package versions, synchronize the Tauri config and frontend package versions, run `cargo check --workspace`, inspect and commit only required `Cargo.lock` changes, then repeat locked checks.
- **Native link/build failure:** install the target-specific compiler/linker and SDK listed under [Environment](#environment). A host build does not prove another target can link.
- **Credential vault probe fails:** unlock the OS credential service. On Linux, ensure a persistent Secret Service/D-Bus session is available; CCL deliberately returns an error instead of saving credentials in plaintext.
- **Developer release is absent:** inspect all three validation jobs and check whether `dev` moved past the tested commit. A failed or stale-tip job does not produce an immutable package.
- **Hash check passes:** this confirms byte integrity against a digest. Updater clients also verify the detached manifest signature; this remains separate from operating system publisher signing.

<a id="release-notes"></a>

## v0.2.3 release notes

These notes target v0.2.3 and are prepared for the authorized stable-release workflow. GitHub Releases is authoritative for publication status. Original v0.2.0 assets remain unchanged.

<!-- release-notes:start -->
# CialloCraftLauncher 0.2.3

This patch fixes launching existing Forge modpacks and preserves each profile’s runtime data.

- Expand HMCL’s `${primary_jar_name}` placeholder using the actual client JAR filename, including inherited JAR overrides, Unicode names and spaces.
- Separate shared resource lookup from the game runtime directory. Existing profiles with local mods, configuration or saves run in their own directory; explicit per-profile HMCL directory choices take precedence. Game data is not moved.
- Preserve different asset indexes published under the same ID by downloading and verifying a separate hash-specific index. Offline launch reuses that exact verified cache; existing shared indexes and version metadata are preserved.
- Identify external profiles’ Minecraft versions and loaders from metadata instead of displaying every custom Forge profile as vanilla.
- Record distinct, non-sensitive launcher error categories for unsupported placeholders, other launch-plan failures and Java failures.

Validation before publication: 378 workspace tests passed with 9 ignored, desktop-feature compilation and warnings-denied Clippy passed. Four real macOS Forge profiles passed online and offline launch preparation (eight successful preparations). These checks did not enter a game world or establish successful Minecraft main-menu startup for every profile. The stable workflow additionally validates and packages Windows x64, macOS arm64 and Linux x64.

CLI and GUI updater manifests have detached release-key signatures. Installers do not have OS publisher certificate signatures; the macOS bundle retains ad-hoc signature verification. Microsoft login availability and user-supplied CurseForge key requirements are unchanged.
<!-- release-notes:end -->

### v0.2.2 historical release notes

#### CialloCraftLauncher 0.2.2

This patch unifies the desktop library and stores display names and hidden state in separate metadata.

- Show discovered Minecraft versions under the selected root and existing CCL game directories together in one library. Discovery and launch use existing paths; game directories are not moved or converted.
- Rename a library display name, hide an entry from the library, and restore hidden entries in Settings. Display names and hidden state live in `.ccl/library.json`; rename keeps the game ID/path and hiding does not delete game files.
- Remove shared/managed categories and the shared-install toggle from the desktop UI. Revision, loader and content actions depend on whether a directory already has CCL revision storage; entries without it are not automatically enrolled in full lifecycle management.
- Use `instance:<64 lowercase hexadecimal characters>` library keys. Earlier source-prefixed GUI keys and the old `shared` install request field are no longer accepted.
- Keep language selection in Settings, including before initial directory setup, and remove the desktop title-bar version/badge label.
- Align the English and Simplified Chinese guides while retaining the original v0.2.0 macOS signature limitation and the v0.2.1 bundle-signing fix.

Package targets are Windows x86_64 (CLI ZIP / GUI NSIS), macOS arm64 (CLI ZIP / GUI DMG and updater archive), and Linux x86_64 (CLI ZIP / GUI AppImage and DEB). CLI and GUI updater manifests authenticate their separate payloads with detached signatures; packages do not have OS publisher certificate signatures. Linux DEB updates use the package manager.

The new GUI display names and hidden state use sidecar metadata; existing game files are not migrated or rewritten. Genuine-account Minecraft acceptance remains deferred, and CurseForge requires a user-supplied authorized API key. Hosted CI and controlled launch fixtures do not establish physical Minecraft acceptance on every platform.


### v0.2.1 historical release notes

# CialloCraftLauncher 0.2.1

This patch improves the desktop workflow and corrects macOS bundle integrity.

- Instances support UTF-8 names, including Chinese, Japanese and spaces. Library discovery and launch use consistent filename validation, fixing `invalid_launch_key` for existing named profiles. Windows filename restrictions remain in effect.
- Add, select and remove saved offline accounts in Accounts; the library selects these accounts directly. Offline profiles never use the credential vault. Microsoft sign-in is temporarily unavailable in the GUI, and its public client ID is in Settings → Advanced. The experimental CLI Microsoft flow remains available.
- Downloads, installation, import and other task operations use a collapsible progress dialog. The Tasks page is removed, progress recovers missed completion events, and a running game collapses the dialog automatically.
- Discover, instance tools and Settings display readable results instead of raw JSON or debug status fields. Redundant instance text and the sidebar slogan are removed.
- Separate CCL launcher logs from game logs, with bounded rotation, optional navigation logging and local export. Persistent launcher logs record allowlisted events without arbitrary request data, credentials or user paths.
- macOS packaging applies a complete ad-hoc bundle signature and verifies the built app, DMG and update archive. This corrects the incomplete signature in the original v0.2.0 assets. Apple Developer ID signing and notarization are still unavailable; macOS may require per-app approval.
- English and Simplified Chinese guides are aligned with these changes.

Packages cover Windows x86_64 (CLI ZIP / GUI NSIS), macOS arm64 (CLI ZIP / GUI DMG and updater archive), and Linux x86_64 (CLI ZIP / GUI AppImage and DEB). CLI and GUI updater manifests authenticate their separate payloads with detached signatures; packages do not have OS publisher certificate signatures. Linux DEB updates use the package manager.

Existing launcher configuration, accounts and instance metadata remain readable without a schema migration. Back up important worlds before operations that replace content. Genuine-account Minecraft acceptance remains deferred, and CurseForge requires a user-supplied authorized API key. Hosted CI and controlled launch fixtures do not establish physical Minecraft acceptance on every platform.

`scripts/release-notes.py` extracts the marked English and Chinese bodies, checks that their titles match the requested package version, and combines them for `.github/workflows/release-stable.yml` to use as the GitHub release notes. Keep both marked sections accurate and their version titles synchronized with the Cargo package version. These notes target v0.2.3; publication requires the approved stable workflow. Review current all-platform performance artifacts before a future release; use the paired Linux comparison with a selected baseline when investigating a suspected regression. Historical acceptance results do not establish performance for a release commit.

<a id="scope"></a>

## Current feature scope

The core provides configuration, directory selection, HTTP downloads, version lookup and installation, managed Java runtime installation under `.ccl/runtimes`, the experimental Microsoft account authentication protocol (not currently exposed by GUI sign-in), offline identity launch, launch planning, native extraction, child-process output capture, Modrinth pack handling, CurseForge and MultiMC/Prism archive conversion, Modrinth/CurseForge online content operations, isolated instance lifecycle and retained revision cleanup, Fabric/Quilt/Legacy Fabric/Forge/NeoForge/OptiFine loaders, local content management, and userdata backups. Pack receipts record hashes for pack-owned paths; updates use those hashes to detect local edits, deletions, and collisions before replacing data. Disk-usage reporting counts logical bytes and does not promise physical space reclaimed when hard links exist. Userdata backups are separate from Modrinth exports, which omit saves and private server lists and require explicit redistribution consent for local binaries. CurseForge API key support does not imply that a real key has production authorization. Microsoft online sign-in is experimental; the owner deferred genuine-account acceptance on 2026-10-23, while offline identity remains supported. Keep the core independent from interfaces and update the English and Chinese user documentation together when changing public behavior.
