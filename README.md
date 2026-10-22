# CialloCraftLauncher

A libre, open-source, cross-platform Minecraft launcher written in Rust.

## Project status

The project is in early development. The `ccl` CLI provides settings and account management, version listing, verified vanilla installation, Java runtime discovery/installation, isolated Fabric/Forge/NeoForge instance management, and game launch. Minecraft 1.21.1 reached the game menu with Fabric 0.20.1 and Forge 52.1.0 on Linux, and with NeoForge 21.1.234 on macOS. Linux NeoForge installation remains unverified because the upstream checksum endpoint was unavailable and the mirror lacked the sidecar. These results do not establish broad compatibility. Loader metadata checks do not guarantee every mod set or loader/platform combination. Fabric API and mods are not bundled. Microsoft account sign-in is implemented but has not been verified with a project-owned registered application and a real account; the account-enabled preview has not met its public preview acceptance gate. See the [Microsoft account setup guide](docs/accounts.en.md) and the [loader guide](docs/loaders.en.md).

## Documentation

- [Architecture and design boundaries (English)](docs/architecture.en.md) · [简体中文](docs/architecture.zh-CN.md)
- [CLI commands and output (English)](docs/cli.en.md) · [简体中文](docs/cli.zh-CN.md)
- [Development guide (English)](docs/development.en.md) · [简体中文](docs/development.zh-CN.md)
- [Microsoft account setup (English)](docs/accounts.en.md) · [简体中文](docs/accounts.zh-CN.md)
- [Modded instances and loaders (English)](docs/loaders.en.md) · [模组实例与加载器（简体中文）](docs/loaders.zh-CN.md)
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
