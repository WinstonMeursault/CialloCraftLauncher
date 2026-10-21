# Command-line interface

## Usage

```text
ccl [--json] [--game-dir PATH] [--config PATH] <doctor|init|versions|install|instances|launch|logs>
```

Global options can be placed before the subcommand. Commands run non-interactively and do not wait for standard input.

| Command | Behavior |
| --- | --- |
| `doctor` | Read-only checks of configuration and game directory state; does not create a directory. |
| `init` | Selects and creates a game directory using the [directory priority](architecture.en.md#game-directory-selection). |
| `versions [--limit N]` | Lists available game versions. `--limit N` limits the result; `--limit 0` lists all versions. |
| `install VERSION` | Installs the named game version into the selected game directory. |
| `instances` | Lists installed vanilla versions and their required Java major versions. |
| `launch VERSION [--username CCLPlayer] [--java PATH] [--memory 2048] [--dry-run]` | Prepares or launches an installed version with an offline account. `--java` selects a Java executable or Java home; otherwise a compatible runtime is discovered. `--memory` sets the maximum heap in MiB and accepts 256–65536. `--dry-run` prepares files and prints the structured launch plan without starting Minecraft. |
| `logs [--run ID] [--lines 100]` | Prints the most recent run log, or the selected run's log, limited to the requested number of lines. |

Launch selects Java matching the version metadata's required major version and host architecture. Offline player UUIDs use the Java offline-player name-based UUID algorithm. Before launch, the launcher verifies the client JAR, applicable library artifacts, and asset index against available size and SHA-1 metadata. It prepares the asset projection, downloads and verifies the client logging configuration when specified, and extracts platform native libraries to a temporary directory. That directory is removed after the game exits; dry-run also removes it after printing the plan.

Game stdout and stderr are captured in `<game-directory>/.ccl/logs/<run-id>.log`. A JSON result record is written alongside it as `<run-id>.json`; it includes the version, process ID, actual game `exit_code`, cancellation state, log path, and start and finish timestamps. The `logs` JSON response has a `log` field containing the text. Launching is an offline mode and does not authenticate with a Microsoft account.

`--game-dir PATH` explicitly selects the game directory and takes precedence over default selection. `--config PATH` selects a configuration file. If omitted, the platform default is read and defaults are used when it is missing. A missing file explicitly selected with `--config` is a filesystem error (exit code `4`).

`--json` outputs command execution results as JSON. It is a global option and can be placed before the subcommand. Progress goes to standard error. clap-generated help and argument parsing errors remain text.

## Configuration

The JSON configuration fields are `downloads.version_list_source`, `downloads.file_source`, `downloads.mirror_root`, `downloads.http_proxy`, and `downloads.max_parallel`. The first two independently control the version-list and file sources and accept `auto`, `official`, or `mirror`; both default to `auto`. Missing fields use defaults. The mirror root is configurable. Version listing and installation use the official version manifest and metadata. Installation downloads the client, libraries, natives, asset index, and assets, requires and verifies SHA-1 for every downloaded file, limits parallel work, retries transient failures, reuses valid cached files, and replaces files through temporary files. Concurrent processes are coordinated with a version lock, and cancellation is supported. An existing version JSON that conflicts with the requested metadata causes a conflict error rather than being overwritten.

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
| `130` | Operation cancelled |

For a game failure, the JSON result record preserves the game's actual `exit_code`; CLI code `8` reports that the launch did not succeed. With `--json`, command execution results are JSON; clap help and argument parsing errors are text and use the argument-error exit code.
