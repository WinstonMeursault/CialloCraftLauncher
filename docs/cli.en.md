# Command-line interface

## Usage

```text
ccl [--json] [--game-dir PATH] [--config PATH] <doctor|init|settings|account|versions|install|instances|launch|logs>
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
| `launch VERSION [--username CCLPlayer] [--offline] [--account ID] [--java PATH] [--memory MiB] [--no-java-download] [--dry-run]` | Prepares or launches with the selected Microsoft account by default, or explicitly uses an offline profile. Java may be detected or installed from Mojang's runtime manifests. |
| `logs [--run ID] [--lines 100]` | Prints the most recent run log, or the selected run's log, limited to the requested number of lines. |

Launch selects Java matching the version metadata's required major version and host architecture. Offline player UUIDs use the Java offline-player name-based UUID algorithm. Before launch, the launcher verifies the client JAR, applicable library artifacts, and asset index against available size and SHA-1 metadata. It prepares the asset projection, downloads and verifies the client logging configuration when specified, and extracts platform native libraries to a temporary directory. That directory is removed after the game exits; dry-run also removes it after printing the plan.

Game stdout and stderr are captured in `<game-directory>/.ccl/logs/<run-id>.log`. A JSON result record is written alongside it as `<run-id>.json`; it includes the version, process ID, actual game `exit_code`, cancellation state, log path, and start and finish timestamps. The `logs` JSON response has a `log` field containing the text. Microsoft account credentials are kept in the operating system credential store, separately from account metadata. On Linux, a persistent Secret Service and D-Bus session must be available; there is no plaintext fallback. Logging out removes local credentials but does not revoke Microsoft consent. `--offline` or `--username NAME` explicitly selects an offline identity.

`--game-dir PATH` explicitly selects the game directory and takes precedence over default selection. `--config PATH` selects a configuration file. If omitted, the platform default is read and defaults are used when it is missing. A missing file explicitly selected with `--config` is a filesystem error (exit code `4`).

`--json` outputs command execution results as JSON. It is a global option and can be placed before the subcommand. Progress goes to standard error. clap-generated help and argument parsing errors remain text.

## Configuration

The JSON configuration fields are `downloads.version_list_source`, `downloads.file_source`, `downloads.mirror_root`, `downloads.http_proxy`, and `downloads.max_parallel`. The first two independently control the version-list and file sources and accept `auto`, `official`, or `mirror`; both default to `auto`. Missing fields use defaults. The mirror root is configurable. Version listing and installation use the official version manifest and metadata. Installation downloads the client, libraries, natives, asset index, and assets, requires and verifies SHA-1 for every downloaded file, limits parallel work, retries transient failures, reuses valid cached files, and replaces files through temporary files. Concurrent processes are coordinated with a version lock, and cancellation is supported. An existing version JSON that conflicts with the requested metadata causes a conflict error rather than being overwritten.

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
| `7` | Existing version metadata conflicts with the requested installation |
| `8` | Launch preparation or game failure, including a nonzero game exit |
| `9` | Java runtime discovery or validation error |
| `10` | Account or authentication error |
| `130` | Operation cancelled |

For a game failure, the JSON result record preserves the game's actual `exit_code`; CLI code `8` reports that the launch did not succeed. With `--json`, command execution results are JSON; clap help and argument parsing errors are text and use the argument-error exit code.
