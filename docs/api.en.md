# API reference

[User guide](user-guide.en.md) · [Development guide](development.en.md) · [简体中文](api.zh-CN.md)

- [CLI commands](#cli)
- [JSON output](#output)
- [Core service index](#core)
- [Events, cancellation and errors](#tasks)
- [Online content contracts](#online-contract)
- [Updates, compatibility and diagnostics](#maintenance)
- [Exit codes](#exit-codes)

<a id="cli"></a>

## CLI commands

```text
ccl [--json] [--network-offline] [--game-dir PATH] [--config PATH] <doctor|diagnose|update|init|settings|account|versions|install|instances|loaders|instance|pack|content|online|launch|logs>
```

Global options can be placed before the subcommand. Commands run non-interactively and do not wait for standard input.

| Command | Behavior |
| --- | --- |
| `doctor [--network-probe] [--write-probe]` | Read-only checks of configuration, Java, directory access and credential-store capability. Network and temporary write probes require explicit flags; it does not create the game directory. |
| `diagnose [--run ID] [--export PATH] [--include-log-preview]` | Classifies a saved run or writes a local, allowlisted diagnostic export. Raw log preview requires explicit opt-in and may disclose secrets. |
| `update check|apply [--channel stable|dev]` | Checks or stages a verified package from the selected GitHub channel. `apply` starts a helper and completes after the CLI exits. |
| `update status|rollback|recover` | Inspects the update transaction, starts a compatible rollback, or resumes interrupted helper work. |
| `init` | Selects and creates a game directory using the [directory priority](user-guide.en.md#directories). |
| `settings show\|get\|set\|reset` | Displays, reads, changes, or resets persisted settings. Proxy credentials are hidden in displayed settings. |
| `account check-storage\|login\|list\|select\|refresh\|logout` | Checks the OS credential vault, signs in with a Microsoft account using device code, lists/selects accounts, refreshes, or removes local credentials. Login checks the vault before contacting Microsoft, prints the device code and opens the default browser; `account login --no-browser` disables opening. See [Microsoft account setup](user-guide.en.md#accounts). |
| `versions [--limit N]` | Lists available game versions. `--limit N` limits the result; `--limit 0` lists all versions. |
| `install VERSION` | Installs the named game version into the selected game directory. |
| `instances` | Lists installed vanilla versions and their required Java major versions. |
| `loaders fabric\|quilt\|legacyfabric\|forge\|neoforge\|optifine MINECRAFT [--limit N]` | Lists loader releases advertised for the requested Minecraft version. |
| `instance list\|show ID\|install ID MINECRAFT LOADER LOADER_VERSION\|update ID MINECRAFT LOADER LOADER_VERSION\|rollback ID\|uninstall-loader ID` | Manages isolated modded instances and loader revisions. See [modded instances and loaders](user-guide.en.md#loaders). |
| `instance rename ID DESTINATION\|copy ID DESTINATION\|delete ID --yes\|history ID\|restore ID REVISION\|disk-usage ID\|clean ID [--keep N] [--include-previous] [--apply]` | Renames, copies, deletes, inspects, restores, and cleans retained instance revisions. Cleanup is preview-only unless `--apply` is given; see [instance lifecycle](user-guide.en.md#instances). |
| `pack inspect ARCHIVE\|show ID\|import ID ARCHIVE [--include-optional] [--replace-modified] [--allow-host HOST]\|export ID OUTPUT [--include-local-content]` | Inspects and imports Modrinth, CurseForge, and MultiMC/Prism exports; exports Modrinth `.mrpack` packs. See the [import guide](user-guide.en.md#imports) and [Modrinth and instance content guide](user-guide.en.md#packs). |
| `content list ID KIND\|add ID KIND SOURCE [--name NAME]\|remove ID KIND NAME\|enable ID NAME\|disable ID NAME\|backup ID OUTPUT\|restore ID ARCHIVE` | Manages local mods, resource packs, shader packs, saves, and portable userdata backups. `KIND` is `mods`, `resourcepacks`, `shaderpacks`, or `saves`. |
| `online search\|versions\|resolve\|install\|updates\|analyze\|pack-install` | Searches Modrinth only, resolves dependencies on applicable providers, installs or updates content, and analyzes recorded online content. See [online content](user-guide.en.md#online). |
| `launch [VERSION] [--instance ID] [--username CCLPlayer] [--offline] [--account ID] [--java PATH] [--memory MiB] [--no-java-download] [--dry-run]` | Prepares or launches a vanilla version or an isolated instance. Uses the selected Microsoft account by default, or explicitly uses an offline profile. Java may be detected or installed from Mojang's runtime manifests. |
| `logs [--instance ID] [--run ID] [--lines 100]` | Prints the most recent run log, or the selected run's log, limited to the requested number of lines. `--instance` selects a managed instance's logs. |

Launch selects Java matching the version metadata's required major version and host architecture. Offline player UUIDs use the Java offline-player name-based UUID algorithm. Before launch, the launcher verifies the client JAR, applicable library artifacts, and asset index against available size and SHA-1 metadata. It prepares the asset projection, downloads and verifies the client logging configuration when specified, and extracts platform native libraries to a temporary directory. That directory is removed after the game exits; dry-run also removes it after printing the plan.

Managed instances use separate game directories and can be launched with `ccl launch --instance ID`; do not provide a positional version in this form. Loader compatibility is checked against upstream metadata before install, but does not guarantee compatibility with every mod set or prove every game launch combination. Fabric API and other mods are not bundled. Forge supports modern processor installers, spec 0 profiles, and the supported legacy `install`/`versionInfo` universal-JAR format; NeoForge uses modern processor installers. Loader updates and loader removal preserve current instance userdata. After a loader update, `instance rollback` stages current userdata into the prior loader generation; after a full content edit or pack update, rollback swaps the complete previous snapshot, including userdata and saves. Back up first if you need newer saves. `instance show` reports the resolved installed profile ID in `loader_version`, which can differ from the loader release string given to the command. Details and examples are in [loader workflows](user-guide.en.md#loaders).

Pack updates check ownership hashes before replacing or removing files. Modified or deleted pack-owned files and a new pack path that collides with unowned local data stop the update with a conflict (exit code `7`); `--replace-modified` explicitly allows those local changes to be replaced or removed. Back up local data before using it. `pack export --include-local-content` includes local binary files only when you explicitly authorize redistribution. Content backups are the portable way to save userdata, including saves; they are separate from Modrinth exports.

Game stdout and stderr are captured in `<game-directory>/.ccl/logs/<run-id>.log`. A JSON result record is written alongside it as `<run-id>.json`; it includes the version, process ID, actual game `exit_code`, cancellation state, log path, and start and finish timestamps. The `logs` JSON response has a `log` field containing the text. Microsoft account credentials are kept in the operating system credential store, separately from account metadata. On Linux, a persistent Secret Service and D-Bus session must be available; there is no plaintext fallback. Logging out removes local credentials but does not revoke Microsoft consent. `--offline` or `--username NAME` explicitly selects an offline identity.

`--game-dir PATH` explicitly selects the game directory and takes precedence over default selection. `--config PATH` selects a configuration file. If omitted, the platform default is read and defaults are used when it is missing. A missing file explicitly selected with `--config` is a filesystem error (exit code `4`), except that `settings set` and `settings reset` can create it.

`--json`, `--network-offline`, `--game-dir`, and `--config` are global options and can be placed before the subcommand. `--network-offline` disables HTTP requests and requires verified local data; it is separate from `launch --offline`, which selects an offline player identity. Downloads resume only when the saved strong ETag and server range response validate the partial transfer. `--json` outputs command execution results as JSON. Progress goes to standard error. clap-generated help and argument parsing errors remain text.

`update --channel` persists the selected channel. Stable uses `main` and version tags of the form `v<VERSION>`; developer builds follow `dev` and are published only after all three platform validation/package jobs succeed. The updater binds metadata to repository, branch, commit, target and data-reader range, then checks archive size, SHA-256 and paths before staging. Packages are unsigned: HTTPS and hashes detect corruption but do not independently authenticate the publisher. `apply` exits after launching a restricted helper; use `update status` to inspect completion, and `update recover` after an interrupted transaction. `update rollback` first checks whether the retained executable can read current launcher metadata. It does not restore the game directory. Update commands require network access except `status`, `rollback` and recovery; `--network-offline` prevents update checks and downloads.

`doctor` does not probe the network or write by default. `--network-probe` checks the official version metadata endpoint; `--write-probe` creates and removes a temporary empty file under launcher data. `diagnose --run ID` reads the saved run record and a bounded log prefix to classify known failure patterns. `diagnose --export PATH` creates a new local file containing allowlisted structured fields. Adding `--include-log-preview` includes up to 16 KiB of raw game output; review it before sharing because arbitrary game/mod logs may contain secrets. No report is uploaded.

<a id="output"></a>

## JSON output

Commands return an object with `ok`. Successful payload fields depend on the command, for example `versions`, `instance`, `managed_instances`, `launch`, and `log`. Errors contain `error.code` and `error.message`. Progress goes to stderr. clap help and argument errors remain text even with `--json`.

```json
{"ok":false,"error":{"code":"content_conflict","message":"..."}}
```

The `launch` record includes `run_id`, `version`, `pid`, `exit_code`, `cancelled`, `log_path`, `started_at_ms`, and `finished_at_ms`. `logs` returns text in `log`. See the [user guide](user-guide.en.md) for workflows and configuration.

<a id="core"></a>

## Core service index

The reusable interfaces are exported by `ccl-core` (`ccl_core` in Rust). This is an early-development library: check generated Rust documentation for exact signatures and types at the checkout you use.

```sh
cargo doc -p ccl-core --no-deps --open
```

| Service/module | Main methods | Responsibility |
| --- | --- | --- |
| `config::ConfigStore` | `get / set / reset / update` | Validated, lock-serialized settings |
| `install::InstallService` | `list_versions / list_versions_for / install` | Manifest lookup, per-root offline cache and vanilla installation |
| `loaders::LoaderService` | `versions / install` | Loader metadata and installation |
| `instances::InstanceService` | `list / metadata / install / acquire_launch / rollback / uninstall_loader` | Managed instances and launch guards |
| `instances::InstanceService` | `rename / duplicate / delete / history / restore_revision / disk_usage / cleanup` | Lifecycle, retained revisions and cleanup |
| `launch::LaunchService` | `prepare / launch` | Java selection, verified preparation and process orchestration |
| `launch` | `list_installed_versions / read_logs` | Installed vanilla versions and recorded logs |
| `modpacks::ModpackService` | `inspect / show / import / export` | Modrinth archives and ownership receipts |
| `import_formats::ImportFormatService` | `inspect / import` | CurseForge and MultiMC/Prism conversion |
| `content::ContentService` | `list / add / remove / set_enabled / backup / restore` | Local content and portable userdata backups |
| `online_content::OnlineContentService` | `search / versions / install_project / install_versions / updates_for_instance / analyze_instance / download_and_import_pack` | Provider content, dependency resolution and transactions |
| `auth::MicrosoftAuth` | `begin_device_login / complete_device_login / refresh / launch_identity` | Device authorization and launch identity |
| `accounts::AccountStore / accounts::OsSecretStore` | `list / select / credentials / remove; probe` | Account metadata and OS-protected credentials |
| `compatibility::inspect_compatibility` | `inspect_compatibility(config_path, game_root)` | Read-only compatibility report for config, account metadata, instances and receipts; no credential access |
| `update::UpdateService / update::Installation` | `check / stage; status / set_channel / prepare_rollback / launch_helper` | Release checks and executable update transaction |
| `diagnostics::DiagnosticsService` | `doctor / diagnose_run / export` | Local health checks, bounded run classification and allowlisted exports |

<a id="tasks"></a>

## Events, cancellation and errors

Pass an optional `TaskContext` to operations that accept one. `EventSink` is an `Arc<dyn Fn(TaskEvent) + Send + Sync>` supplied by the interface. Clone the `CancellationToken` to request cancellation; keep an instance's launch guard alive while its game is running.

`TaskEvent` uses the serialized tag `kind` and snake-case variants: `started`, `progress`, `finished`, `process_started`, `log`, and `process_exited`. Progress contains `completed` and optional `total`; process events carry a PID or optional exit code. Core services return typed errors; the CLI maps those to the exit codes below.

```rust
use std::sync::Arc;
use ccl_core::{CancellationToken, TaskContext};

let token = CancellationToken::default();
let context = TaskContext::new(token.clone(), Arc::new(|event| {
    // Forward the event to the interface / 将事件转发给界面。
    let _ = event;
}));
// Pass Some(context) to an operation / 将 Some(context) 传给操作。
token.cancel();
```

<a id="online-contract"></a>

## Online content contracts

`ccl-core::online_content::OnlineContentService` exposes provider search, compatible versions, dependency resolution, update checks, conflict analysis, and install operations without a CLI dependency.

### Providers

- Modrinth uses its public API for projects, versions, and required dependency resolution. Exact pinned dependency versions are retained and checked against the selected Minecraft version and loader. Optional dependencies are listed but not installed automatically; incompatible dependencies are reported during analysis.
- CurseForge uses its REST API. The caller must provide its own API key through `OnlineConfig`; the launcher does not include or persist the key. Download requests attach the key only to the exact HTTPS CurseForge CDN hosts allowed by the download request. If the API omits a file's download URL, that file is left unavailable; the launcher does not guess a website URL.

Modrinth search can be filtered by project type, Minecraft version, and loader; the core `search` service queries only Modrinth. Both providers expose version files with available hashes and sizes. CurseForge's numeric project IDs are retained as strings in the provider-neutral API.

### Instance operations

`install_project` selects a version compatible with the target instance, resolves required dependencies, downloads and verifies all selected files, then applies the set in one `InstanceService::edit_content` revision. It writes `.ccl/online-content.json`, recording project versions and SHA-512 ownership hashes. Other provider projects and local mods remain in place. Updates replace only files owned by the updated projects and remove obsolete owned files after checking that their bytes still match the receipt. A modified or missing owned file stops the update with a conflict so the active generation is preserved.

`updates_for_instance` checks receipt projects against their providers. `analyze_instance` uses the version metadata stored in the receipt and reports missing required dependencies, incompatible selections, duplicate project versions, and different files claiming the same filename. Modpack versions can be downloaded and passed to the existing Modrinth or CurseForge import converter.

<a id="maintenance"></a>

## Updates, compatibility and diagnostics

The updater fetches from this project's GitHub repository only. Stable releases are created from a `v<VERSION>` tag on `main`; developer releases are created for `dev` commits after Windows x64, macOS arm64 and Linux x86_64 validation succeeds. It retains the ten newest developer releases. Update manifests declare data schema range `1..=1`; the current readers preserve existing schema-less config, account metadata and instance metadata, and accept receipt schema 1. Unknown future fields in strict metadata or future receipt schemas block compatibility and updater rollback.

`compatibility::inspect_compatibility` is read-only. It inspects the supplied config, adjacent account metadata and CCL-owned instance generations/receipts. It does not open the OS credential store or inspect saves and general game files. There is currently no data migration. A future format migration must lock and back up affected metadata, stage and atomically commit the change, support interruption recovery, and reject binary rollback when the retained reader cannot consume the resulting data.

`DiagnosticsService::doctor` checks Java candidates, directory access and whether a credential-store adapter is compiled in; it does not verify that a user vault is unlocked. Network and write probes are opt-in. `diagnose_run` checks a bounded game-log prefix and crash reports whose timestamps fall near the saved run; it reports known categories/evidence and leaves unknown causes unknown. Structured export replaces paths and omits accounts, credentials, worlds and server lists. Raw log preview is a separate opt-in field with best-effort redaction, not comprehensive redaction.

P7 implementation is in progress. Updater and diagnostics APIs are available, while complete three-platform replacement/rollback fault acceptance, physical upgrade evidence, broader performance baselines, OS signing and release acceptance are not yet complete. All current CCL release assets are unsigned; integrity checks do not establish publisher identity.

<a id="exit-codes"></a>

## Exit codes

| Code | Meaning |
| --- | --- |
| `0` | Success; for a launched game, its recorded exit code is also `0`. |
| `2` | Argument error |
| `3` | Configuration error |
| `4` | Filesystem error |
| `5` | Network or installation download error |
| `6` | Invalid or unknown version |
| `7` | Conflict with vanilla metadata, a managed-instance lock, or modpack ownership/update data |
| `8` | Launch preparation or game failure, including a nonzero game exit |
| `9` | Java runtime discovery or validation error |
| `10` | Account or authentication error |
| `11` | Loader installation or metadata error |
| `12` | Managed instance error |
| `13` | Modrinth pack error |
| `14` | Local content or userdata backup error |
| `15` | Online content provider or operation error |
| `16` | Archive import format error |
| `17` | Launcher updater error, such as network, invalid release or recovery failure |
| `18` | Diagnostic inspection, diagnosis or export error |
| `130` | Operation cancelled |

Vanilla metadata conflicts, managed-instance lock conflicts, Modrinth pack conflicts (including ownership/update conflicts), and content or pack operations blocked by an instance lock use exit code `7`. Loader failures, including loader-profile conflicts, and other instance failures use `11` and `12`. Other Modrinth pack and local content errors use `13` and `14`; online-content errors use `15`, archive import format errors use `16`, and cancellation uses `130`.

For a game failure, the JSON result record preserves the game's actual `exit_code`; CLI code `8` reports that the launch did not succeed. With `--json`, command execution results are JSON; clap help and argument parsing errors are text and use the argument-error exit code.
