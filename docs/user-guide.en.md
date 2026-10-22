# User guide

[API reference](api.en.md) · [Development guide](development.en.md) · [简体中文](user-guide.zh-CN.md)

- [Quick start](#quickstart)
- [Game directory](#directories)
- [Settings and configuration](#configuration)
- [Loaders and installation](#loaders)
- [Instance lifecycle and history](#instances)
- [Packs, local content and backups](#packs)
- [CurseForge and MultiMC/Prism imports](#imports)
- [Online search, dependencies and updates](#online)
- [Networking and offline mode](#networking)
- [Microsoft accounts](#accounts)

<a id="quickstart"></a>

## Quick start

Use `--game-dir PATH` to choose a game root explicitly. `doctor` checks its state; `init` creates the selected directory. Replace `1.21.1` with the Minecraft version you want. `launch --offline` selects an offline player identity and still permits downloads; [network offline mode](#networking) is a separate setting.

```sh
ccl --help
ccl doctor
ccl init
ccl versions --limit 10
ccl install 1.21.1
ccl launch 1.21.1 --offline --dry-run
ccl launch 1.21.1 --offline
ccl logs --lines 100
```

Java matching the version and host architecture is detected or installed automatically. Use `--java PATH` to select it explicitly, or `--no-java-download` to disable runtime installation. Use `--json` for structured output; see the [command and output reference](api.en.md#cli).

<a id="directories"></a>

## Game directory

The default game directory is selected in this order:

1. An explicit `--game-dir` path.
2. An existing `.minecraft` directory in the current working directory.
3. The platform's conventional Minecraft data directory; an existing directory can be reused on macOS.
4. `.minecraft` in the user's home directory.
5. Create `.minecraft` in the current working directory.

`doctor` checks configuration and directory state without creating or modifying the game directory. `init` selects and creates the game directory using this order.

<a id="configuration"></a>

## Settings and configuration

The user configuration is JSON and defaults to:

- Windows: `%APPDATA%/CialloCraftLauncher/config.json`
- macOS: `~/Library/Application Support/CialloCraftLauncher/config.json`
- Linux: `$XDG_CONFIG_HOME/ciallocraftlauncher/config.json`, or `~/.config/ciallocraftlauncher/config.json` if the variable is unset

The mirror root must be an HTTPS URL without credentials, and unknown fields are rejected. Proxy credentials are redacted in displayed settings. On Unix, config files written by the CLI are owner-only. If the default configuration file is missing, defaults are used; a missing file explicitly supplied with `--config` is an error.

The JSON configuration fields are `downloads.version_list_source`, `downloads.file_source`, `downloads.mirror_root`, `downloads.http_proxy`, `downloads.max_parallel`, and `downloads.offline`. The first two independently control the version-list and file sources and accept `auto`, `official`, or `mirror`; both default to `auto`. Missing fields use defaults. The mirror root is configurable. Version listing and installation use the official version manifest and metadata. Installation downloads the client, libraries, natives, asset index, and assets; it verifies using available metadata hashes and sizes, limits parallel work, retries transient failures, resumes only validated partial transfers, reuses valid cached files, and writes through temporary files before replacement. Concurrent processes use a version lock, and cancellation is supported. Existing conflicting version JSON causes a conflict error rather than being overwritten.

CurseForge online API requests require a user supplied `CCL_CURSEFORGE_API_KEY`; the launcher does not include or persist it. CurseForge archive import requires the key only when selected remote files must be resolved; overrides-only archives without selected remote files need no key. The key is sent only to approved provider API/CDN requests. Fixture coverage of header scoping does not establish live provider authorization. See [imports](user-guide.en.md#imports) and [online content](user-guide.en.md#online).

Launch settings are `launch.java` (optional executable or Java home), `launch.max_memory_mb` (default `2048`), and `launch.auto_install_java` (default `true`). The selected Java setting can be overridden with `--java`; `--no-java-download` skips managed Java installation for that run. Managed Java runtimes are installed under `<game-directory>/.ccl/runtimes` from official runtime manifests and verified artifacts. `auth.microsoft_client_id` is an optional public OAuth application ID, not a client secret. See [Microsoft account setup](user-guide.en.md#accounts).

`settings get` and `settings show` redact proxy username/password. The config file stores the configured proxy URL for network use; keep that file private. On Unix, files written by the launcher use owner-only permissions. Configuration updates are validated and written atomically, with a lock to coordinate simultaneous CLI processes. `settings reset --all` also recovers a malformed configuration by replacing it with defaults.

Settings examples:

```sh
ccl settings show
ccl settings get downloads.max_parallel
ccl settings set downloads.max_parallel 6
ccl settings set downloads.http_proxy null
ccl settings reset launch.java
```

<a id="loaders"></a>

## Loaders and installation

CCL can query loader metadata and install Fabric, Quilt, Legacy Fabric, Forge, NeoForge, and OptiFine into isolated managed instances. Loader compatibility is checked against the provider's version list before installation. This check confirms that the release is advertised for the requested Minecraft version; it does not verify every mod, modpack, Java setup, or launch combination.

Fabric and Legacy Fabric profiles come from their Meta APIs; Quilt profiles come from Quilt Meta. Forge supports processor based installers, spec 0 profiles with embedded Maven artifacts, and the earlier `install`/`versionInfo` universal JAR format. NeoForge uses processor based installers and embedded runtime artifacts. Unsupported installer structures return an explicit unsupported-format error.

OptiFine releases are listed through the configured BMCLAPI compatible mirror. CCL downloads a fresh installer over HTTPS, checks its archive size and the embedded Minecraft/OptiFine version constants, and runs the bundled patcher against the vanilla client JAR when the installer contains one. Bundled LaunchWrapper JARs are installed when present; older installers fall back to Maven LaunchWrapper 1.12. OptiFine does not publish a checksum through this metadata endpoint, so CCL does not reuse a prior installer download as a verified cache entry.

Isolated cold installs and macOS game menus have been verified for Quilt `0.20.0-beta.9` on Minecraft `1.21.4`, NeoForge `21.1.234` on Minecraft `1.21.1`, and OptiFine `HD_U_I6` on Minecraft `1.20.1`. Legacy Fabric `0.20.1` on Minecraft `1.8.9` and Forge `14.23.5.2860` on Minecraft `1.12.2` installed successfully on macOS and Linux; Linux clients reached game startup using a virtual display and software rendering. These results do not certify other versions, platforms, or third-party mods.

### Query loader releases

```sh
ccl loaders fabric 1.21.1
ccl loaders quilt 1.21.4
ccl loaders legacyfabric 1.8.9
ccl loaders forge 1.20.1 --limit 10
ccl loaders neoforge 1.21.1 --limit 10
ccl loaders optifine 1.20.1
```

The command prints compatible loader releases, newest first. Pass the selected release string to `instance install` or `instance update`.

### Manage an isolated instance

```sh
ccl instance install my-world 1.21.1 fabric 0.20.1
ccl instance install my-quilt-world 1.21.4 quilt 0.20.0-beta.9
ccl instance install my-optifine-world 1.20.1 optifine HD_U_I6
ccl instance list
ccl instance show my-world
ccl launch --instance my-world --dry-run
ccl logs --instance my-world
ccl instance update my-world 1.21.1 fabric 0.20.1
ccl instance rollback my-world
ccl instance uninstall-loader my-world
```

Each managed instance has its own game directory, version metadata, mods, configuration, and saves. It does not share the selected vanilla game directory with another instance. Installing or updating builds a staging revision and switches to it after a successful install. Updates retain the previous successful revision for `rollback`. Rollback switches to that revision. Loader removal returns the instance to vanilla while retaining user game data. These operations preserve mods, configuration, and saves across revisions.

The `loader_version` field shown by `instance show` records the resolved installed profile ID, such as `fabric-loader-0.20.1-1.21.1`; it is not necessarily the exact release string supplied on the command line. Instance launch uses the selected installed profile and its separate game directory.

CCL does not bundle Fabric API, mods, or modpacks. Install those separately into the instance's `mods` directory. Upstream compatibility metadata is a preflight check, not a guarantee that a particular mod set will work.

An instance selects one loader at a time. Use mods built for that loader and Minecraft release; Fabric, Forge and NeoForge mod files are not generally interchangeable. Changing the loader preserves the existing `mods` directory, so review those files before launching with the new loader. Removing the loader leaves those files in place but vanilla does not load them.

Forge-family installer integrity is checked against the official Maven SHA-1 sidecar. If it is unavailable, CCL fetches the official SHA-512 sidecar, verifies the installer bytes against it, and only then derives the SHA-1 used by the install pipeline. A cached installer is reused only when its URL, recorded SHA-1, and file size match. If neither upstream checksum can be obtained and no valid matching record exists, installation fails. Offline installation can reuse a verified installer record when the other required artifacts are already available; loader version discovery requires network access.

Metadata and checksum sources: [Fabric Meta](https://meta.fabricmc.net/), [Quilt Meta](https://meta.quiltmc.org/), [Legacy Fabric Meta API](https://github.com/Legacy-Fabric/legacy-meta), [OptiFine downloads](https://optifine.net/downloads), [Forge Maven](https://maven.minecraftforge.net/), and [NeoForge Maven](https://maven.neoforged.net/releases/).

`--json` can be used with these commands. `loaders` returns `loader_versions`; `instance list`, `show`, and lifecycle commands return `managed_instances` or `instance` records. Network and installation errors follow the general [CLI exit codes](api.en.md#exit-codes).

<a id="instances"></a>

## Instance lifecycle and history

These commands operate on managed instances under the selected game root. They use the same stable instance lock as launch; a running instance cannot be renamed, copied, deleted, restored or cleaned.

```sh
ccl instance copy source copy
ccl instance rename copy renamed
ccl instance history renamed
ccl instance disk-usage renamed
ccl instance restore renamed previous
ccl instance clean renamed --keep 2
ccl instance clean renamed --keep 2 --apply
ccl instance delete renamed --yes
```

Copy preserves the active instance, previous revision and archived revisions. Mutable files, worlds and configuration are independent copies. Verified asset files may share hard links. Copy does not copy unfinished stages. Rename changes the instance ID and rebases metadata paths; the operation records recovery information before moving the directory. IDs are ASCII letters, digits, `_` or `-`, up to 64 characters; case-only renames and case-insensitive destination collisions are refused.

Commits retain the immediate previous revision for `rollback`; older previous revisions are archived under `history/`. `history` lists `current`, `previous` (when available), and archive IDs. `restore ID REVISION` creates a new active revision from a complete retained snapshot, including its worlds and configuration, and retains the former active revision for rollback. `restore` accepts `previous` or an archive ID from `history`. Back up recent worlds before restoring. Loader rollback stages current saves, settings and other user data into the prior generation before swapping it into place; interruption recovery preserves a consistent active and rollback generation. Rollback after a full content edit or pack update swaps the complete snapshots.

`disk-usage` reports logical byte counts for active, previous, history and unfinished stage data. Hard links mean these numbers are not exclusive physical disk usage or guaranteed reclaimed space.

`clean` previews its targets by default. `--keep N` retains the newest N archived revisions (default 2); cleanup also targets an unfinished stage. `--include-previous` explicitly includes the immediate rollback revision. `--apply` executes the plan while holding the lock. Active worlds and content are never cleanup targets. Cleanup does not sweep shared root libraries/assets, other launchers' files, account credentials or installed Java runtimes in the active instance.

`delete --yes` permanently removes the entire selected instance, including its worlds and all history. Back up data first. It does not delete other instances or shared root resources. Stable lock files outside instance directories remain intentionally: deleting a lock file could allow two processes to bypass each other.

<a id="packs"></a>

## Packs, local content and backups

CCL can inspect, import and export Modrinth .mrpack v1 archives, including Quilt dependencies, and manage isolated instance content. CurseForge and MultiMC/Prism exports are supported through [additional import converters](user-guide.en.md#imports). A GUI is not provided.

The validated sample uses Minecraft `1.21.1`, Fabric Loader `0.20.1`, and Fabric API `0.116.17+1.21.1`. It reached the main menu on macOS arm64 and on Linux x86_64 using CPU rendering. An exported and re-imported macOS instance also reached the menu with matching mod hashes and override files. This does not certify other packs or third-party mod combinations. Windows is checked through compilation and automated tests.

### Inspect and import a pack

```sh
ccl pack inspect ./pack.mrpack
ccl pack import my-pack ./pack.mrpack
ccl pack show my-pack
```

`inspect` validates the archive and prints its manifest without installing it. `show` prints the recorded manifest and ownership data for an imported instance. Import supports Minecraft with no loader, Fabric, Quilt, Forge, or NeoForge as declared by the pack. Optional client files are skipped unless requested:

```sh
ccl pack import my-pack ./pack.mrpack --include-optional
```

Importing another archive into an existing managed ID updates that instance. CCL checks that pack-owned files still match their recorded hashes. A locally changed or deleted owned file, or a conflicting unmanaged file at a new pack path, stops the update with exit code `7`. Back up local work, or explicitly discard those changes with:

```sh
ccl pack import my-pack ./pack-v2.mrpack --replace-modified
```

`--replace-modified` permits replacing or removing the conflicting local files as needed by the new pack. Treat it as permission to discard those local changes.

### Download sources and integrity

Pack file URLs must use HTTPS. By default, downloads are accepted from Modrinth's CDN, GitHub, and GitLab hosts, with GitHub release asset hosts allowed for redirects. Every redirect is checked against the same HTTPS host policy, with a maximum of 10 redirects. Add a trusted exact hostname when a pack uses another source:

```sh
ccl pack import my-pack ./pack.mrpack --allow-host downloads.example.org
```

Repeat `--allow-host` to add more hosts. CCL checks each downloaded file's SHA-1, SHA-512, and declared size before placing it in the instance. `.mrpack` archives are limited to 100,000 entries, 128 MiB per file, and 512 MiB total uncompressed data.

### Export a pack

```sh
ccl pack export my-pack ./my-pack.mrpack
```

By default, export keeps verified remote file references and includes local configuration and options as overrides. It does not upload anything. Files that differ from the imported pack cannot be represented by their old remote reference. New or untracked mod, resource-pack, and shader-pack binaries require the explicit `--include-local-content` option:

Export also accepts ordinary managed instances created with `instance install`, including vanilla instances. It derives Minecraft and loader dependencies from the installed instance metadata. An instance without a pack record has no pinned remote references, so exporting its local binaries requires the same explicit permission option.

```sh
ccl pack export my-pack ./my-pack.mrpack --include-local-content
```

Use that option only when you have permission to redistribute every included binary. CCL does not check licenses or obtain permission for you. Review Modrinth's [`.mrpack` format specification](https://support.modrinth.com/en/articles/8802351-modrinth-modpack-format-mrpack) and [modpack permissions guidance](https://support.modrinth.com/en/articles/8797527-obtaining-modpack-permissions) before sharing an archive. CCL does not publish or upload the exported file.

Saves and `servers.dat` are excluded from pack exports; use a userdata backup for them.

### Manage local content

```sh
ccl content list my-pack mods
ccl content list my-pack resourcepacks
ccl content list my-pack shaderpacks
ccl content list my-pack saves
ccl content add my-pack mods ./local-mod.jar
ccl content add my-pack saves ./world --name MyWorld
ccl content remove my-pack mods local-mod.jar
ccl content enable my-pack example.jar
ccl content disable my-pack example.jar
```

`content add` copies the source and leaves the original in place. Mods must be `.jar` files (or `.jar.disabled` files); resource packs and shader packs must be `.zip` files; saves must be directories. `--name` is optional and selects the destination name. Enable and disable apply to `.jar` mods only.

### Back up and restore userdata

```sh
ccl content backup my-pack ./my-pack-userdata.zip
ccl content restore my-pack ./my-pack-userdata.zip
```

Backups contain user data from `mods`, `config`, `resourcepacks`, `shaderpacks`, `saves`, `defaultconfigs`, `datapacks`, `options.txt`, and `servers.dat`. They do not contain Minecraft or loader files, Java runtimes, or account credentials. Restore replaces every declared root; roots that were absent in the backup are removed from the instance. Make a backup before restoring if you may need the current files. Backups are limited to 100,000 entries and 2 GiB of file data.

### Revisions and running instances

Pack imports/updates and content edits are prepared in a staged instance generation and committed under the instance operation lock. They cannot replace the game directory while a managed launch holds the lock. `instance rollback` after a pack or content change restores the exact previous snapshot, including its user data and saves. Back up first if you need those newer saves. Loader rollback preserves current user data while switching loader revisions.

See [CLI exit codes](api.en.md#exit-codes); these commands support `--json`. Standard Modrinth dependencies cannot represent standalone OptiFine or Legacy Fabric; those managed instances use userdata backup rather than a misleading portable export.

<a id="imports"></a>

## CurseForge and MultiMC/Prism imports

`pack inspect ARCHIVE` inspects archives without downloading or installing. `pack import ID ARCHIVE` detects Modrinth, CurseForge and MultiMC/Prism exports by their manifests. A standard export may have its manifest at the ZIP root or inside one containing folder. Modrinth files need not have the `.mrpack` extension to be recognized.

```sh
ccl pack inspect ./export.zip
ccl pack import imported ./export.zip
ccl pack import imported ./export-v2.zip --replace-modified
```

### CurseForge

CCL reads Minecraft and loader versions from `manifest.json`, resolves pinned project/file IDs through the official API, verifies the declared SHA-1 and size, and imports the overrides and selected client files transactionally. Optional files are skipped unless `--include-optional` is supplied. File downloads use `x-api-key` only for explicitly recognized CurseForge HTTPS CDN hosts; the key is never added to archive URLs, receipts, or logs. Redirects to other permitted hosts do not receive it.

Remote file resolution requires your own API key in `CCL_CURSEFORGE_API_KEY`. Do not paste the key into a command argument or commit it. An overrides-only archive with no selected remote files can be converted without a key. A restricted file without a provider-authorized download URL fails with a diagnostic; CCL does not guess a download URL or bypass the author's distribution choice. The API and CDN requirements are documented by [CurseForge](https://docs.curseforge.com/rest-api/) and its [CDN authentication announcement](https://blog.curseforge.com/introducing-api-key-authentication-for-curseforge-file-downloads/).

This project has no owner-provided CurseForge API key for live acceptance. Provider behavior is checked with service/format fixtures; this is not evidence of production API authorization.

### MultiMC and Prism

CCL reads `mmc-pack.json` components and imports local game content from `minecraft/` or `.minecraft/`, including supported mods, configuration, packs, worlds, options and server lists. Loader/version requirements are resolved through CCL's existing installers. Exported instance hooks are not executed and account data is not imported. Unknown components and custom patches are rejected explicitly rather than silently producing a different instance. Multiple loaders are rejected. The current converter does not import arbitrary custom Java arguments, launcher-specific settings or account databases.

### Boundaries and recovery

Import uses the same locked, staged revision commit and file ownership conflict checks as Modrinth import. `--replace-modified` permits replacing conflicting managed content; back up local data before using it. Source archives remain untouched. Archive preflight checks entry count, declared uncompressed sizes, traversal, symlinks, case collisions and file/directory collisions before content is read. Limits are 100,000 entries, 128 MiB per entry, 512 MiB total uncompressed content and 2 MiB manifest JSON. CurseForge file metadata responses are also capped at 2 MiB, and cancellation interrupts pending metadata requests and downloads. Downloaded files count toward the converted pack's size limit.

These imports preserve private userdata, including worlds or server lists when present. Sharing an exported `.mrpack` is a separate action: that export excludes worlds/server lists and local binary redistribution still requires `--include-local-content`. Conversion does not grant redistribution permission.

<a id="online"></a>

## Online search, dependencies and updates

Use Modrinth without a key. CurseForge online requests require your own `CCL_CURSEFORGE_API_KEY` in the process environment; the launcher does not persist it. Search supports project type, Minecraft version and loader filters. Required dependencies and pinned versions are resolved and checked for compatibility; optional dependencies are listed, and incompatible selections are reported by analysis.

Installing another project retains unrelated mods. Updates stop if owned files were changed or deleted, or if new files collide with unmanaged local content. `updates` checks availability; use `install` to apply the selected version.

`online` accepts `modrinth` or `curseforge` as the provider. Project and version arguments can use the provider's project slug/ID and version ID. These examples show all seven subcommands:

```sh
ccl online search modrinth sodium --minecraft 1.21.1 --loader fabric --kind mod --limit 10
ccl online versions modrinth sodium --minecraft 1.21.1 --loader fabric
ccl online resolve modrinth sodium 1.21.1 fabric
ccl online install my-instance modrinth sodium --version VERSION_ID
ccl online updates my-instance
ccl online analyze my-instance
ccl online pack-install my-pack modrinth example-pack --version VERSION_ID --include-optional
```

`install` resolves and installs the selected project and required dependencies into an existing instance. `pack-install` downloads a provider pack and creates or updates an instance. `updates` and `analyze` inspect the receipt for an existing instance. CurseForge commands that call its API need `CCL_CURSEFORGE_API_KEY` in the process environment; Modrinth does not require a key. A successful fixture test of key header scoping does not establish live authorization for a real CurseForge key.

The APIs use `DownloadConfig` for proxy and offline behavior. A CurseForge key is required for CurseForge search, version lookup, and direct downloads; Modrinth needs no key. Online requests fail when offline mode is enabled.

<a id="networking"></a>

## Networking and offline mode

CCL separates the identity used to launch Minecraft from network availability. Setting `downloads.offline` to `true` disables network requests. A verified artifact already present at its normal game-directory path can be reused; a missing or invalid artifact produces an offline cache error.

Online installation saves the validated version manifest at `<game-directory>/.ccl/cache/version_manifest.json`. Offline installation uses this saved manifest and then checks the usual version metadata, libraries, asset index, and asset object paths. Offline mode cannot discover versions or download missing files. Return `downloads.offline` to `false` to refresh metadata or fill missing files.

The `versions` command also reads this validated per-game manifest in network offline mode, so it can list versions already known to that game directory without contacting a version service.

The install and launch cache lives in the selected game directory. Java runtime installation follows the same rule: an already valid managed runtime can be used without network access. A missing runtime can be installed offline when its saved runtime catalog, component manifest, and every required file are already cached; otherwise CCL reports the missing cache item. Existing source preferences and HTTP proxy settings apply only while online.

Downloads are checked against metadata hashes and sizes where provided. Interrupted single-file transfers can resume from a partial file only when the saved strong ETag matches the response ETag and the server confirms the requested offset and total with `Content-Range`. If those checks fail, CCL discards the partial and starts again. A file is published only after its complete size and hashes pass verification. Cancellation and exhausted retries leave the prior destination intact.

After the required files have been cached:

```sh
ccl --network-offline versions --limit 10
ccl --network-offline launch --instance my-world --offline
```

<a id="accounts"></a>

## Microsoft accounts

> **Preview status:** Microsoft sign-in is implemented but has not been verified with a real account. A CCL OAuth application has not yet been registered. The complete Microsoft, Xbox, and Minecraft flow must pass with that application before this feature can meet the CLI preview acceptance gate.

Application owners: see [registration and acceptance](development.en.md#microsoft-application).

### Configure and sign in

Save the client ID in the launcher's user configuration:

```sh
ccl settings set auth.microsoft_client_id YOUR-APPLICATION-CLIENT-ID
ccl account check-storage
ccl account login
```

`account login` tests the credential vault before contacting Microsoft and stops before requesting a device code if the vault cannot safely store credentials. `account check-storage` runs the same write/read/delete probe on demand; it uses a temporary sentinel and does not alter saved accounts.

Or pass an ID for one login:

```sh
ccl account login --client-id YOUR-APPLICATION-CLIENT-ID
```

The launcher prints a Microsoft verification address and one-time device code. Open that address in a browser, sign in, and enter the code. Do not share the code; anyone who enters it first could authorize the pending login. The CLI waits for completion. Then use:

```sh
ccl account list
ccl account select ACCOUNT-ID
ccl launch VERSION
```

With a selected account, `launch` uses it by default and refreshes credentials when needed. To launch offline instead, pass `--offline` or `--username NAME`. `--account ACCOUNT-ID` selects a specific saved account for one launch. `account logout [ACCOUNT-ID]` removes local credentials and account metadata; it does not revoke Microsoft consent.

### Credential storage

Access and refresh tokens are stored only through the operating system credential store. Account names and IDs are kept separately in `accounts.json`; that metadata file contains no tokens. On macOS and Windows the operating system credential vault is used. On Linux, a Secret Service implementation reachable through the active D-Bus session must be installed and unlocked. A headless system without a persistent Secret Service cannot use Microsoft login. There is no plaintext credential fallback. A graphical desktop is not required if you provide and unlock a persistent Secret Service in your session.

If secure storage is unavailable, login or refresh fails with an error. Start a desktop session with an unlocked keyring, or use offline launch until secure storage is available. Do not work around this by copying tokens into a file or shell command.
