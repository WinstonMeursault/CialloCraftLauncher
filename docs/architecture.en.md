# Architecture and design boundaries

## Goals

CialloCraftLauncher starts with a command-line interface. The reusable Rust core provides configuration, directory, network, download, and installation logic; the CLI and a future graphical interface call the same core services. The core does not depend on a terminal or graphical interface, so adding a GUI later will not require moving business logic into the interface layer.

Initial target platforms:

- Windows x86_64
- macOS arm64
- Linux x86_64

## Workspace

- `crates/ccl-core`: shared services, configuration, errors, HTTP transport, and task events.
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

## Download sources

Version lists and game files have separate source settings, each supporting `auto`, `official`, and `mirror`. Both default to `auto`; the core will choose a suitable source and fallback policy. The mirror root can be set in the user configuration. The configuration structure exists, but source selection, mirror URL mapping, downloads, and fallback workflows are not implemented yet.

The user configuration is JSON and defaults to:

- Windows: `%APPDATA%/CialloCraftLauncher/config.json`
- macOS: `~/Library/Application Support/CialloCraftLauncher/config.json`
- Linux: `$XDG_CONFIG_HOME/ciallocraftlauncher/config.json`, or `~/.config/ciallocraftlauncher/config.json` if the variable is unset

Configuration fields are `downloads.version_list_source`, `downloads.file_source`, `downloads.mirror_root`, `downloads.http_proxy`, and `downloads.max_parallel`. Missing fields use defaults. The mirror root must be an HTTPS URL without credentials, and unknown fields are rejected. If the default configuration file is missing, defaults are used; a missing file explicitly supplied with `--config` is an error.

## CLI and GUI consistency

The CLI and GUI share core operations, configuration, and error definitions. The CLI must not implement separate download or installation rules, and the GUI must not require the core to start a terminal or read standard input. `--json` formats command execution results as JSON. clap help and argument parsing errors remain text. CLI commands run without interactive input.

The core currently provides configuration, directory selection, an HTTP client, and task event/cancellation foundations. Installation, downloads, authentication, and a GUI are not implemented yet.
