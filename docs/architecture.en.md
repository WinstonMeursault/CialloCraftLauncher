# Architecture and design boundaries

## Goals

CialloCraftLauncher starts with a command-line interface. The reusable Rust core provides configuration, directory, network, download, and installation logic; the CLI and a future graphical interface call the same core services. The core does not depend on a terminal or graphical interface, so adding a GUI later will not require moving business logic into the interface layer.

Initial target platforms:

- Windows x86_64
- macOS arm64
- Linux x86_64

## Workspace

- `crates/ccl-core`: shared services, configuration, errors, HTTP transport, task events, installation, Java discovery, and launch orchestration.
- `crates/ccl-cli`: command-line arguments, text or JSON output, exit codes, and calls into the core.
- `docs`: public documentation for users and contributors.

The core reports progress through task contexts and provides a cooperative cancellation handle. An interface displays events and collects input; the core performs work, validates configuration, and returns structured errors. The core must not depend on an interface.

## Game directory selection

The default game directory is selected in this order:

1. An explicit `--game-dir` path.
2. An existing `.minecraft` directory in the current working directory.
3. The platform's conventional Minecraft data directory; an existing directory can be reused on macOS.
4. `.minecraft` in the user's home directory.
5. Create `.minecraft` in the current working directory.

`doctor` checks configuration and directory state without creating or modifying the game directory. `init` selects and creates the game directory using this order.

The CLI also provides `versions`, which reads the official version manifest and accepts `--limit N` to limit results (`0` means all versions), and `install VERSION`, which installs a selected version in the game directory. `settings` reads, validates, updates, and resets typed user configuration. A cross-process lock serializes each read-modify-write transaction; changes are validated before an atomic file replacement. The global `--network-offline` option disables HTTP and allows use of cached, verified data; this is separate from choosing an offline launch identity. Resumable downloads continue only when ETag and range-response checks match.

`instances` lists installed vanilla versions and their Java requirements. `launch VERSION` selects a Java runtime matching the metadata's required major version and host architecture, prepares and verifies installed files, and launches with the selected Microsoft account or an explicitly requested offline identity. Missing Java can be installed beneath `.ccl/runtimes` using Mojang's official component manifests and verified artifact hashes. Microsoft access and refresh tokens use the operating system credential store; metadata is kept separately. On Linux, persistent Secret Service/D-Bus support is required and there is no plaintext fallback. Account login is implemented but live application approval and end-to-end validation are still outstanding; see [account setup](accounts.en.md). It captures both process output streams and writes a run log plus a JSON result record beneath `.ccl/logs`. `logs` reads those records' text logs; `--dry-run` prepares a structured launch plan and cleans up temporary native files without starting the game. Launch and installation behavior is implemented in the shared core, independently of the GUI.

## Download sources

Version lists and game files have separate source settings, each supporting `auto`, `official`, and `mirror`. Both default to `auto`; the mirror root can be set in the user configuration. The current version listing and installation workflows use the official manifest and version metadata. Installation downloads the client, libraries, native libraries, asset index, and assets. It validates files against available upstream hashes and sizes, bounds concurrent downloads, retries transient failures, resumes transfers only after ETag and range checks, reuses valid cached files, and writes through temporary files before replacement. A cross-process version lock prevents competing installations from writing the same version simultaneously. Tasks support cooperative cancellation. Existing version JSON that conflicts with the requested metadata is reported as a conflict instead of being replaced. NeoForge installer verification can use its official SHA-512 checksum when the SHA-1 sidecar is unavailable; the pipeline derives SHA-1 only after verifying those bytes.

The user configuration is JSON and defaults to:

- Windows: `%APPDATA%/CialloCraftLauncher/config.json`
- macOS: `~/Library/Application Support/CialloCraftLauncher/config.json`
- Linux: `$XDG_CONFIG_HOME/ciallocraftlauncher/config.json`, or `~/.config/ciallocraftlauncher/config.json` if the variable is unset

