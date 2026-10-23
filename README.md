# CialloCraftLauncher

A libre, open-source Minecraft: Java Edition launcher written in Rust for Windows, macOS and Linux.

## Project status

CCL currently provides a command-line interface and a reusable core independent of the interface. A GUI is planned for later. The first stable CLI release is being prepared; developer releases are prereleases and do not imply stable-release acceptance.

| Area | Available functionality |
| --- | --- |
| Installation and launch | Vanilla versions, Java discovery/managed runtimes, verified downloads/cache reuse, process logs and explicit network offline mode |
| Instances and loaders | Isolated instances; Fabric, Quilt, Legacy Fabric, Forge, NeoForge and OptiFine; revision rollback, rename/copy/delete/history and cleanup |
| Packs and content | Modrinth packs, supported CurseForge/MultiMC/Prism imports, online search/dependencies/updates/conflict analysis, local content and userdata backups |
| Maintenance | Stable/dev update channels, transactional executable replacement/recovery/rollback, local diagnostics and continuous regression workflows |

Support follows the tested samples and boundaries in the guides, rather than every Minecraft version, loader combination or operating-system release. Builds and automated tests cover these targets:

| Platform | Archive target | Validation boundary |
| --- | --- | --- |
| Windows x64 | `x86_64-pc-windows-msvc` | Windows runner builds/tests, credential-vault and update-process tests; no physical Windows game-launch acceptance |
| macOS Apple silicon | `aarch64-apple-darwin` | Automated checks and physical installation/game-launch/update samples |
| Linux x64 | `x86_64-unknown-linux-gnu` | Automated checks and physical samples; headless validation used a virtual display/software rendering for selected games |

Microsoft device login, refresh, OS credential storage and browser authorization are implemented. Online account integration remains experimental; the owner has deferred full entitled-account acceptance for this release. Offline identities remain supported. CurseForge uses a user-supplied key, optionally saved in the OS credential vault; CCL does not bundle a project key. See the [account guide](docs/user-guide.en.md#accounts) and [release checklist](docs/development.en.md#release).

## Install and get started

1. Open [GitHub Releases](https://github.com/WinstonMeursault/CialloCraftLauncher/releases), choose a release and download the archive matching the target above, with its `.sha256` file. Developer tags start with `dev-`; stable tags use `v<VERSION>`. GitHub's automatic source archives are not executable packages.
2. Follow the checksum and platform setup instructions in the [user guide](docs/user-guide.en.md#installation), extract the entire archive and run the binary from its extracted directory, or add that directory to `PATH`.
3. Run `ccl --help` and `ccl doctor` (`./ccl` on Unix or `.\ccl.exe` in PowerShell before adding it to `PATH`), then follow the guide for configuration, account setup and installation.

Packages are unsigned and not notarized. HTTPS and checksums detect corruption but do not independently authenticate the publisher. Minimum OS versions and additional architectures have not been certified. Linux Microsoft-account/provider-key storage requires an available, unlocked Secret Service/D-Bus session; there is no plaintext credential fallback.

To build from this checkout:

```sh
cargo build --release --locked -p ccl-cli
cargo run --release --locked -p ccl-cli -- --help
```

See the [development guide](docs/development.en.md#environment) for toolchain and native prerequisites. Builds do not bundle Minecraft, Java runtimes, mods or user credentials. Java can be downloaded later by the launcher when needed.

## Documentation

All public guides have aligned English and Simplified Chinese editions:

- [User guide](docs/user-guide.en.md) · [简体中文](docs/user-guide.zh-CN.md) — installation, configuration, account/key storage, vanilla/modded instances, imports/exports/backups, networking, updates and troubleshooting.
- [API reference](docs/api.en.md) · [简体中文](docs/api.zh-CN.md) — command/options, JSON contracts, core services, task events, errors and exit codes.
- [Development guide](docs/development.en.md) · [简体中文](docs/development.zh-CN.md) — architecture, builds/tests, packaging, release checklist, compatibility and service acceptance.
- [简体中文项目说明](README.zh-CN.md)

Report reproducible problems through [GitHub Issues](https://github.com/WinstonMeursault/CialloCraftLauncher/issues). Include the CCL version/target, command, expected/actual result and a reviewed diagnostic report where useful. Never include passwords, API keys, device codes, tokens or unreviewed raw logs.

## Acknowledgements

We thank the developers and contributors of Hello Minecraft! Launcher (HMCL) for their work on open-source Minecraft launchers. CialloCraftLauncher is independently developed; a graphical interface is planned for the future and is not available yet.

## License

Copyright (C) 2026 WinstonMeursault

This program is free software: you can redistribute it and/or modify
it under the terms of the GNU Affero General Public License as published
by the Free Software Foundation, either version 3 of the License, or
(at your option) any later version.

This program is distributed in the hope that it will be useful,
but WITHOUT ANY WARRANTY; without even the implied warranty of
MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the
GNU Affero General Public License for more details.

You should have received a copy of the GNU Affero General Public License
along with this program. If not, see <https://www.gnu.org/licenses/>.

SPDX license identifier: `AGPL-3.0-or-later`. See [LICENSE](LICENSE) for the full license text.

## Disclaimer

This is an unofficial Minecraft launcher. It is not approved by or associated with Mojang or Microsoft.
