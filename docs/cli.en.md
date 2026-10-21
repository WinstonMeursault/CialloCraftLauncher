# Command-line interface

## Usage

```text
ccl [--json] [--game-dir PATH] [--config PATH] <doctor|init>
```

Global options can be placed before the subcommand. Commands run non-interactively and do not wait for standard input.

| Command | Behavior |
| --- | --- |
| `doctor` | Read-only checks of configuration and game directory state; does not create a directory. |
| `init` | Selects and creates a game directory using the [directory priority](architecture.en.md#game-directory-selection). |

`--game-dir PATH` explicitly selects the game directory and takes precedence over default selection. `--config PATH` selects a configuration file. If omitted, the platform default is read and defaults are used when it is missing. A missing file explicitly selected with `--config` is a filesystem error (exit code `4`).

`--json` outputs command execution results as JSON. clap-generated help and argument parsing errors remain text.

## Configuration

The JSON configuration fields are `downloads.version_list_source`, `downloads.file_source`, `downloads.mirror_root`, `downloads.http_proxy`, and `downloads.max_parallel`. The first two independently control the version-list and file sources and accept `auto`, `official`, or `mirror`; both default to `auto`. Missing fields use defaults. The mirror root is configurable. Mirror mapping, version lookup, installation, downloads, launching, account management, and instance management commands are not implemented yet.

## Exit codes

| Code | Meaning |
| --- | --- |
| `0` | Success |
| `2` | Argument error |
| `3` | Configuration error |
| `4` | Filesystem error |
| `5` | Network error |
| `130` | Operation cancelled |

Scripts should use exit codes to determine the result. With `--json`, command execution results are JSON; clap help and argument parsing errors are text and use the argument-error exit code.