Configuration fields include download preferences, optional `auth.microsoft_client_id` (a public client ID), and `launch.java`, `launch.max_memory_mb` (default 2048), and `launch.auto_install_java` (default true). Missing fields use defaults for backward compatibility. The mirror root must be an HTTPS URL without credentials, and unknown fields are rejected. Proxy credentials are redacted in displayed settings. On Unix, config files written by the CLI are owner-only. If the default configuration file is missing, defaults are used; a missing file explicitly supplied with `--config` is an error.

## Isolated modded instances

The `loaders` service discovers compatible Fabric, Quilt, Legacy Fabric, Forge, NeoForge, and OptiFine releases and installs supported profiles. The `forge` engine interprets modern installer client processors, reports output through task events, and checks declared output hashes. Forge supports processor based and supported legacy metadata formats; NeoForge uses processor based installers. Inherited profiles retain the vanilla client archive's owning directory; child library versions override matching parent artifacts while keeping distinct native classifiers. Provider compatibility metadata is a preflight check, not proof that all third-party mod combinations launch successfully. See [loaders and instances](loaders.en.md).

The `instances` service owns `<game root>/.ccl/instances/<id>/game`. Installation and update build a separate staging revision and commit only after success. A journal supports recovery from an interrupted revision switch; the previous successful revision remains available for rollback. Loader updates and loader removal preserve current user data. Pack imports/updates and content edits use the same staged-generation transaction and cross-process instance lock as launches, so a running game cannot be replaced underneath a mutation.

Instance lifecycle operations can rename, copy, delete, list and restore retained revisions, report logical disk usage, and preview or apply cleanup of archived revisions and unfinished stages. Cleanup keeps active worlds/content out of scope by design; hard links mean reported logical bytes are not physical disk usage or guaranteed reclaimed space. See [instance lifecycle](instances.en.md).

Archive import recognizes Modrinth, CurseForge and MultiMC/Prism exports. CurseForge remote file resolution uses the official API and requires a user supplied API key; fixture tests for request scoping do not establish live API authorization. MultiMC/Prism import accepts supported vanilla/loader components and local game content, and rejects unsupported custom patches. These conversions do not execute archive hooks or import account databases. See [archive imports](imports.en.md).

The online content service supports Modrinth and CurseForge search, version lookup, dependency resolution, update checks, conflict analysis, and transactional install/update operations. CurseForge key availability or header-scoping fixtures are not evidence that a real key has production authorization. See [online content](online-content.en.md).

The `modpack_format` module reads and writes Modrinth format version 1 archives, validates manifest fields and destination paths, limits ZIP expansion, and applies `overrides/` with client overrides layered on top. `modpacks` imports selected files and overrides transactionally and stores a receipt with hashes of pack-owned paths. Updates compare current files with those ownership hashes and stop on modified/deleted owned files or collisions with unowned local data unless the caller explicitly authorizes replacement. After a pack or content change, `instance rollback` restores the prior full instance snapshot, including userdata and saves; users should back up first when they need to retain newer saves. `content` manages local content and portable userdata backups. Backups include user data and saves but exclude game/loader files, Java runtimes, and account credentials. Modrinth exports are separate: they exclude saves and private server lists, and local binary files require explicit redistribution authorization. See [Modrinth packs and instance content](modpacks.en.md) for the commands and safety details.

## CLI and GUI consistency

The CLI and GUI share core operations, configuration, and error definitions. The CLI must not implement separate download or installation rules, and the GUI must not require the core to start a terminal or read standard input. `--json` formats command execution results as JSON. clap help and argument parsing errors remain text. CLI commands run without interactive input.

The core provides configuration, directory selection, HTTP transport, task events and cancellation, version listing, installation, managed Java runtime installation, account storage and authentication, installed-version listing, launch preparation and process capture, managed instance lifecycle, pack/archive import, online content operations, and loader installation. See [Microsoft account setup](accounts.en.md) for registration and credential-storage requirements. Live Microsoft approval and authentication validation remain outstanding; a public account-enabled preview is not ready for distribution.
