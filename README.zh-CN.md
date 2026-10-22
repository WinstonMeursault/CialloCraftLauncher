# CialloCraftLauncher

一个使用 Rust 编写的自由、开源、跨平台 Minecraft 启动器。

## 项目状态

项目处于早期开发阶段。CLI 已提供原版安装和启动、Java 管理、独立实例生命周期与历史清理、Fabric/Quilt/Legacy Fabric/Forge/NeoForge/OptiFine 安装、Modrinth 整合包管理、CurseForge 与 MultiMC/Prism 导入、在线内容与依赖检查、本地内容备份、显式网络离线模式、启动器更新和本地诊断。P7 更新及诊断 API 已实现，但完整的跨平台升级验收矩阵仍在进行中。发行包未签名；HTTPS 和哈希不能独立认证发布者。核心与界面解耦，GUI 尚未开发。兼容性声明以各指南中的实际验证矩阵为准。Microsoft 登录已经实现，但本项目应用注册与真实账户验收仍暂缓；CurseForge 使用用户自有 API key，尚无真实 key 的生产验收。三平台构建和自动化测试覆盖 Windows x64、macOS arm64、Linux x86_64。

## 文档

- [使用指南](docs/user-guide.zh-CN.md) · [English](docs/user-guide.en.md) — 安装、实例、整合包、在线内容、更新、诊断、账户与离线使用。
- [API 文档](docs/api.zh-CN.md) · [English](docs/api.en.md) — CLI 命令、JSON 输出、核心服务、更新/诊断 API、事件和退出码。
- [开发指南](docs/development.zh-CN.md) · [English](docs/development.en.md) — 架构、构建、验证、更新、兼容性、诊断与应用注册。
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
