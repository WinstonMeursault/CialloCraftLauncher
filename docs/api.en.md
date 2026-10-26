# API reference

[User guide](user-guide.en.md) · [Development guide](development.en.md) · [简体中文](api.zh-CN.md)

- [CLI commands](#cli)
- [JSON output](#output)
- [Desktop application](#desktop)
- [Core service index](#core)
- [Events, cancellation and errors](#tasks)
- [Online content contracts](#online-contract)
- [Updates, compatibility and diagnostics](#maintenance)
- [Exit codes](#exit-codes)

<a id="cli"></a>

## CLI commands

```text
ccl [--json] [--network-offline] [--game-dir PATH] [--config PATH] <doctor|diagnose|update|init|settings|credentials|account|versions|install|instances|loaders|instance|pack|content|online|launch|logs>
```

Global options can be placed before or after the subcommand. Commands run non-interactively except `credentials curseforge set`, which requires a terminal and reads the key with echo disabled, including when `--json` is used.

| Command | Behavior |
| --- | --- |
| `doctor [--network-probe] [--write-probe]` | Read-only checks of configuration, Java, directory access and credential-store capability. Network and temporary write probes require explicit flags; it does not create the game directory. |
| `diagnose [--instance ID] [--run ID] [--export PATH] [--include-log-preview]` | Classifies a saved run or writes a local, allowlisted diagnostic export. Raw log preview requires `--export`, is redacted on a best-effort basis, and may disclose secrets. |
| `update check\|apply [--channel stable\|dev]`; `update status\|rollback`; `update recover [--installation PATH]` | Checks or stages a verified package from the selected GitHub channel. `apply` starts a helper to perform the executable replacement asynchronously. `status` inspects the journal, `rollback` starts a compatible rollback, and `recover` resumes helper work for the current executable or the explicitly supplied installation. |
| `init` | Selects and creates a game directory using the [directory priority](user-guide.en.md#directories). |
| `settings show\|get KEY\|set KEY VALUE\|reset [KEY\|--all]` | Displays, reads, changes, or resets persisted settings. Proxy credentials are hidden in displayed settings. For `set`, booleans, numbers, and `null` are parsed as JSON; other values are strings. |
| `credentials curseforge set\|status\|remove` | Enters a key through a hidden terminal prompt, reports whether a key is saved and an environment override exists, or removes the saved key. Keys are scoped to the selected configuration path and stored in the OS credential vault. `CCL_CURSEFORGE_API_KEY` overrides the saved key; removal does not clear that variable. |
| `account check-storage\|login [--client-id CLIENT_ID] [--no-browser]\|list\|select ID\|refresh [ID]\|logout [ID]` | Checks the OS credential vault, signs in with a Microsoft account using device code, lists/selects accounts, refreshes, or removes local credentials. Login checks the vault before contacting Microsoft, prints the device code and opens the verification page in the default browser unless `--no-browser` is set. A client ID can be supplied on the command or in `auth.microsoft_client_id`. See [Microsoft account setup](user-guide.en.md#accounts). |
| `versions [--limit N]` | Lists available game versions. `--limit N` limits the result; `--limit 0` lists all versions. |
| `install VERSION` | Installs the named game version into the selected game directory. |
| `instances` | Lists installed vanilla versions and their required Java major versions. |
| `loaders fabric\|quilt\|legacyfabric\|forge\|neoforge\|optifine MINECRAFT [--limit N]` | Lists loader releases advertised for the requested Minecraft version. |
| `instance list\|show ID\|install ID MINECRAFT LOADER LOADER_VERSION\|update ID MINECRAFT LOADER LOADER_VERSION\|rollback ID\|uninstall-loader ID` | Manages isolated modded instances and loader revisions. See [modded instances and loaders](user-guide.en.md#loaders). |
| `instance rename ID DESTINATION\|copy ID DESTINATION\|delete ID --yes\|history ID\|restore ID REVISION\|disk-usage ID\|clean ID [--keep N] [--include-previous] [--apply]` | Renames, copies, deletes, inspects, restores, and cleans retained instance revisions. Cleanup is preview-only unless `--apply` is given; see [instance lifecycle](user-guide.en.md#instances). |
| `pack inspect ARCHIVE\|show ID\|import ID ARCHIVE [--include-optional] [--replace-modified] [--allow-host HOST]\|export ID OUTPUT [--include-local-content]` | Inspects and imports Modrinth, CurseForge, and MultiMC/Prism exports; exports Modrinth `.mrpack` packs. See the [import guide](user-guide.en.md#imports) and [Modrinth and instance content guide](user-guide.en.md#packs). |
| `content list ID KIND\|add ID KIND SOURCE [--name NAME]\|remove ID KIND NAME\|enable ID NAME\|disable ID NAME\|backup ID OUTPUT\|restore ID ARCHIVE` | Manages local mods, resource packs, shader packs, saves, and portable userdata backups. `KIND` is `mods`, `resourcepacks`, `shaderpacks`, or `saves`. |
| `online search PROVIDER QUERY [--minecraft VERSION] [--loader LOADER]... [--kind KIND] [--limit N]`; `versions PROVIDER PROJECT [--minecraft VERSION] [--loader LOADER]`; `resolve PROVIDER PROJECT MINECRAFT LOADER`; `install ID PROVIDER PROJECT [--version VERSION]`; `updates ID`; `analyze ID`; `pack-install ID PROVIDER PROJECT [--version VERSION] [--include-optional]` | Searches Modrinth or CurseForge, resolves dependencies, installs or updates content, and analyzes recorded online content. Provider is `modrinth` or `curseforge`; see [online content](user-guide.en.md#online). |
| `launch [VERSION] [--instance ID] [--username NAME] [--offline] [--account ID] [--java PATH] [--memory MiB] [--no-java-download] [--dry-run]` | Prepares or launches a vanilla version or an isolated instance. Uses the selected Microsoft account by default, or explicitly uses an offline profile. Java may be detected or installed from Mojang's runtime manifests. |
| `logs [--instance ID] [--run ID] [--lines N]` | Prints the most recent run log, or the selected run's log, limited to the requested number of lines (default `100`). `--instance` selects a managed instance's logs. |

Launch selects Java matching the version metadata's required major version and host architecture. Offline player UUIDs use the Java offline-player name-based UUID algorithm. Before launch, the launcher verifies the client JAR, applicable library artifacts, and asset index against available size and SHA-1 metadata. It prepares the asset projection, downloads and verifies the client logging configuration when specified, and extracts platform native libraries to a temporary directory. That directory is removed after the game exits; dry-run also removes it after printing the plan.

Managed instances use separate game directories and can be launched with `ccl launch --instance ID`; do not provide a positional version in this form. Loader compatibility is checked against upstream metadata before install, but does not guarantee compatibility with every mod set or prove every game launch combination. Fabric API and other mods are not bundled. Forge supports modern processor installers, spec 0 profiles, and the supported legacy `install`/`versionInfo` universal-JAR format; NeoForge uses modern processor installers. Loader updates and loader removal preserve current instance userdata. After a loader update, `instance rollback` stages current userdata into the prior loader generation; after a full content edit or pack update, rollback swaps the complete previous snapshot, including userdata and saves. Back up first if you need newer saves. `instance show` reports the resolved installed profile ID in `loader_version`, which can differ from the loader release string given to the command. Details and examples are in [loader workflows](user-guide.en.md#loaders).

Pack updates check ownership hashes before replacing or removing files. Modified or deleted pack-owned files and a new pack path that collides with unowned local data stop the update with a conflict (exit code `7`); `--replace-modified` explicitly allows those local changes to be replaced or removed. Back up local data before using it. `pack export --include-local-content` includes local binary files only when you explicitly authorize redistribution. Content backups are the portable way to save userdata, including saves; they are separate from Modrinth exports.

Game stdout and stderr are captured in `<game-directory>/.ccl/logs/<run-id>.log`. A JSON result record is written alongside it as `<run-id>.json`; it includes the version, process ID, actual game `exit_code`, cancellation state, log path, and start and finish timestamps. The `logs` JSON response has a `log` field containing the text. Microsoft account credentials are kept in the operating system credential store, separately from account metadata. On Linux, a persistent Secret Service and D-Bus session must be available; there is no plaintext fallback. Logging out removes local credentials but does not revoke Microsoft consent. `--offline` or `--username NAME` explicitly selects an offline identity; `--account` cannot be combined with `--offline` or `--username`; `--offline` and `--username` may be combined. When offline is selected without `--username`, the profile name defaults to `CCLPlayer`.

`--game-dir PATH` explicitly selects the game directory and takes precedence over default selection. `--config PATH` selects a configuration file. If omitted, the platform default is read and defaults are used when it is missing. A missing file explicitly selected with `--config` is a filesystem error (exit code `4`), except that `settings set` and `settings reset` can create it.

`--json`, `--network-offline`, `--game-dir`, and `--config` are global options and can be placed before or after the subcommand. `--network-offline` disables HTTP requests and requires verified local data; it is separate from `launch --offline`, which selects an offline player identity. Downloads resume only when the saved strong ETag and server range response validate the partial transfer. `--json` outputs exactly one command result JSON document on stdout on success or an operational error. Operational errors use `{"ok":false,"error":{"code":"...","message":"..."}}`; `error.code` identifies the error family and `message` is human-readable. Progress and diagnostics go to stderr. During `account login`, device authorization details are emitted as a `kind: "device_authorization"` JSON line on stderr; a browser-open failure can emit a `kind: "browser_open"` line there. These progress lines are not part of the final JSON result. clap-generated help and argument parsing errors remain text.

`update check --channel` and `update apply --channel` persist the selected channel. Stable uses `main` and version tags of the form `v<VERSION>`; developer builds follow `dev` and are published only after all three platform validation/package jobs succeed. Existing CLI ZIP asset names remain `ccl-<target>.zip`; manifests are `manifest-<target>.json` and carry `product: "ccl-cli"`. A detached `manifest-<target>.json.sig` signature covers the exact manifest bytes, including repository, product, channel, version, branch, commit, target, data-schema range, executable size/hash and package size/hash/URL. Tauri's trusted comment includes `timestamp:`, `file:`, and `version:`; the updater authenticates the comment and requires its version to match the manifest. It then checks archive size, SHA-256 and paths before staging. The `v0.1` CLI line predates this manifest schema and must be manually replaced with the first signed-manifest release. The manifest and its digest are cryptographically signed; the CLI ZIP is not OS publisher-signed. The updater uses an exclusive OS file lock in the installation’s `.ccl-update` directory to serialize channel changes, staging, status reads, rollback preparation, and helper transactions. A competing operation fails with an updater busy error (exit code `17`). The lock is released when that process or helper exits. `apply` returns after launching a restricted helper. The helper attempts to replace the executable, runs health checks, and commits or restores the retained executable; on Windows it retries temporary sharing or permission failures for up to 30 seconds. `update status` returns `update_status: null` when no transaction exists; otherwise it reports the journal state (`staged`, `replacing`, `checking`, `committed`, `rolling_back`, `rolled_back`, or `failed`), current and candidate build identities, optional error, and `unsigned` (false for authenticated signed updates). While a transaction is unfinished, another update cannot be staged; run `update recover` to resume helper work. `update rollback` is available only after a committed update and first checks whether the retained executable can read current launcher metadata. It does not restore the game directory. Update commands require network access except `status`, `rollback` and recovery; `--network-offline` prevents update checks and downloads.

`doctor` does not probe the network or write by default. `--network-probe` checks the official version metadata endpoint; `--write-probe` creates and removes a temporary empty file under launcher data. `diagnose --run ID` reads the saved run record and a bounded log prefix to classify known failure patterns. `diagnose --export PATH` creates a new local file containing allowlisted structured fields. Adding `--include-log-preview` includes up to 16 KiB of raw game output; review it before sharing because arbitrary game/mod logs may contain secrets. No report is uploaded.

<a id="output"></a>

## JSON output

Commands return an object with `ok`. Successful payload fields depend on the command, for example `versions`, `instance`, `managed_instances`, `launch`, and `log`. Errors have `ok: false`, `error.code`, and `error.message`. Error code families include `configuration_error`, `filesystem_error`, `network_error`, `invalid_version`, `instance_conflict`, `authentication_error`, `account_error`, `java_error`, `launch_error`, `loader_error`, `instance_error`, `pack_conflict`, `modpack_error`, `content_error`, `content_conflict`, `online_content_error`, `import_format_error`, `update_error`, `diagnostic_error`, and `cancelled`. Progress goes to stderr. clap help and argument errors remain text even with `--json`.

`credentials curseforge status` returns `provider`, `saved`, and `environment_override`; `set` and `remove` return `provider` and `saved`. These responses never contain the key. A successful save confirms local storage, not authorization by the provider.

```json
{"ok":false,"error":{"code":"content_conflict","message":"..."}}
```

The `launch` record includes `run_id`, `version`, `pid`, `exit_code`, `cancelled`, `log_path`, `started_at_ms`, and `finished_at_ms`. `logs` returns text in `log`. See the [user guide](user-guide.en.md) for workflows and configuration.

<a id="gui-g3"></a>
<a id="desktop-alpha"></a>
<a id="desktop"></a>

## Desktop application

The Tauri desktop application is `ccl-gui`, distributed separately from CLI ZIPs. This API reference targets v0.2.1; its GUI changes are not present in the original v0.2.0 package. Stable publishing requires a manual workflow dispatch for an existing `vX.Y.Z` tag and the exact `PUBLISH` confirmation. Invoke commands through Tauri's `invoke` API. Successful commands return their typed value. A failed command rejects with a serialized `{ "code": "...", "message": "..." }` error; these GUI error codes are separate from CLI error codes and exit statuses.

Common GUI error codes include `setup_required` (confirm a game root), `library_unavailable` (metadata cannot safely be read), `configuration_error`, `task_busy`, `task_not_found`, `task_not_cancellable`, `path_not_selected`, `cancelled`, and `update_error`. Commands may return additional operation-specific codes; consumers should branch on `code`, not localized message text.

| Command | Purpose |
| --- | --- |
| `bootstrap`, `initialize_root`, `select_root`, `select_config` | Return startup state, explicitly create and save the selected game root, or pick an existing root/configuration. A missing game directory is created only by `initialize_root`; picker cancellation returns `null`. |
| `save_preferences`, `set_network_offline` | Save GUI appearance preferences, or persist `downloads.offline` in the shared core config. |
| `list_versions`, `list_loaders`, `install_instance` | Discover versions/loaders and start vanilla/shared or managed-loader installation. |
| `inspect_import`, `import_instance` | Pick and inspect a supported local archive, then import it as a managed instance. |
| `launch_game`, `log_runs`, `read_run_log` | Launch a selected entry and read live or saved run logs. |
| `account_list`, `account_add_offline`, `account_select`, `account_logout`, `account_login`, `account_refresh`, `account_check_storage`, `open_login_browser` | List accounts, add a saved offline profile, select/remove an account, or invoke the retained experimental native device-login adapters and vault checks. |
| `cancel_task`, `task_snapshot`, `task_history`, `set_visibility` | Cancel active work, retrieve current/history snapshots, or control update notifications when the window is hidden. |
| `launcher_logs`, `launcher_log_debug`, `launcher_log_export`, `frontend_diagnostic` | Read bounded launcher logs, opt into debug navigation events, export a selected log locally, or record a fixed frontend error category. |
| `instance_query`, `instance_mutate`, `online_query`, `online_mutate` | Typed Advanced-page requests for instance/content operations and provider discovery/mutations. |
| `choose_path`, `inspect_dropped`, `maintenance`, `update_installation`, `gui_self_update` | Native path selection/import inspection, settings/provider/diagnostics, selected CLI maintenance and current GUI package updates. |

`bootstrap` returns a `Bootstrap` object with `root`, `root_exists`, `setup_required`, `launch_defaults`, `config_path`, `preferences`, `entries`, and `warnings`. `launch_defaults` contains nullable `java`, numeric `max_memory_mb`, and boolean `auto_install_java` and `network_offline`. `preferences` contains `theme` (`system`, `light`, or `dark`) and `language` (`en` or `zh-CN`). Each library entry contains `key`, `id`, `kind` (`vanilla` or `managed`), `minecraft`, nullable `loader`, and `game_directory`. Entry keys are `vanilla:<version>` or `managed:<instance-id>`. Warnings report unavailable or incompatible library metadata; bootstrap does not rewrite it. `initialize_root` creates and persists the proposed root only after explicit confirmation. `select_root` only selects an existing directory. `save_preferences` accepts `{ theme, language }`; GUI preferences are independent from the shared core configuration.

`install_instance` accepts `{ id, minecraft, loader, loader_version, shared }`. Set `shared: true` for a shared vanilla install and leave `loader` and `loader_version` null. With `shared: false`, both loader fields null create a managed vanilla instance; specify both to install a managed loader. `launch_game` accepts `{ key, identity, java, memory_mb, auto_install_java, dry_run }`; `identity` is `{ kind: "offline" | "microsoft", username, account_id }`. Offline identity uses `account_id: null` and an optional username (default `CCLPlayer`); Microsoft identity requires `account_id` and uses `username: null`. Keys use the library-entry format above. `inspect_import` opens a native archive picker and returns `null` on cancellation or `{ path, summary }`; `import_instance` accepts `{ update_existing, id, archive, include_optional, replace_modified, allowed_hosts }`. The archive must match the path most recently inspected or dropped. `select_config` returns `null` on cancellation or a refreshed `Bootstrap` after selecting an existing valid config. `CCL_GUI_CONFIG_PATH` pins the config and disables its picker; `CCL_GUI_GAME_DIR` pins the root and disables root selection.

`list_versions` returns entries shaped `{ id, kind }`; `list_loaders` accepts `{ minecraft, loader }` and returns loader version records. `account_list`, `account_select`, `account_logout`, and `account_add_offline` return `{ accounts, selected }`. `account_add_offline { username }` validates and saves a local offline profile without using the OS credential vault; selecting a saved account sets the launch selection, and removing an offline account removes only its local profile. Account metadata contains `id`, `username`, `uuid`, nullable `xuid`, `account_type`, nullable `oauth_client_id`, and nullable `last_used_unix`, never tokens. `account_login` accepts nullable `client_id` and `open_browser`; `account_refresh` accepts an account `id`; both return a task snapshot. `open_login_browser` accepts `task_id` and opens only the verification URI attached to that running login. `account_check_storage` returns `{ available: true }` on success. `choose_path` accepts one of `content_file`, `content_directory`, `archive`, `export`, `backup`, `diagnostic`, or `installation` and returns a nullable selected path; later commands require the matching native-picker grant. `cancel_task` accepts `{ id }`; `task_history` returns up to eight completed snapshots.

Long-running work returns a `TaskSnapshot` with `id`, `revision`, `state`, `kind`, `cancellable`, `phase`, `completed`, nullable `total`, nullable `pid`, `logs`, nullable `error` and `error_code`, nullable `result`, and nullable `authorization`. `state` is `running`, `cancelling`, `succeeded`, `failed`, or `cancelled`. Authorization contains only `user_code`, `verification_uri`, and `expires_in`; the device code and account tokens remain in Rust. The `gui-task` event carries snapshots. Consumers should ignore a snapshot whose `(id, revision)` is older than the one already applied. One task runs at a time; switching pages does not cancel it. Eight completed snapshots are retained. The native registry retains at most 1,000 log lines of at most 512 characters. Every snapshot sent to the WebView—events, `task_snapshot`, `task_history`, and `cancel_task` responses—contains at most the latest 100 lines. While hidden, the frontend keeps only its latest pending snapshot; on becoming visible it requests `task_snapshot` to reconcile state. While a task is active, a single in-flight reconciliation request runs at one-second intervals (five seconds for a running game) to recover missed terminal events; it stops when idle. Ordinary progress/log notifications are coalesced to at most 10 per second while visible; lifecycle and terminal transitions are sent immediately. Install/import cancellation is cooperative; synchronous instance transactions may be uncancellable and expose `cancellable: false`. Stopping a game is a separate confirmed action.

Offline identity selection in the GUI is separate from the persisted network-offline setting. `set_network_offline { offline }` updates the shared core `downloads.offline` value and returns a refreshed `Bootstrap`, so CLI and GUI use the same setting. GUI appearance preferences use a separate `gui.json`; root and config selections are stored alongside them. Account metadata is stored beside the core configuration. Saved offline profiles do not use the credential vault. Microsoft sign-in is temporarily unavailable in the GUI; the existing experimental CLI login protocol remains. The GUI client ID is configured only in the Settings advanced disclosure. No genuine-account acceptance is claimed here. See the [desktop GUI guide](user-guide.en.md#desktop-alpha) and [development notes](development.en.md#gui-g3).

`task_snapshot` returns the active task or `null`; `task_history` returns up to eight terminal snapshots. The task registry remains active across frontend navigation. The GUI presents task progress in a collapsible dialog with a persistent bottom-right entry; it collapses after a launched game reports its PID. Snapshot and history contracts remain available even when the dialog is closed.

`launcher_logs { index }` returns `{ text, path, debug }` for the current log (`index: 0`) or one of four rotated logs (`1..=4`). `text` is bounded to the latest 1,000 lines and 256 KiB. `launcher_log_debug { enabled }` persists whether debug navigation events are recorded. `launcher_log_export { index }` opens a local save dialog and returns whether export occurred; it writes the displayed bounded content and never uploads it. `frontend_diagnostic { event }` accepts only `render_error`, `unhandled_rejection`, `operation_error`, and `navigation`; navigation is recorded only when debug logging is enabled. Persistent logs are stored under GUI data `logs/`, rotate across five files of at most 5 MiB each, and allowlist startup version/platform/commit, task start/terminal events and GUI error codes. They never persist arbitrary request/response payloads, credentials, paths or JavaScript exception text.

`log_runs` returns up to 1,000 saved run records, newest first. `read_run_log` accepts `{ key, run_id, lines }` and returns `{ run_id, text, truncated }`; `run_id` may be null to select the latest run. It reads at most the last 1 MiB, returns at most 1,000 lines, and trims individual lines to 8,192 characters. `lines` is capped at 1,000. The Logs page uses task snapshots for the active launch; saved-run reads resume after launch ends. Creating managed instances and default imports rejects existing IDs under the instance lock; explicit existing-instance pack updates retain core conflict checks; shared vanilla installation can verify and repair an existing version.

The Advanced page exposes managed-instance loader install/update/uninstall, rename/copy/delete, history/restore/rollback, disk usage and cleanup preview/apply, pack inspect/receipt/export, local content list/add/remove/enable/disable and userdata backup/restore. Shared vanilla entries are read-only there. `InstanceRequest` is a tagged JSON enum with `operation` values `list`, `show`, `install`, `update`, `uninstall_loader`, `rename`, `copy`, `delete`, `history`, `restore`, `rollback`, `disk_usage`, `clean`, `pack_inspect`, `pack_show`, `pack_export`, `content_list`, `content_add`, `content_remove`, `content_enable`, `content_disable`, `content_backup`, and `content_restore`; unknown fields are rejected. Mutations run as tasks. Synchronous filesystem operations such as rename, copy, delete, restore, cleanup and local content edits use core instance locks and task admission control and cannot be cancelled once started; this also applies to instance rollback. Loader changes, online installs, imports and launches receive task cancellation and core cleanup context. Snapshots expose `cancellable`; cancellation of an uncancellable transaction is rejected and closing waits until it finishes.

The request fields by operation are: `list {}`, `show { id }`, `install/update { id, minecraft, loader, loader_version }`, `uninstall_loader { id }`, `rename/copy { id, destination }`, `delete/history/rollback/disk_usage/pack_show { id }`, `restore { id, revision }`, `clean { id, keep, include_previous, apply }`, `pack_inspect { archive }`, `pack_export { id, output, include_local_content }`, `content_list { id, kind }`, `content_add { id, kind, source, name? }`, `content_remove { id, kind, name }`, `content_enable/content_disable { id, name }`, and `content_backup { id, output }` / `content_restore { id, archive }`. `kind` serializes as `mod`, `resource_pack`, `shader_pack`, or `save` (distinct from CLI content-kind arguments). `loader` is lowercase: `fabric`, `quilt`, `legacyfabric`, `optifine`, `forge`, or `neoforge`.

Dropping one `.zip` or `.mrpack` onto the ready window inspects it and opens the import modal. The modal lets the user edit the instance name and import options; import is explicit and an existing instance is never automatically overwritten. A matching existing ID is rejected by default. Explicit `update_existing: true` selects the core pack-update path after a separate confirmation; `replace_modified` remains a separate permission. Adapter paths require a native picker grant for the matching purpose (archive, export, backup, content file/directory, diagnostic, installation or drop); a dropped archive is checked against its drop grant before inspection.

`OnlineQuery` and `OnlineMutation` are `operation`-tagged JSON enums that reject unknown fields. Query operations are `search { provider, query, minecraft?, loader?, kind?, limit? }`, `versions { provider, project, minecraft?, loader? }`, `resolve { provider, project, version?, minecraft, loader? }`, `updates { id }`, and `analyze { id }`. Mutation operations are `install { id, provider, project, version? }` and `pack_install { id, provider, project, version?, include_optional? }`. Providers serialize as `modrinth` or `curseforge`. Search defaults are an empty loader list, `kind: "mod"`, and `limit: 20`. The adapter constructs core `OnlineInstallRequest { root, instance_id, provider, project_id, version_id }` and calls the context-aware install path, which validates target instance identity and loader compatibility and carries task context through download and staged mutation. CurseForge credentials come from the selected config's OS vault unless `CCL_CURSEFORGE_API_KEY` is set; status returns only `saved` and `environment_override`, never the key.

`MaintenanceRequest` is an `operation`-tagged JSON enum that rejects unknown fields. It accepts `settings`, `set { key, value, expected }`, `reset { key?, expected }`, `provider_status`, `provider_set { key }`, `provider_remove`, `doctor { network_probe, write_probe }`, `diagnose { key?, run? }`, `export_diagnostics { key?, run?, output, include_log_preview }`, and `about`. The GUI edits the download and launch settings independently. `auth.microsoft_client_id` is retained in the shared config and is available only in the Settings advanced disclosure; it is a public application ID. GUI Microsoft sign-in is temporarily unavailable. The existing experimental CLI login protocol remains. Each field saves independently. `expected` is compared inside the core config lock, so an external CLI/GUI edit produces a stale-settings error instead of being overwritten; reload before saving again. Reset can target one key or all settings and can repair malformed config. Selecting another config validates and selects it, then clears cached path grants and dropped-archive selection. `CCL_GUI_CONFIG_PATH` pins the config and disables the picker. Provider keys use a password input and are stored only in the OS vault; environment override state is present/absent and the variable takes precedence.

Diagnostic export is local and requires a save-path grant. Raw log preview is off by default, capped by core limits and must be explicitly enabled; best-effort redaction does not guarantee removal of secrets. Review the exported file manually before sharing. `update_installation` receives the existing `UpdateRequest` operations `status`, `check`, `apply`, `rollback` and `recover`, each carrying `installation_root`; `check` and `apply` also carry `channel: "stable" | "dev"`. The native picker selects the folder containing the standalone CLI executable. These requests update only that selected CLI installation.

`gui_self_update` accepts an `operation`-tagged request with `status`, `check { channel }`, `apply { channel }`, `rollback`, or `recover`. It operates only on the currently installed GUI package; it has no `installation_root` field. The response contains `build`, `selected_channel`, `supported`, nullable `reason`, nullable `status`, nullable `check`, nullable `helper_pid`, and `restart_required`. `check` contains `available`, `build_pending`, `branch_commit`, and nullable `candidate`; transaction status contains `phase`, `version`, `channel`, `has_backup`, and nullable `last_error`. `apply`, `rollback`, and `recover` require confirmation and exit/restart the app when a helper is launched. GUI manifests are `gui-manifest-<target>.json`, with fields `schema_version`, `repository`, `product`, `channel`, `version`, `commit`, `branch`, `target`, `data_schema_min`, `data_schema_max`, `os_signed`, and `asset { format, name, size, sha256, url }`. The `asset` identifies the updater payload; distribution-only DMG/DEB files are separate release assets. `os_signed` is false for current installers. A detached `gui-manifest-<target>.json.sig` covers the exact manifest bytes and carries Tauri's trusted `timestamp:`, `file:`, and `version:` comment; the updater verifies that the authenticated version matches the manifest. The same embedded release key verifies both products. The app updater accepts the Windows per-user NSIS EXE, macOS `.app.tar.gz`, or Linux AppImage; DEB installations are updated by the package manager. Its helper keeps a previous package copy and rolls back when startup health is not acknowledged within 60 seconds. See [`self_update.rs`](../crates/ccl-gui/src/self_update.rs) and [`self_update_commands.rs`](../crates/ccl-gui/src/self_update_commands.rs), along with [`update_service.rs`](../crates/ccl-gui/src/update_service.rs) for standalone CLI operations.

`UpdateRequest` for `update_installation` is tagged by `operation`: `status`, `rollback`, and `recover` carry `{ installation_root }`; `check` and `apply` also require `{ channel }`. The `UpdateResponse` result tag is `status`, `checked`, `started`, or `recovery_started`, with the corresponding build/status, update check, or helper PID. Every operation is dispatched as a task; the selected root must be an existing CLI installation directory authorized by the native folder picker. These requests never target the GUI executable.

The release distribution targets are Windows x64 per-user NSIS EXE, macOS arm64 DMG, and Linux x86_64 AppImage and DEB. The GUI updater payloads are the NSIS EXE, macOS `.app.tar.gz`, and AppImage; DEB updates remain under package-manager control. Installers have no OS publisher signature and macOS packages are not notarized. GUI updater manifest signatures authenticate metadata and the payload digest; they are distinct from Authenticode and Apple signing/notarization.


The Discover page also resolves an optional version and nullable loader against a selected Minecraft version. A selected version resolves its pinned dependency graph; installation uses the same resolver. The GUI clears a plan when version or target filters change and requires a plan before installing content.

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
| `instances::InstanceService` | `list / metadata / install / install_vanilla / install_new_loader / acquire_launch / rollback / uninstall_loader` | Managed instances and launch guards |
| `instances::InstanceService` | `rename / duplicate / delete / history / restore_revision / disk_usage / cleanup` | Lifecycle, retained revisions and cleanup |
| `launch::LaunchService` | `prepare / launch` | Java selection, verified preparation and process orchestration |
| `launch` | `list_installed_versions / read_logs` | Installed vanilla versions and recorded logs |
| `modpacks::ModpackService` | `inspect / show / import / import_new / export` | Modrinth archives and ownership receipts |
| `import_formats::ImportFormatService` | `inspect / import / import_new` | CurseForge and MultiMC/Prism conversion |
| `content::ContentService` | `list / add / remove / set_enabled / backup / restore` | Local content and portable userdata backups |
| `online_content::OnlineContentService` | `search / versions / install_project / install_versions / updates_for_instance / analyze_instance / download_and_import_pack` | Provider content, dependency resolution and transactions |
| `auth::MicrosoftAuth` | `begin_device_login / complete_device_login / refresh / launch_identity` | Device authorization and launch identity |
| `accounts::AccountStore / accounts::OsSecretStore` | `list / selected / select / upsert / credentials / remove; probe` | Account metadata and OS-protected credentials |
| `accounts::OsProviderKeyStore` | `for_config_file / set_curseforge / get_curseforge / remove_curseforge` | Configuration-scoped provider keys in the OS credential vault; no plaintext fallback |
| `compatibility::inspect_compatibility` | `inspect_compatibility(config_path, game_root)` | Read-only compatibility report for config, account metadata, instances and receipts; no credential access |
| `update::UpdateService / update::Installation` | `check / stage; channel / set_channel / status / prepare_rollback / launch_helper / for_recovery` | Release checks and executable update transaction |
| `diagnostics::DiagnosticsService` | `doctor / diagnose_run / export` | Local health checks, bounded run classification and allowlisted exports |

<a id="tasks"></a>

## Events, cancellation and errors

Pass an optional `TaskContext` to operations that accept one. `EventSink` is an `Arc<dyn Fn(TaskEvent) + Send + Sync>` supplied by the interface. Clone the `CancellationToken` to request cancellation; keep an instance's launch guard alive while its game is running.

`TaskEvent` uses the serialized tag `kind` and snake-case variants: `started`, `progress`, `finished`, `process_started`, `log`, and `process_exited`. `started`, `progress`, and `finished` include a task name; progress also includes `completed` and nullable `total`. Process events contain a PID or nullable exit code; log events contain `stream` and `line`. Cancellation is cooperative: `cancel()` sets a shared flag, and operations that accept a context check it at their cancellation points. `TaskContext::emit` checks cancellation before invoking the sink, so no later event is delivered through that context after cancellation is observed. Cancel operations by calling `CancellationToken::cancel`; the CLI also maps an observed Ctrl-C to cancellation. Cancelled operations normally exit with `130`. A caller should keep an instance launch guard alive until the child process exits. Core services return typed errors; the CLI maps them to the exit codes below.

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
- CurseForge uses its REST API. The caller must provide its own API key through `OnlineConfig`; core does not persist the key, while the CLI can store a user key in the OS credential vault. Download requests attach the key only to the exact HTTPS CurseForge CDN hosts allowed by the download request. If the API omits a file's download URL, that file is left unavailable; the launcher does not guess a website URL.

Search results can be filtered by project type, Minecraft version, and loader. Both providers expose version files with available hashes and sizes. CurseForge's numeric project IDs are retained as strings in the provider-neutral API.

### Instance operations

`install_project` selects a version compatible with the target instance, resolves required dependencies, downloads and verifies all selected files, then applies the set in one `InstanceService::edit_content` revision. It writes `.ccl/online-content.json`, recording project versions and SHA-512 ownership hashes. Other provider projects and local mods remain in place. Updates replace only files owned by the updated projects and remove obsolete owned files after checking that their bytes still match the receipt. A modified or missing owned file stops the update with a conflict so the active generation is preserved.

`updates_for_instance` checks receipt projects against their providers. `analyze_instance` uses the version metadata stored in the receipt and reports missing required dependencies, incompatible selections, duplicate project versions, and different files claiming the same filename. Modpack versions can be downloaded and passed to the existing Modrinth or CurseForge import converter.

<a id="maintenance"></a>

## Updates, compatibility and diagnostics

The updaters fetch from this project's GitHub repository only. Stable releases use a `v<VERSION>` tag on `main`; developer releases are created for `dev` commits after Windows x64, macOS arm64 and Linux x86_64 validation succeeds, retaining the ten newest developer releases. CLI and GUI manifests use distinct `product` values (`ccl-cli` and `ccl-gui`) and have matching detached `.json.sig` assets. Tauri CLI 2.12.4 signs each raw manifest with the shared long-lived Actions key; the matching public key is embedded in `ccl-core`. Manifests declare data schema range `1..=1`; current readers preserve existing schema-less config, account metadata and instance metadata, and accept receipt schema 1. Unknown future fields in strict metadata or future receipt schemas block compatibility and updater rollback. Installer files remain unsigned by OS publisher certificates; the release signature authenticates manifest metadata and the package digest, not an Authenticode signature or macOS notarization.

`compatibility::inspect_compatibility` is read-only. It inspects the supplied config, adjacent account metadata and CCL-owned instance generations/receipts. It does not open the OS credential store or inspect saves and general game files. There is currently no data migration. A future format migration must lock and back up affected metadata, stage and atomically commit the change, support interruption recovery, and reject binary rollback when the retained reader cannot consume the resulting data.

`DiagnosticsService::doctor` checks Java candidates, directory access and whether a credential-store adapter is compiled in; it does not verify that a user vault is unlocked. Network and write probes are opt-in. `diagnose_run` checks a bounded game-log prefix and crash reports whose timestamps fall near the saved run; it reports known categories/evidence and leaves unknown causes unknown. Structured export replaces paths and omits accounts, credentials, worlds and server lists. Raw log preview is a separate opt-in field with best-effort redaction, not comprehensive redaction.

CLI archives and GUI installers are unsigned by OS publisher certificates. A matching SHA-256 confirms bytes, not publisher identity; the updaters authenticate each signed manifest and its package digest using the embedded release public key. This does not provide Authenticode signing or macOS notarization.

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

For a game failure, the JSON result record preserves the game's actual `exit_code`; CLI code `8` reports that the launch did not succeed. With `--json`, command execution results are JSON; clap help and argument parsing errors are text and use the argument-error exit code. A launched game that exits nonzero sets the result record’s `exit_code` to the game’s status, sets the result `ok` to false, and makes the CLI exit `8`; cancellation sets `cancelled` and exits `130`.
