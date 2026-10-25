# CialloCraftLauncher

一个使用 Rust 编写的自由、开源 Minecraft Java 版启动器，面向 Windows、macOS 和 Linux。

## 项目状态

CCL 提供 Rust 核心、命令行界面和 Tauri 桌面 GUI。G3 增加独立桌面安装包与 GUI 应用包更新器；v0.2.0 稳定版发布需经用户明确同意。开发版属于预发布，不代表已完成稳定版验收。

| 方面 | 已提供的功能 |
| --- | --- |
| 安装与启动 | 原版版本、Java 探测/托管运行时、下载校验/缓存复用、进程日志与显式网络离线模式 |
| 实例与加载器 | 隔离实例；Fabric、Quilt、Legacy Fabric、Forge、NeoForge、OptiFine；修订版回退、重命名/复制/删除/历史和清理 |
| 整合包与内容 | Modrinth 整合包、受支持的 CurseForge/MultiMC/Prism 导入、在线搜索/依赖/更新/冲突分析、本地内容与用户数据备份 |
| 维护 | CLI 与 GUI 的稳定/开发签名更新、事务式安装/恢复/回退、本地诊断与回归流程 |

支持范围以指南中的样本和边界为准，不代表全部 Minecraft 版本、加载器组合或操作系统版本。CI 与发行工作流配置的目标平台为：

| 平台 | 归档目标名称 | 验证边界 |
| --- | --- | --- |
| Windows x64 | `x86_64-pc-windows-msvc` | 工作流目标：Windows 2025；尚无 Windows 物理游戏启动验收 |
| macOS Apple 芯片 | `aarch64-apple-darwin` | 工作流目标：macOS 15；历史样本不能证明最低系统版本 |
| Linux x64 | `x86_64-unknown-linux-gnu` | 工作流目标：Ubuntu 24.04；历史样本不能证明兼容所有 Linux 发行版 |

G3 发行工作流配置为 Windows x64 按用户安装的 NSIS EXE、macOS arm64 DMG，以及 Linux x64 AppImage 和 DEB。独立 GUI 验证工作流检查更新并手动回退、坏程序包自动回退及替换中断后的恢复；发行工作流检查渲染后的启动就绪状态。GUI 自更新支持 Windows 安装程序、macOS `.app.tar.gz` 和 Linux AppImage。DEB 安装应通过系统包管理器更新。托管运行器上的 CI 启动检查不能证明最低计划系统版本上的行为，也不代表已通过物理机游戏启动验收。

Microsoft 设备码登录、刷新、系统凭据存储与浏览器授权已实现。在线账户集成仍属实验性功能；所有者已暂缓本次发行的真实正版账户完整验收。继续支持离线身份。CurseForge 使用用户自行提供的 key，可选保存于系统凭据库；CCL 不内置项目 key。参阅[账户指南](docs/user-guide.zh-CN.md#accounts)及[发布清单](docs/development.zh-CN.md#release)。

## 安装与开始使用

1. 打开 [GitHub Releases](https://github.com/WinstonMeursault/CialloCraftLauncher/releases)，选择发行并下载目标平台的 CLI ZIP 或 GUI 安装程序。CLI ZIP 附带 `.sha256` 文件；开发标签以 `dev-` 开头，稳定标签为 `v<VERSION>`。GitHub 自动提供的源码归档不是可执行程序包。
2. 按[使用指南](docs/user-guide.zh-CN.md#installation)完成平台安装。若希望在应用中更新，请将 GUI 安装在用户可写的位置。DEB 安装应使用系统包管理器更新。
3. CLI 和 GUI 更新器会验证发行清单的独立签名，然后核对清单绑定的程序包哈希。安装文件本身没有 Authenticode 签名或公证，操作系统可能显示发布者警告。

CLI 与 GUI 更新清单使用共享的 Tauri 更新密钥签名，绑定产品、版本、渠道、提交、目标平台和程序包摘要。早于首个签名清单版本的 CLI 需要先手动更新。GUI 安装文件仍未使用操作系统发布者证书签名，macOS 程序包也未经公证；更新加密签名与操作系统发布者身份是两回事。计划的最低支持版本为 Windows 10（含 WebView2）、macOS 13，以及 glibc 2.39 的 Linux；尚未在这些最低版本上进行物理验证。Linux 的 Microsoft 账户/平台 key 存储需要可用且已解锁的 Secret Service/D-Bus 会话，没有明文凭据回退。

从当前源码构建：

```sh
cargo build --release --locked -p ccl-cli
cargo run --release --locked -p ccl-cli -- --help
```

工具链与本机构建依赖见[开发指南](docs/development.zh-CN.md#environment)。构建产物不附带 Minecraft、Java 运行时、模组或用户凭据；需要时启动器可另行下载 Java。

## 文档

全部公开指南均有对齐的英文和简体中文版：

- [使用指南](docs/user-guide.zh-CN.md) · [English](docs/user-guide.en.md) — 安装、配置、账户/key 保存、原版/模组实例、导入/导出/备份、网络、更新及排错。
- [API 文档](docs/api.zh-CN.md) · [English](docs/api.en.md) — 命令/选项、JSON 契约、核心服务、任务事件、错误与退出码。
- [开发指南](docs/development.zh-CN.md) · [English](docs/development.en.md) — 架构、构建/测试、打包、发布清单、兼容性与服务验收。
- [English project overview](README.md)

通过 [GitHub Issues](https://github.com/WinstonMeursault/CialloCraftLauncher/issues)报告可复现问题，提供 CCL 版本/目标平台、命令、预期/实际结果，以及必要且经过检查的诊断报告。不要附上密码、API key、设备代码、令牌或未经检查的原始日志。

## 致谢

感谢 Hello Minecraft! Launcher（HMCL）的开发者和贡献者为开源 Minecraft 启动器所做的工作。CialloCraftLauncher 独立开发；GUI 与 CLI 是共享 Rust 核心的独立应用程序。

## 许可证

版权所有 (C) 2026 WinstonMeursault

本程序是自由软件：您可以遵循自由软件基金会发布的 GNU Affero 通用公共许可证（GNU AGPL）第三版或（由您选择）任何更高版本的条款重新发布和/或修改本程序。

本程序按“原样”提供，不附带任何明示或暗示的担保，包括但不限于适销性或特定用途适用性的暗示担保。详情请参阅 GNU Affero 通用公共许可证。

您应已随本程序收到 GNU Affero 通用公共许可证副本。如未收到，请访问 <https://www.gnu.org/licenses/>。

SPDX 许可证标识：`AGPL-3.0-or-later`。完整许可证文本见 [LICENSE](LICENSE)。

## 免责声明

这是一个非官方 Minecraft 启动器，未经 Mojang 或 Microsoft 批准，也与其无关联。
