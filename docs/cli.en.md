# Command-line interface

## Usage

```text
ccl [--json] [--network-offline] [--game-dir PATH] [--config PATH] <doctor|init|settings|account|versions|install|instances|loaders|instance|pack|content|online|launch|logs>
```

Global options can be placed before the subcommand. Commands run non-interactively and do not wait for standard input.

| Command | Behavior |
| --- | --- |
| `doctor` | Read-only checks of configuration and game directory state; does not create a directory. |
| `init` | Selects and creates a game directory using the [directory priority](architecture.en.md#game-directory-selection). |
| `settings show|get|set|reset` | Displays, reads, changes, or resets persisted settings. Proxy credentials are hidden in displayed settings. |
| `account check-storage|login|list|select|refresh|logout` | Checks the OS credential vault, signs in with a Microsoft account using device code, lists/selects accounts, refreshes, or removes local credentials. Login checks the vault before contacting Microsoft. See [Microsoft account setup](accounts.en.md). |
| `versions [--limit N]` | Lists available game versions. `--limit N` limits the result; `--limit 0` lists all versions. |
| `install VERSION` | Installs the named game version into the selected game directory. |
| `instances` | Lists installed vanilla versions and their required Java major versions. |
| `loaders fabric\|quilt\|legacyfabric\|forge\|neoforge\|optifine MINECRAFT [--limit N]` | Lists loader releases advertised for the requested Minecraft version. |
| `instance list\|show ID\|install ID MINECRAFT LOADER LOADER_VERSION\|update ID MINECRAFT LOADER LOADER_VERSION\|rollback ID\|uninstall-loader ID` | Manages isolated modded instances and loader revisions. See [modded instances and loaders](loaders.en.md). |
| `instance rename ID DESTINATION\|copy ID DESTINATION\|delete ID --yes\|history ID\|restore ID REVISION\|disk-usage ID\|clean ID [--keep N] [--include-previous] [--apply]` | Renames, copies, deletes, inspects, restores, and cleans retained instance revisions. Cleanup is preview-only unless `--apply` is given; see [instance lifecycle](instances.en.md). |
| `pack inspect ARCHIVE\|show ID\|import ID ARCHIVE [--include-optional] [--replace-modified] [--allow-host HOST]\|export ID OUTPUT [--include-local-content]` | Inspects and imports Modrinth, CurseForge, and MultiMC/Prism exports; exports Modrinth `.mrpack` packs. See the [import guide](imports.en.md) and [Modrinth and instance content guide](modpacks.en.md). |
| `content list ID KIND\|add ID KIND SOURCE [--name NAME]\|remove ID KIND NAME\|enable ID NAME\|disable ID NAME\|backup ID OUTPUT\|restore ID ARCHIVE` | Manages local mods, resource packs, shader packs, saves, and portable userdata backups. `KIND` is `mods`, `resourcepacks`, `shaderpacks`, or `saves`. |
| `online search\|versions\|resolve\|install\|updates\|analyze\|pack-install` | Searches Modrinth or CurseForge, resolves dependencies, installs or updates content, and analyzes recorded online content. See [online content](online-content.en.md). |
| `launch [VERSION] [--instance ID] [--username CCLPlayer] [--offline] [--account ID] [--java PATH] [--memory MiB] [--no-java-download] [--dry-run]` | Prepares or launches a vanilla version or an isolated instance. Uses the selected Microsoft account by default, or explicitly uses an offline profile. Java may be detected or installed from Mojang's runtime manifests. |
| `logs [--instance ID] [--run ID] [--lines 100]` | Prints the most recent run log, or the selected run's log, limited to the requested number of lines. `--instance` selects a managed instance's logs. |

Launch selects Java matching the version metadata's required major version and host architecture. Offline player UUIDs use the Java offline-player name-based UUID algorithm. Before launch, the launcher verifies the client JAR, applicable library artifacts, and asset index against available size and SHA-1 metadata. It prepares the asset projection, downloads and verifies the client logging configuration when specified, and extracts platform native libraries to a temporary directory. That directory is removed after the game exits; dry-run also removes it after printing the plan.

Managed instances use separate game directories and can be launched with `ccl launch --instance ID`; do not provide a positional version in this form. Loader compatibility is checked against upstream metadata before install, but does not guarantee compatibility with every mod set or prove every game launch combination. Fabric API and other mods are not bundled. Forge supports modern processor installers, spec 0 profiles, and the supported legacy `install`/`versionInfo` universal-JAR format; NeoForge uses modern processor installers. Loader updates and loader removal preserve current instance userdata. After a loader update, `instance rollback` stages current userdata into the prior loader generation; after a full content edit or pack update, rollback swaps the complete previous snapshot, including userdata and saves. Back up first if you need newer saves. `instance show` reports the resolved installed profile ID in `loader_version`, which can differ from the loader release string given to the command. Details and examples are in [loaders.en.md](loaders.en.md).

Pack updates check ownership hashes before replacing or removing files. Modified or deleted pack-owned files and a new pack path that collides with unowned local data stop the update with a conflict (exit code `7`); `--replace-modified` explicitly allows those local changes to be replaced or removed. Back up local data before using it. `pack export --include-local-content` includes local binary files only when you explicitly authorize redistribution. Content backups are the portable way to save userdata, including saves; they are separate from Modrinth exports.

Game stdout and stderr are captured in `<game-directory>/.ccl/logs/<run-id>.log`. A JSON result record is written alongside it as `<run-id>.json`; it includes the version, process ID, actual game `exit_code`, cancellation state, log path, and start and finish timestamps. The `logs` JSON response has a `log` field containing the text. Microsoft account credentials are kept in the operating system credential store, separately from account metadata. On Linux, a persistent Secret Service and D-Bus session must be available; there is no plaintext fallback. Logging out removes local credentials but does not revoke Microsoft consent. `--offline` or `--username NAME` explicitly selects an offline identity.

`--game-dir PATH` explicitly selects the game directory and takes precedence over default selection. `--config PATH` selects a configuration file. If omitted, the platform default is read and defaults are used when it is missing. A missing file explicitly selected with `--config` is a filesystem error (exit code `4`), except that `settings set` and `settings reset` can create it.

`--json`, `--network-offline`, `--game-dir`, and `--config` are global options and can be placed before the subcommand. `--network-offline` disables HTTP requests and requires verified local data; it is separate from `launch --offline`, which selects an offline player identity. Downloads resume only when the saved strong ETag and server range response validate the partial transfer. `--json` outputs command execution results as JSON. Progress goes to standard error. clap-generated help and argument parsing errors remain text.

## Configuration

The JSON configuration fields are `downloads.version_list_source`, `downloads.file_source`, `downloads.mirror_root`, `downloads.http_proxy`, `downloads.max_parallel`, and `downloads.offline`. The first two independently control the version-list and file sources and accept `auto`, `official`, or `mirror`; both default to `auto`. Missing fields use defaults. The mirror root is configurable. Version listing and installation use the official version manifest and metadata. Installation downloads the client, libraries, natives, asset index, and assets; it verifies using available metadata hashes and sizes, limits parallel work, retries transient failures, resumes only validated partial transfers, reuses valid cached files, and writes through temporary files before replacement. Concurrent processes use a version lock, and cancellation is supported. Existing conflicting version JSON causes a conflict error rather than being overwritten.

CurseForge online API requests require a user supplied `CCL_CURSEFORGE_API_KEY`; the launcher does not include or persist it. CurseForge archive import requires the key only when selected remote files must be resolved; overrides-only archives without selected remote files need no key. The key is sent only to approved provider API/CDN requests. Fixture coverage of header scoping does not establish live provider authorization. See [imports](imports.en.md) and [online content](online-content.en.md).

Launch settings are `launch.java` (optional executable or Java home), `launch.max_memory_mb` (default `2048`), and `launch.auto_install_java` (default `true`). The selected Java setting can be overridden with `--java`; `--no-java-download` skips managed Java installation for that run. Managed Java runtimes are installed under `<game-directory>/.ccl/runtimes` from official runtime manifests and verified artifacts. `auth.microsoft_client_id` is an optional public OAuth application ID, not a client secret. See [Microsoft account setup](accounts.en.md).

Examples:

```sh
ccl settings show
ccl settings set downloads.max_parallel 6
ccl settings set launch.java /opt/java/bin/java
ccl settings set auth.microsoft_client_id 00000000-0000-0000-0000-000000000000
ccl settings set downloads.http_proxy null
ccl settings reset launch.java
ccl account login
ccl account list
ccl launch 1.21.1 --dry-run
ccl loaders fabric 1.21.1 --limit 10
ccl loaders quilt 1.21.4 --limit 10
ccl loaders legacyfabric 1.8.9 --limit 10
ccl loaders optifine 1.20.1
ccl instance install my-world 1.21.1 fabric 0.20.1
ccl launch --instance my-world --dry-run
ccl instance rollback my-world
ccl pack inspect ./pack.mrpack
ccl pack import my-pack ./pack.mrpack --include-optional
ccl content list my-pack mods
ccl content backup my-pack ./my-pack-userdata.zip
ccl pack export my-pack ./my-pack.mrpack --include-local-content
```

`settings get` and `settings show` redact proxy username/password. The config file stores the configured proxy URL for network use; keep that file private. On Unix, files written by the launcher use owner-only permissions. Configuration updates are validated and written atomically, with a lock to coordinate simultaneous CLI processes. `settings reset --all` also recovers a malformed configuration by replacing it with defaults.

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
| `130` | Operation cancelled |

Vanilla metadata conflicts, managed-instance lock conflicts, Modrinth pack conflicts (including ownership/update conflicts), and content or pack operations blocked by an instance lock use exit code `7`. Loader failures, including loader-profile conflicts, and other instance failures use `11` and `12`. Other Modrinth pack and local content errors use `13` and `14`; online-content errors use `15`, archive import format errors use `16`, and cancellation uses `130`.

For a game failure, the JSON result record preserves the game's actual `exit_code`; CLI code `8` reports that the launch did not succeed. With `--json`, command execution results are JSON; clap help and argument parsing errors are text and use the argument-error exit code.
