# CialloCraftLauncher

一个使用 Rust 编写的自由、开源、跨平台 Minecraft 启动器。

## 项目状态

项目处于早期开发阶段。`ccl` 命令行提供设置和账户管理、版本列表、校验原版安装、Java 运行时探测与安装、隔离的 Fabric/Forge/NeoForge 实例管理以及游戏启动。安装前会根据上游元数据检查加载器兼容关系，但这不能保证所有模组组合可用，也不代表已广泛验证实际游戏启动。CCL 不会捆绑 Fabric API 或模组。Microsoft 账户登录已经实现，但尚未使用本项目注册的应用及真实账户验证；启用账户的预览包尚未满足公开预览验收要求。参阅 [Microsoft 账户设置指南](docs/accounts.zh-CN.md)和[加载器指南](docs/loaders.zh-CN.md)。

## 文档

- [架构与设计边界（简体中文）](docs/architecture.zh-CN.md) · [English](docs/architecture.en.md)
- [命令行与输出（简体中文）](docs/cli.zh-CN.md) · [English](docs/cli.en.md)
- [开发指南（简体中文）](docs/development.zh-CN.md) · [English](docs/development.en.md)
- [Microsoft 账户设置（简体中文）](docs/accounts.zh-CN.md) · [English](docs/accounts.en.md)
- [模组实例与加载器（简体中文）](docs/loaders.zh-CN.md) · [English](docs/loaders.en.md)
- [English project overview](README.md)

## 致谢

感谢 Hello Minecraft! Launcher（HMCL）的开发者和贡献者为开源 Minecraft 启动器所做的工作。CialloCraftLauncher 独立开发，图形界面计划在未来提供，目前尚不可用。

## 许可证

版权所有 (C) 2026 WinstonMeursault

本程序是自由软件：您可以遵循自由软件基金会发布的 GNU Affero 通用公共许可证（GNU AGPL）第三版或（由您选择）任何更高版本的条款重新发布和/或修改本程序。

本程序按“原样”提供，不附带任何明示或暗示的担保，包括但不限于适销性或特定用途适用性的暗示担保。详情请参阅 GNU Affero 通用公共许可证。

您应已随本程序收到 GNU Affero 通用公共许可证副本。如未收到，请访问 <https://www.gnu.org/licenses/>。

SPDX 许可证标识：`AGPL-3.0-or-later`。完整许可证文本见 [LICENSE](LICENSE)。

## 免责声明

这是一个非官方 Minecraft 启动器，未经 Mojang 或 Microsoft 批准，也与其无关联。
