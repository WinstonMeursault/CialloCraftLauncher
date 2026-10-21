# Command-line interface

## Usage

```text
ccl [--json] [--game-dir PATH] [--config PATH] <doctor|init|versions|install>
```

Global options can be placed before the subcommand. Commands run non-interactively and do not wait for standard input.

| Command | Behavior |
| --- | --- |
| `doctor` | Read-only checks of configuration and game directory state; does not create a directory. |
| `init` | Selects and creates a game directory using the [directory priority](architecture.en.md#game-directory-selection). |
| `versions [--limit N]` | Lists available game versions. `--limit N` limits the result; `--limit 0` lists all versions. |
| `install VERSION` | Installs the named game version into the selected game directory. |

`--game-dir PATH` explicitly selects the game directory and takes precedence over default selection. `--config PATH` selects a configuration file. If omitted, the platform default is read and defaults are used when it is missing. A missing file explicitly selected with `--config` is a filesystem error (exit code `4`).

`--json` outputs command execution results as JSON. It is a global option and can be placed before the subcommand. Progress goes to standard error. clap-generated help and argument parsing errors remain text.

## Configuration

The JSON configuration fields are `downloads.version_list_source`, `downloads.file_source`, `downloads.mirror_root`, `downloads.http_proxy`, and `downloads.max_parallel`. The first two independently control the version-list and file sources and accept `auto`, `official`, or `mirror`; both default to `auto`. Missing fields use defaults. The mirror root is configurable. The current `versions` and `install` workflows use the official version manifest and metadata. Installation downloads the client, libraries, natives, asset index, and assets, requires and verifies SHA-1 for every downloaded file, limits parallel work, retries transient failures, reuses valid cached files, and replaces files through temporary files. Concurrent processes are coordinated with a version lock, and cancellation is supported. An existing version JSON that conflicts with the requested metadata causes a conflict error rather than being overwritten.

## Exit codes

| Code | Meaning |
| --- | --- |
| `0` | Success |
| `2` | Argument error |
| `3` | Configuration error |
| `4` | Filesystem error |
| `5` | Network error |
| `6` | Invalid or unknown version |
| `7` | Existing version metadata conflicts with the requested installation |
| `130` | Operation cancelled |

Scripts should use exit codes to determine the result. With `--json`, command execution results are JSON; clap help and argument parsing errors are text and use the argument-error exit code.
