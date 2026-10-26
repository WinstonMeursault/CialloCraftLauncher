# CialloCraftLauncher

A libre, open-source Minecraft: Java Edition launcher written in Rust for Windows, macOS and Linux.

## Project status

CCL provides a Rust core, a command-line interface, and a Tauri desktop GUI with its own installer and package updater. The latest stable release is v0.2.0 on `main`, with the CLI, core, desktop GUI and signed-manifest updater. Current GUI changes described in the guides are on `dev` and are not part of that stable release.

| Area | Available functionality |
| --- | --- |
| Installation and launch | Vanilla versions, Java discovery/managed runtimes, verified downloads/cache reuse, process logs and explicit network offline mode |
| Instances and loaders | Isolated instances; Fabric, Quilt, Legacy Fabric, Forge, NeoForge and OptiFine; revision rollback, rename/copy/delete/history and cleanup |
| Packs and content | Modrinth packs, supported CurseForge/MultiMC/Prism imports, online search/dependencies/updates/conflict analysis, local content and userdata backups |
| Maintenance | Signed stable/dev updates for CLI and GUI, transactional install/recovery/rollback, local diagnostics and regression workflows |

Support follows the samples and boundaries in the guides, rather than every Minecraft version, loader combination or operating-system release. The configured CI and release targets are:

| Platform | Archive target | Validation boundary |
| --- | --- | --- |
| Windows x64 | `x86_64-pc-windows-msvc` | Workflow target: Windows 2025; no physical Windows game-launch acceptance |
| macOS Apple silicon | `aarch64-apple-darwin` | Workflow target: macOS 15; historical samples do not establish the minimum OS |
| Linux x64 | `x86_64-unknown-linux-gnu` | Workflow target: Ubuntu 24.04; historical samples do not establish compatibility with every Linux distribution |

Desktop release workflows are configured for Windows x64 per-user NSIS EXE, macOS arm64 DMG, and Linux x64 AppImage and DEB packages. The separate GUI validation workflow exercises update and manual rollback, bad-package automatic rollback, and interrupted-replacement recovery; release workflows check rendered startup readiness. GUI self-update accepts the Windows installer, macOS `.app.tar.gz`, and Linux AppImage. DEB installations update through the package manager. CI smoke launches on hosted runners do not establish behavior on the oldest planned OS versions or physical game-launch acceptance.

Saved offline accounts can be added, selected and removed in the GUI without using the OS credential vault. Microsoft sign-in is temporarily unavailable in the GUI; the experimental CLI flow remains available, with full entitled-account acceptance still deferred. In the GUI, the Microsoft client ID is configured only in the Settings advanced section. CurseForge uses a user-supplied key, optionally saved in the OS credential vault; CCL does not bundle a project key. See the [account guide](docs/user-guide.en.md#accounts) and [release checklist](docs/development.en.md#release).

## Install and get started

1. Open [GitHub Releases](https://github.com/WinstonMeursault/CialloCraftLauncher/releases), choose a release and download the CLI ZIP or GUI installer for your target. CLI ZIPs include `.sha256` files; developer tags start with `dev-`, and stable tags use `v<VERSION>`. GitHub's automatic source archives are not executable packages.
2. Follow the platform installation instructions in the [user guide](docs/user-guide.en.md#installation). Install GUI packages in a writable user location if you want in-app updates. Use the system package manager to update a DEB installation.
3. CLI and GUI updaters verify detached signatures for their release manifests and then verify the named package hash. The installer files themselves are not Authenticode-signed or notarized, so operating systems may show publisher warnings.

CLI and GUI update manifests are signed with the shared Tauri updater key, binding product, version, channel, commit, target and package digest. Older CLI binaries need a manual bootstrap to the first release using signed manifests. GUI installer files remain OS-unsigned and macOS packages are not notarized; cryptographic update signatures are separate from OS publisher identity. Planned support floors are Windows 10 with WebView2, macOS 13, and Linux with glibc 2.39; they have not been physically verified. Linux Microsoft-account/provider-key storage requires an available, unlocked Secret Service/D-Bus session; there is no plaintext credential fallback.

To build the CLI from this checkout:

```sh
cargo build --release --locked -p ccl-cli
cargo run --release --locked -p ccl-cli -- --help
```

For the desktop GUI, follow the [desktop build instructions](docs/development.en.md#gui-g3), including frontend and notice generation. See the [development guide](docs/development.en.md#environment) for toolchain and native prerequisites. Builds do not bundle Minecraft, Java runtimes, mods or user credentials. Java can be downloaded later by the launcher when needed.

## Documentation

All public guides have aligned English and Simplified Chinese editions:

- [User guide](docs/user-guide.en.md) · [简体中文](docs/user-guide.zh-CN.md) — installation, configuration, account/key storage, vanilla/modded instances, imports/exports/backups, networking, updates and troubleshooting.
- [API reference](docs/api.en.md) · [简体中文](docs/api.zh-CN.md) — command/options, JSON contracts, core services, task events, errors and exit codes.
- [Development guide](docs/development.en.md) · [简体中文](docs/development.zh-CN.md) — architecture, builds/tests, packaging, release checklist, compatibility and service acceptance.
- [简体中文项目说明](README.zh-CN.md)

Report reproducible problems through [GitHub Issues](https://github.com/WinstonMeursault/CialloCraftLauncher/issues). Include the CCL version/target, command, expected/actual result and a reviewed diagnostic report where useful. Never include passwords, API keys, device codes, tokens or unreviewed raw logs.

## Acknowledgements

We thank the developers and contributors of Hello Minecraft! Launcher (HMCL) for their work on open-source Minecraft launchers. CialloCraftLauncher is independently developed; its GUI and CLI are separate applications that share the Rust core.

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
