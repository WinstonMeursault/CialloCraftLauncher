# CialloCraftLauncher

A libre, open-source, cross-platform Minecraft launcher written in Rust.

## Project status

The project is in early development. The CLI provides vanilla installation and launch, Java management, independent instance lifecycle and history cleanup, Fabric/Quilt/Legacy Fabric/Forge/NeoForge/OptiFine installation, Modrinth pack management, CurseForge and MultiMC/Prism imports, online content and dependency checks, local content backups, explicit network offline mode, launcher updates, and local diagnostics. P7 update and diagnostic APIs are implemented, but the full cross-platform upgrade acceptance matrix is still in progress. Release packages are unsigned; HTTPS and hashes do not independently authenticate the publisher. The core is independent of interfaces; a GUI has not been developed. Compatibility claims follow the actual validation matrices in the guides. Microsoft login is implemented, but this application's registration and real-account acceptance remain deferred. CurseForge uses a user-owned API key and has no live-key production acceptance. Builds and automated tests cover Windows x64, macOS arm64 and Linux x86_64.

## Documentation

- [User guide](docs/user-guide.en.md) · [简体中文](docs/user-guide.zh-CN.md) — installation, instances, packs, online content, updates, diagnostics, accounts and offline use.
- [API reference](docs/api.en.md) · [简体中文](docs/api.zh-CN.md) — CLI commands, JSON output, core services, update/diagnostic APIs, events and exit codes.
- [Development guide](docs/development.en.md) · [简体中文](docs/development.zh-CN.md) — architecture, build, validation, updates, compatibility, diagnostics and application registration.
- [简体中文项目说明](README.zh-CN.md)

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
