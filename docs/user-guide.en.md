# User guide

[API reference](api.en.md) · [Development guide](development.en.md) · [简体中文](user-guide.zh-CN.md)

- [Quick start](#quickstart)
- [Desktop app and self-update](#desktop-alpha)
- [Installation and prerequisites](#installation)
- [Game directory](#directories)
- [Settings and configuration](#configuration)
- [Loaders and installation](#loaders)
- [Instance lifecycle and history](#instances)
- [Packs, local content and backups](#packs)
- [CurseForge and MultiMC/Prism imports](#imports)
- [Online search, dependencies and updates](#online)
- [Networking and offline mode](#networking)
- [Launcher updates and diagnostics](#maintenance)
- [Microsoft accounts](#accounts)
- [Troubleshooting](#troubleshooting)

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

`versions`, `loaders`, and `online search` return up to 20 entries by default. For `versions` and `loaders`, `--limit 0` returns all available entries; online search is capped by the provider. `logs` shows the latest 100 lines by default.

<a id="desktop-alpha"></a>

## Desktop app and self-update

This guide targets v0.2.2, including the CLI, core, desktop app and signed-manifest updater. check the [GitHub Releases page](https://github.com/WinstonMeursault/CialloCraftLauncher/releases) for package availability. The GUI packages are a per-user NSIS EXE for Windows x64, a DMG for macOS arm64, and AppImage or DEB for Linux x64. See the development guide for package targets and release requirements.

The desktop app asks you to confirm a game directory before it creates a missing one. Its library combines discovered Minecraft versions under the selected game root with existing CCL game directories, without moving or converting either. It can install versions and loader profiles, import supported local archives, launch with a saved offline account, and inspect live or saved game logs. Multiple offline accounts can be added, selected and removed; they are local profile names and do not use the OS credential vault. Microsoft sign-in is temporarily unavailable in the GUI. The experimental CLI sign-in flow remains available. Configure appearance and language in Settings, which is available before setup is complete. Configure the public Microsoft application client ID only in Settings → Advanced settings (`auth.microsoft_client_id`). The GUI's network-offline setting is saved in the shared launcher configuration and affects CLI behavior too. Choosing an offline player identity does not disable downloads or other network activity.

One install, import, or launch task can run at a time. Work continues when you switch pages. A collapsible progress dialog shows task progress and can be reopened from the bottom-right task entry; it collapses automatically after the game process starts. Installation and import tasks can be cancelled; stopping a game asks for confirmation. The task registry retains up to eight completed snapshots, and the dialog displays at most the latest 30 task log lines. Closing the app during active work asks whether to cancel the task or stop the game and exit. A second launch focuses the existing window.

Drop one `.zip` or `.mrpack` onto the ready window to inspect it and open the import dialog. Choose or edit the instance name and import options, review the summary, then explicitly start the import. An existing instance ID is rejected; the app does not automatically replace it. You can also use the native archive picker. For content files, backup/export destinations, diagnostics, and a CLI installation, use the corresponding native picker so the app can grant access to that selected path.

The library presents all discovered entries together, without a shared/managed category. You can change an entry’s display name; this writes only to `.ccl/library.json` and keeps its ID and game path unchanged. Remove from library hides the entry but leaves all files on disk; use Settings → Restore hidden entries to show hidden entries again. Existing game directories are never moved or copied during discovery. Operations that need CCL revision history—such as changing loaders, managing installed content, restoring revisions or deleting the entire directory—are available only for directories that already have CCL revision storage. Other discovered directories can be launched and have their display name changed or be hidden; they are not automatically adopted for full lifecycle management. In Discover, choose Modrinth or CurseForge, search, select a project and version, then inspect versions or resolve dependencies before installing into a supported CCL instance. Pack imports may create or update the explicitly named CCL instance after confirmation. Optional pack files are excluded unless selected.

Settings separates appearance and game-root choices from shared core settings. The Maintenance panel exposes the core download and launch settings. The Microsoft client ID is available only under the Settings advanced disclosure; GUI sign-in is temporarily unavailable. Save each field separately. If CLI or another process changes the config, reload before saving; stale edits are rejected. You can select a local JSON config unless `CCL_GUI_CONFIG_PATH` pins the config. CurseForge keys are entered in a password field and stored in the OS credential vault. `CCL_CURSEFORGE_API_KEY` overrides the saved value; status reveals only whether a saved key or override exists. Maintenance has separate controls for the installed GUI package and a standalone CLI installation.

Health checks make network or temporary-write probes only when their checkboxes are selected. Diagnostic export is local. Raw log preview is an explicit option and may contain private data despite best-effort redaction; review the file before sharing. The CLI updater operates on the standalone CLI in the folder you select. GUI update controls act only on the currently installed desktop package. Both support stable (`main`) and dev (`dev`) channels and persist the selected channel.

<a id="installation"></a>

## Installation and prerequisites

Download the CLI archive for your system from the [GitHub Releases page](https://github.com/WinstonMeursault/CialloCraftLauncher/releases). This guide targets v0.2.2; check GitHub Releases for published package availability. Developer releases are published separately. CLI archives use these names:

| System | Archive |
| --- | --- |
| Windows x86_64 | `ccl-x86_64-pc-windows-msvc.zip` |
| macOS arm64 | `ccl-aarch64-apple-darwin.zip` |
| Linux x86_64 | `ccl-x86_64-unknown-linux-gnu.zip` |

Desktop packages use separate names and installers. The v0.2.2 desktop release is planned to use the following package formats; check GitHub Releases to confirm publication:

| System | Install package | In-app update package |
| --- | --- | --- |
| Windows x86_64 | `ccl-gui-x86_64-pc-windows-msvc.exe` (per-user NSIS) | NSIS EXE; install in a writable user location |
| macOS arm64 | `ccl-gui-aarch64-apple-darwin.dmg` | `.app.tar.gz`; keep the installed `.app` in a writable location |
| Linux x86_64 | `ccl-gui-x86_64-unknown-linux-gnu.AppImage` or `.deb` | AppImage supports in-app updates; update DEB through the package manager |

GUI installer names and CLI ZIP names stay separate. The Windows self-updater supports the per-user installer, not a portable executable. macOS and Linux application-package updates need write access to the installed bundle or AppImage location. An installation in a protected or package-managed location should be updated by reinstalling through that installation method.

CLI ZIP release assets include `.sha256` checksum files. A checksum can detect a damaged download, but it does not authenticate the publisher. The GUI in-app updater verifies a detached signed manifest and its bound package digest. Directly installed GUI packages are not OS-signed; the public key signature is separate from Authenticode and macOS notarization.

Download the adjacent `.sha256` checksum file too, then verify the archive before extracting it. In PowerShell, run these commands from the download directory:

```powershell
$archive = "ccl-x86_64-pc-windows-msvc.zip"
$expected = (Get-Content "$archive.sha256").Split()[0]
$actual = (Get-FileHash $archive -Algorithm SHA256).Hash.ToLowerInvariant()
if ($actual -ne $expected) { throw "SHA-256 mismatch" }
```

On Linux, verify with `sha256sum -c ccl-x86_64-unknown-linux-gnu.zip.sha256`. On macOS, run `shasum -a 256 -c ccl-aarch64-apple-darwin.zip.sha256`. Run each command in the directory containing both files; continue only when it reports a match. The checksums detect archive changes and transfer corruption, but do not independently authenticate who published the archive.

Extract the whole CLI archive folder and keep its executable, README files, `docs/` directory, `LICENSE`, and third-party notice files together. The program is `ccl.exe` on Windows and `ccl` on macOS/Linux. Run `.\ccl.exe --help` in PowerShell or `./ccl --help` on macOS/Linux from that folder. If the Unix executable bit was lost during extraction, run `chmod u+x ./ccl` once. Keep the extracted folder in a location you can write to: the CLI updater stores transaction files in a sibling `.ccl-update` directory beside the executable. You can add the folder to `PATH` or invoke the executable by its full path. CLI archives do not contain the desktop application. The v0.2.2 target also includes a desktop app; dev prereleases may contain newer GUI changes. See [launcher updates](#maintenance) for update integrity limits.

You do not need to install Java to install CCL. On first launch CCL can download a Java runtime matching the Minecraft version and system architecture. For manual Java selection, install a compatible runtime and pass its executable or Java home with `--java PATH`. A network connection is needed to discover versions and download game files, managed Java, loaders, packs, or updates. Provide write access to the selected game directory and CCL configuration/data directories. On Linux, Microsoft login and saved CurseForge keys additionally need an available OS credential service; Microsoft login specifically requires an unlocked Secret Service reachable through the session D-Bus.

The targets above describe build and package coverage, not certification of each OS minimum. Minecraft menu validation is documented in each feature section and is narrower: there is no physical Windows game-launch validation in the current evidence. Linux game launches require a working display server and graphics drivers; CI success alone does not validate a user's graphics stack.


### macOS signature checks and first launch

The macOS build configuration uses an ad-hoc signature over the complete `.app`, including its Info.plist and resources. This is not a Developer ID signature or Apple notarization. After installing a trusted package, macOS may still require **System Settings → Privacy & Security → Open Anyway**; see [Apple's instructions](https://support.apple.com/en-us/102445). The updater manifest signature does not replace this OS check.

The original v0.2.0 macOS DMG and updater archive have an incomplete linker-only signature and can display “app is damaged”. A matching download checksum does not repair that signature. The v0.2.1 build configuration corrects this and packaging verifies the full bundle, DMG and update archive; existing v0.2.0 assets are unchanged. Install a package from v0.2.1 or later to obtain the corrected bundle; the v0.2.2 target retains this fix. For an already downloaded copy whose checksum you have verified against that release, you can repair its bundle signature locally with the commands below, substituting the actual installed path. This creates a local ad-hoc signature; it does not establish Apple publisher trust. If verification fails, stop and replace the copy. Then use Apple's per-app approval if offered; do not disable Gatekeeper globally.

```sh
codesign --force --sign - "/Applications/CialloCraftLauncher.app"
codesign --verify --deep --strict --verbose=2 "/Applications/CialloCraftLauncher.app"
```

### First run

Run `ccl doctor` to inspect configuration, directory access, Java candidates, and credential-store adapter availability. Then use `ccl init` to create the selected game directory, `ccl versions` to check version discovery, and `ccl install VERSION` to install a version. `doctor` is read-only unless a probe is explicitly requested. `init` creates the game directory but does not install Minecraft. Use `ccl launch VERSION --offline --dry-run` to preview a launch with an offline player identity, then omit `--dry-run` to start it. The first launch may download Java and remaining game files.

```sh
ccl doctor
ccl init
ccl versions --limit 10
ccl install 1.21.1
ccl launch 1.21.1 --offline --dry-run
```

<a id="directories"></a>

## Game directory

The default game directory is selected in this order:

1. An explicit `--game-dir` path.
2. An existing `.minecraft` directory in the current working directory.
3. The platform's conventional Minecraft data directory; an existing directory can be reused on macOS.
4. `.minecraft` in the user's home directory.
5. Create `.minecraft` in the current working directory.

`doctor` checks configuration and directory state without creating or modifying the game directory. `init` selects and creates the game directory using this order.

For existing profiles, resource files remain under the selected `.minecraft` root. If `versions/<profile>` contains `mods`, `config`, `saves`, or `options.txt`, CCL uses that profile directory for the game’s working directory and `--gameDir`; otherwise it uses the root. An explicit per-profile `hmclversion.cfg` directory choice (`usesGlobal: false`) takes precedence, including a custom directory. CCL does not move saves or settings. The game library reads loader and Minecraft version information from profile metadata rather than treating a custom profile name as a game version.

Forge manifests normalized by HMCL can use `${primary_jar_name}`; CCL expands it to the actual client JAR filename. If two profiles require different asset indexes with the same ID, CCL downloads and verifies a separate `assets/indexes/ccl-<SHA1>.json` and selects it for that launch. The shared index is preserved. Offline launch requires the exact index to be cached.

<a id="configuration"></a>

## Settings and configuration

The user configuration is JSON and defaults to:

- Windows: `%APPDATA%/CialloCraftLauncher/config.json`
- macOS: `~/Library/Application Support/CialloCraftLauncher/config.json`
- Linux: `$XDG_CONFIG_HOME/ciallocraftlauncher/config.json`, or `~/.config/ciallocraftlauncher/config.json` if the variable is unset

The mirror root must be an HTTPS URL without credentials, and unknown fields are rejected. Proxy credentials are redacted in displayed settings. On Unix, config files written by the CLI are owner-only. If the default configuration file is missing, defaults are used. Commands that read configuration require an explicitly selected `--config PATH` file to exist, including `settings show` and `settings get`. `update status`, `update rollback`, and `update recover` do not read that file. `settings set` and `settings reset` initialize a missing file from defaults; `settings reset --all` can also replace malformed configuration with defaults.

The configuration fields and defaults are:

| Setting | Default | Valid values |
| --- | --- | --- |
| `downloads.offline` | `false` | `true` or `false` |
| `downloads.version_list_source` | `auto` | `auto`, `official`, or `mirror` |
| `downloads.file_source` | `auto` | `auto`, `official`, or `mirror` |
| `downloads.mirror_root` | `https://bmclapi2.bangbang93.com` | HTTPS URL without credentials |
| `downloads.http_proxy` | `null` | A proxy URL or `null` |
| `downloads.max_parallel` | `8` | Integer from 1 to 64 |
| `auth.microsoft_client_id` | `null` | Public OAuth client ID or `null` |
| `launch.java` | `null` | Java executable path, Java home, or `null` |
| `launch.max_memory_mb` | `2048` | Integer from 256 to 131072 MiB |
| `launch.auto_install_java` | `true` | `true` or `false` |

For example, this complete configuration uses all defaults:

```json
{
  "downloads": {
    "offline": false,
    "version_list_source": "auto",
    "file_source": "auto",
    "mirror_root": "https://bmclapi2.bangbang93.com",
    "http_proxy": null,
    "max_parallel": 8
  },
  "auth": {
    "microsoft_client_id": null
  },
  "launch": {
    "java": null,
    "max_memory_mb": 2048,
    "auto_install_java": true
  }
}
```

For mapped Minecraft endpoints, `auto` tries official URLs before the configured mirror; `official` uses only official URLs; `mirror` tries the mirror before official URLs. Unknown hosts retain their original URLs. OptiFine uses the configured mirror endpoint. These settings do not redirect Modrinth, CurseForge, or GitHub requests.

You can omit fields; omitted fields use these defaults. `--memory` overrides `launch.max_memory_mb` for one launch and accepts 256–131072 MiB. Version listing and installation use the official version manifest and metadata. Installation downloads the client, libraries, natives, asset index, and assets; it verifies using available metadata hashes and sizes, limits parallel work, retries transient failures, resumes only validated partial transfers, reuses valid cached files, and writes through temporary files before replacement. Concurrent processes use a version lock, and cancellation is supported. Existing conflicting version JSON causes a conflict error rather than being overwritten.

CurseForge API requests require a user-provided key authorized for the relevant API and file downloads. [CurseForge's API documentation](https://docs.curseforge.com/rest-api/) describes its key process; access depends on provider authorization. Run `ccl credentials curseforge set` to enter an issued key through a hidden terminal prompt. CCL saves it in the OS credential vault, scoped to the selected configuration path, never in `config.json`. `ccl credentials curseforge status` shows whether a key is saved and whether an environment override is active; `ccl credentials curseforge remove` deletes the saved key. `CCL_CURSEFORGE_API_KEY` remains a temporary environment override and takes precedence over the saved key. Avoid command arguments, shell history, logs, and repository files for secrets. The OS vault must be available (on Linux, this generally requires a running Secret Service session); CCL does not fall back to plaintext storage. CurseForge archive import requires a key only when selected remote files must be resolved; overrides-only archives without selected remote files need no key. The key is sent only to approved provider API/CDN requests. Fixture coverage of header scoping does not establish live provider authorization. See [imports](user-guide.en.md#imports) and [online content](user-guide.en.md#online).

Launch settings are `launch.java` (optional executable or Java home), `launch.max_memory_mb` (default `2048`), and `launch.auto_install_java` (default `true`). The selected Java setting can be overridden with `--java`; `--no-java-download` skips managed Java installation for that run. Managed Java runtimes are installed under `<game-directory>/.ccl/runtimes` from official runtime manifests and verified artifacts. `auth.microsoft_client_id` is an optional public OAuth application ID, not a client secret. It is shown in the GUI only under Settings → Advanced settings; GUI sign-in is temporarily unavailable. The experimental CLI flow still uses it. See [account setup](user-guide.en.md#accounts).

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

Instance IDs accept UTF-8 names, including Chinese/Japanese text, spaces and safe punctuation. `/`, `\`, `:`, `*`, `?`, `"`, `<`, `>`, and `|`, control characters, trailing dots/spaces, `.`/`..`, and Windows reserved device names are rejected. Validation changes only which IDs are accepted; it does not move existing game directories or add metadata fields.

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

Copy preserves the active instance, previous revision and archived revisions. Mutable files, worlds and configuration are independent copies. Verified asset files may share hard links. Copy does not copy unfinished stages. Rename changes the instance ID and rebases metadata paths; the operation records recovery information before moving the directory. IDs support UTF-8, spaces and safe punctuation, up to 64 characters and 240 UTF-8 bytes. Control characters, `<>:"/\|?*`, `.`/`..`, trailing dots/spaces and Windows reserved names (`CON`, `PRN`, `AUX`, `NUL`, `COM1`–`COM9`, `LPT1`–`LPT9`, including names with extensions) are rejected; case-only renames and case-insensitive destination collisions are refused.

Commits retain the immediate previous revision for `rollback`; older previous revisions are archived under `history/`. `history` lists `current`, `previous` (when available), and archive IDs. `restore ID REVISION` creates a new active revision from a complete retained snapshot, including its worlds and configuration, and retains the former active revision for rollback. `restore` accepts `previous` or an archive ID from `history`. Back up recent worlds before restoring. Loader rollback stages current saves, settings and other user data into the prior generation before swapping it into place; interruption recovery preserves a consistent active and rollback generation. Rollback after a full content edit or pack update swaps the complete snapshots.

`disk-usage` reports logical byte counts for active, previous, history and unfinished stage data. Hard links mean these numbers are not exclusive physical disk usage or guaranteed reclaimed space.

`clean` previews its targets by default. `--keep N` retains the newest N archived revisions (default 2); cleanup also targets an unfinished stage. `--include-previous` explicitly includes the immediate rollback revision. `--apply` executes the plan while holding the lock. Active worlds and content are never cleanup targets. Cleanup does not sweep shared root libraries/assets, other launchers' files, account credentials or installed Java runtimes in the active instance.

`delete --yes` permanently removes the entire selected instance, including its worlds and all history. Back up data first. It does not delete other instances or shared root resources. Stable lock files outside instance directories remain intentionally: deleting a lock file could allow two processes to bypass each other.

<a id="packs"></a>

## Packs, local content and backups

CCL can inspect, import and export Modrinth .mrpack v1 archives, including Quilt dependencies, and manage isolated instance content. CurseForge and MultiMC/Prism exports are supported through [additional import converters](user-guide.en.md#imports). The desktop app can inspect supported local archives through its file picker; see [Desktop app and self-update](#desktop-alpha).

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

Remote file resolution requires your own API key saved with `ccl credentials curseforge set` or temporarily supplied in `CCL_CURSEFORGE_API_KEY`. Do not paste the key into a command argument or commit it. An overrides-only archive with no selected remote files can be converted without a key. A restricted file without a provider-authorized download URL fails with a diagnostic; CCL does not guess a download URL or bypass the author's distribution choice. The API and CDN requirements are documented by [CurseForge](https://docs.curseforge.com/rest-api/) and its [CDN authentication announcement](https://blog.curseforge.com/introducing-api-key-authentication-for-curseforge-file-downloads/).

Provider behavior has service and format fixture coverage. This does not establish successful live authorization or download access with a production key. Provider errors vary; follow the returned message and the provider's access requirements.

### MultiMC and Prism

CCL reads `mmc-pack.json` components and imports local game content from `minecraft/` or `.minecraft/`, including supported mods, configuration, packs, worlds, options and server lists. Loader/version requirements are resolved through CCL's existing installers. Exported instance hooks are not executed and account data is not imported. Unknown components and custom patches are rejected explicitly rather than silently producing a different instance. Multiple loaders are rejected. The current converter does not import arbitrary custom Java arguments, launcher-specific settings or account databases.

### Boundaries and recovery

Import uses the same locked, staged revision commit and file ownership conflict checks as Modrinth import. `--replace-modified` permits replacing conflicting managed content; back up local data before using it. Source archives remain untouched. Archive preflight checks entry count, declared uncompressed sizes, traversal, symlinks, case collisions and file/directory collisions before content is read. Limits are 100,000 entries, 128 MiB per entry, 512 MiB total uncompressed content and 2 MiB manifest JSON. CurseForge file metadata responses are also capped at 2 MiB, and cancellation interrupts pending metadata requests and downloads. Downloaded files count toward the converted pack's size limit.

These imports preserve private userdata, including worlds or server lists when present. Sharing an exported `.mrpack` is a separate action: that export excludes worlds/server lists and local binary redistribution still requires `--include-local-content`. Conversion does not grant redistribution permission.

<a id="online"></a>

## Online search, dependencies and updates

Use Modrinth without a key. For CurseForge online requests, save your own key with `ccl credentials curseforge set`, or temporarily set `CCL_CURSEFORGE_API_KEY`. Search supports project type, Minecraft version and loader filters. Required dependencies and pinned versions are resolved and checked for compatibility; optional dependencies are listed, and incompatible selections are reported by analysis.

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

`install` resolves and installs the selected project and required dependencies into an existing instance. `pack-install` downloads a provider pack and creates or updates an instance. `updates` and `analyze` inspect the receipt for an existing instance. CurseForge commands that call its API need a saved key or `CCL_CURSEFORGE_API_KEY`; Modrinth does not require a key. A successful fixture test of key header scoping does not establish live authorization for a real CurseForge key.

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

<a id="maintenance"></a>

## Launcher updates and diagnostics

The v0.2.1 release added the GUI workflow fixes, macOS bundle sealing and signed-manifest updater. The v0.2.2 target adds the unified library and metadata-only library actions. CLI builds predating signed manifests still require a manual bootstrap. CLI builds that predate detached signed-manifest updates cannot update themselves across that protocol change. Bootstrap once by downloading the current CLI ZIP from the [GitHub Releases page](https://github.com/WinstonMeursault/CialloCraftLauncher/releases) and verifying its `.sha256` file before replacing the old executable. Later updates use the signed manifest. Existing settings, account metadata, credentials and instances remain in the usual user data locations; compatibility checks cover known legacy formats, not every historical data set.

CCL checks for its own updates only when you run an update command. The stable channel follows `main`; v0.2.2 is this guide’s target version, and is not available until published on GitHub Releases. The dev channel follows `dev` and may include newer GUI changes not yet in stable. Stable releases use version tags such as `v0.2.2`. A dev release is published for a dev commit after all three supported CI targets complete validation and packaging; the ten newest developer releases are retained.

```sh
ccl update check
ccl update check --channel stable
ccl update apply
ccl update status
ccl update rollback
ccl update recover
ccl update recover --installation /path/to/ccl
```

Selecting a channel with `--channel` saves that choice. `check` reports whether a compatible published package is available. `apply` downloads and validates the target-specific archive, then starts a helper to replace the executable asynchronously. CLI manifests are named `manifest-<target>.json`; each has a matching `.json.sig` asset. The signature covers the exact manifest bytes and binds the product, version, channel, branch, commit, target and package digest. The updater verifies it with the public key embedded in the application, then checks the package size, SHA-256 and archive paths before staging. The CLI ZIP itself is not OS-signed. On Windows, temporary executable-sharing failures are retried for up to 30 seconds. Check `update status` after the command returns. If the helper still holds the update lock, wait briefly and retry; a busy response is not a failed transaction. Use `recover` after an interrupted helper or replacement; if the normal executable is unavailable, run recovery from the retained helper with `--installation` pointing to the original executable path. `rollback` uses the retained previous executable only when the current config/account metadata, instances and receipts are readable by that older binary. A binary rollback never restores your game directory, saves or content.

The GUI Maintenance panel has its own `Check for updates`, `Download and install`, `Roll back GUI update`, and `Recover interrupted update` controls. GUI manifests use `gui-manifest-<target>.json` plus a detached `.json.sig` asset signed by the same embedded updater key. The signature binds the product/version/channel/commit/target and the updater payload digest; operating-system publisher signing is separate and is not provided. GUI packages are not Authenticode-signed, and macOS packages are not notarized.

Self-update applies only to an installed per-user Windows NSIS application, a writable macOS `.app`, or a writable Linux AppImage. A Linux DEB installation is updated through its package manager. The helper retains a checked copy of the previous application, installs the verified package, starts the new application and waits up to 60 seconds for its startup health acknowledgment. If the app exits early or does not acknowledge startup, the helper restores and restarts the previous application. Use the GUI rollback control after an applied update while the backup is available. Use recovery after an interrupted transaction; recovery returns to the known previous application when possible. The journal and update payload live outside the installed application. Game files, saves, accounts and credentials are not part of the application rollback.

Keep game directories, saves and configuration outside the application package. Self-update refuses overlapping selected data/configuration paths. Manual rollback also refuses to overwrite application files modified after the update; preserve those changes first. On Windows, rollback restores the checked current-user uninstall/product registry metadata as well as application files.

If an interruption leaves the application entry missing, use the retained helper directly. Its default directory is `~/Library/Application Support/CialloCraftLauncher/gui/update` on macOS, `%APPDATA%\CialloCraftLauncher\gui\update` on Windows, or `${XDG_CONFIG_HOME:-$HOME/.config}/ciallocraftlauncher/gui/update` on Linux; an explicit `CCL_GUI_DATA_DIR` uses its `update` subdirectory. Set `GUI_UPDATE_DIR` to that directory, then run `"$GUI_UPDATE_DIR/helper" --ccl-gui-update-helper "$GUI_UPDATE_DIR" recover` on macOS, or use `helper.AppImage` on Linux. In PowerShell, run `& "$GUI_UPDATE_DIR\helper.exe" --ccl-gui-update-helper "$GUI_UPDATE_DIR" recover`. Retain the journal, payload and backups until recovery completes. These commands recover the recorded installation; they do not select a different application.

Only one GUI instance or updater helper can operate on an installation at a time. If a status or update operation reports that the installation is busy, close the other instance or let the helper finish, then retry. `--network-offline` blocks CLI and GUI update checks and downloads. Package-manager or read-only installations should be updated through their installation method.

`doctor` checks Java candidates, launcher/game directory access and whether a system credential-store adapter is present. It does not log in or test whether the vault is unlocked. Add `--network-probe` to contact Mojang's version metadata endpoint, or `--write-probe` to create and remove a temporary empty file under CCL-owned data. Both probes are explicit.

```sh
ccl doctor
ccl doctor --network-probe
ccl doctor --write-probe
ccl diagnose --run RUN-ID
ccl diagnose --instance INSTANCE-ID --run RUN-ID
ccl diagnose --export ./ccl-diagnostic.json
ccl diagnose --run RUN-ID --export ./ccl-diagnostic.json --include-log-preview
```

Run diagnosis uses the saved run record, a bounded game-log prefix and timestamp-correlated crash reports to recognize a small set of common failure patterns. It reports evidence and suggestions; an unknown failure is not forced into a category. Structured diagnostic export uses allowlisted fields and replaces local paths. It excludes account databases, credential stores, worlds and server lists. The optional raw preview includes up to 16 KiB of game output and applies best-effort redaction, but may still contain account names, tokens or other content written by the game or mods. Review the file before sharing it. CCL does not upload diagnostic reports.

Updater compatibility checks cover defined historical samples, not every old configuration, account database or instance. GUI packages use signed update manifests, while the application files themselves are not Authenticode-signed or notarized. Hosted-runner startup checks and package targets do not certify the planned minimum OS versions. Physical Windows Minecraft game-launch validation is not available in the current evidence. See the API reference for manifest details and the development guide for release requirements.

<a id="accounts"></a>

## Accounts

The GUI Accounts page lets you add, select and remove multiple saved offline player names. Names are 1–16 ASCII letters, digits or underscores. These local profiles do not use the OS credential vault. The game library selects a saved account directly; it does not ask for a separate username. The CLI continues to support its default offline identity. `--account ACCOUNT-ID` takes the actual ID of a saved account (do not use `offline` as a placeholder ID); `--offline --username NAME` remains available for a one-off name.

> **Experimental CLI online integration:** Microsoft device login, Xbox Live/XSTS exchange, and Minecraft service integration remain in the CLI. Microsoft sign-in is temporarily unavailable in the GUI. Full entitled-account acceptance is deferred; automated coverage and OAuth device-flow checks do not establish that every account can launch the game. Check the reported authentication stage and machine error code if CLI login or launch fails.

Application owners: see [registration and acceptance](development.en.md#microsoft-application).

### Configure and sign in

For the experimental CLI flow, save the client ID in the launcher's user configuration. In the GUI, the public client ID belongs only in Settings → Advanced settings; GUI sign-in is currently unavailable:

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

The launcher prints a Microsoft verification address and one-time device code, and automatically opens the default browser. Sign in there and enter the code. Use `account login --no-browser` to open the page yourself, such as when signing in on a headless server. If automatic opening fails, the printed address remains usable and login continues. Do not share the code; anyone who enters it first could authorize the pending login. The CLI waits for completion. Then use:

```sh
ccl account list
ccl account select ACCOUNT-ID
ccl launch VERSION
```

With a selected account, `launch` uses it by default and refreshes credentials when needed. Pass `--offline` to select an offline identity; you can combine it with `--username NAME` to choose that identity's name. These options each conflict with `--account ACCOUNT-ID`, which selects a saved account for one launch. `account logout [ACCOUNT-ID]` removes local credentials and account metadata; it does not revoke Microsoft consent.

Authentication failures identify the service stage and HTTP status, with allowlisted machine error codes when available. Raw service error text, tokens and account identifiers are not printed. A service rejection is separate from a verified lack of Java entitlement; a generic 401/403 alone does not establish the cause.

A recorded live retry completed Microsoft OAuth, Xbox Live and XSTS, then received HTTP 403 at Minecraft login before profile or entitlement data was returned. That status does not identify the service-side cause or establish whether the account owns the game. The authentication stage and error code are the useful details to report when troubleshooting.

### Credential storage

Access and refresh tokens are stored only through the operating system credential store. Account names and IDs are kept separately in `accounts.json`; that metadata file contains no tokens. On macOS and Windows the operating system credential vault is used. On Linux, a Secret Service implementation reachable through the active D-Bus session must be installed and unlocked. A headless system without a persistent Secret Service cannot use Microsoft login. There is no plaintext credential fallback. A graphical desktop is not required if you provide and unlock a persistent Secret Service in your session.

If secure storage is unavailable, CLI login or refresh fails with an error. Start a desktop session with an unlocked keyring, or use a saved GUI offline profile until secure storage is available. Do not work around this by copying tokens into a file or shell command.

<a id="troubleshooting"></a>

## Launcher logs and diagnostics

The GUI Logs page has separate views for game-run logs and CCL launcher logs. Launcher logs record startup build identity, task start/terminal outcomes, GUI error categories and optional navigation events. Launch failures distinguish unsupported placeholders (`launch_unsupported_placeholder`), other plan errors (`launch_plan_failed`), and Java errors (`launch_java_failed`). They do not contain arbitrary request data, account names, tokens, filesystem paths, JavaScript exception text or game output. Five files are retained at up to 5 MiB each; each view is limited to 1,000 lines and 256 KiB. Enable debug logging only when navigation events are useful. Export copies the displayed local log to a location you choose; CCL does not upload it.

Diagnostic reports are a separate feature. Review any report before sharing, especially when the optional raw game-log preview is enabled.

## Troubleshooting

- **Version discovery or downloads fail:** check connectivity and proxy settings with `ccl settings show`. `--network-offline` and `downloads.offline` disable HTTP requests; clear offline mode before fetching missing files. The `doctor --network-probe` check tests access to Mojang's version metadata endpoint.
- **The game cannot find Java:** run `ccl doctor`, allow the managed runtime download, or pass a compatible Java executable/home with `--java PATH`. `--no-java-download` intentionally fails if no suitable runtime is installed.
- **A launch exits immediately:** inspect `ccl logs --lines 200` or select a run with `ccl logs --run RUN-ID`; for a managed instance, add `--instance ID`. Run `ccl diagnose --run RUN-ID` to inspect the saved result and nearby crash report. The run record stores the game's actual exit code; the CLI returns `8` when the game exits unsuccessfully.
- **Configuration is invalid:** check the JSON file and setting name. `ccl settings reset --all` replaces invalid configuration with defaults; use `--config PATH` when working with a non-default file.
- **CLI Microsoft login or launch fails:** inspect the service stage and allowlisted machine error code. Confirm the public client ID and credential vault, then distinguish Microsoft authorization, Xbox/XSTS access, Minecraft profile access, and entitlement errors. A generic HTTP 401/403 does not by itself prove that the account lacks ownership.
- **CurseForge requests are denied:** confirm that the saved key or `CCL_CURSEFORGE_API_KEY` is active and authorized for the API and requested file. A key accepted for search may not authorize a restricted file download; follow the provider's authorization and distribution rules.
- **A loader or pack install reports unsupported/conflicting content:** check the selected Minecraft/loader release, inspect pack metadata with `ccl pack inspect`, and review locally modified owned files before using `--replace-modified`.

Share logs or diagnostic exports only after reviewing them. Raw game logs and the optional diagnostic log preview can contain account names, tokens, or mod-written content.
