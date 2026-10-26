# 使用指南

[API 文档](api.zh-CN.md) · [开发指南](development.zh-CN.md) · [English](user-guide.en.md)

- [快速开始](#quickstart)
- [桌面应用与自更新](#desktop-alpha)
- [安装与前置条件](#installation)
- [游戏目录](#directories)
- [设置与配置](#configuration)
- [加载器与安装](#loaders)
- [实例生命周期与历史](#instances)
- [整合包、本地内容与备份](#packs)
- [CurseForge 与 MultiMC/Prism 导入](#imports)
- [在线搜索、依赖与更新](#online)
- [网络与离线模式](#networking)
- [启动器更新与诊断](#maintenance)
- [Microsoft 账户](#accounts)
- [故障排查](#troubleshooting)

<a id="quickstart"></a>

## 快速开始

可用 `--game-dir PATH` 显式选择游戏根目录。`doctor` 检查状态，`init` 创建所选目录。将 `1.21.1` 替换为需要的 Minecraft 版本。`launch --offline` 选择离线玩家身份，仍允许下载；[网络离线模式](#networking)是单独的设置。

```sh
ccl --help
ccl doctor
ccl init
ccl versions --limit 10
ccl install 1.21.1
ccl launch 1.21.1 --offline --dry-run
ccl launch 1.21.1 --offline
ccl logs --lines 100
```

程序自动探测或安装匹配游戏版本及主机架构的 Java。`--java PATH` 可显式指定运行时，`--no-java-download` 禁用运行时安装。`--json` 提供结构化输出，参阅[命令及输出参考](api.zh-CN.md#cli)。

`versions`、`loaders` 和 `online search` 默认最多返回 20 项。`versions` 和 `loaders` 使用 `--limit 0` 可返回全部结果；在线搜索数量受平台上限限制。`logs` 默认显示最新 100 行。

<a id="desktop-alpha"></a>

## 桌面应用与自更新

本指南对应 v0.2.1，包含 CLI、核心、桌面应用和签名清单更新器。已发布程序包请查看 [GitHub Releases 页面](https://github.com/WinstonMeursault/CialloCraftLauncher/releases)。GUI 程序包包括 Windows x64 按用户安装的 NSIS EXE、macOS arm64 DMG，以及 Linux x64 AppImage 或 DEB。平台目标和发行要求见开发指南。

桌面程序会先让你确认游戏目录，再创建缺失的目录。它可以安装原版、创建托管加载器实例、导入受支持的本地归档、使用已保存的离线账户启动游戏，并查看实时或已保存的游戏日志。可添加、选择和移除多个离线账户；这些本地玩家名不使用系统凭据库。GUI 中的 Microsoft 登录暂时不可用，实验性的 CLI 登录流程仍保留。公开的 Microsoft 客户端 ID 仅在“设置 → 高级设置”中配置（`auth.microsoft_client_id`）。GUI 的网络离线设置会保存到共享启动器配置，因此也会影响 CLI。选择离线玩家身份不会关闭下载或其他网络活动。

同一时间只能运行一个安装、导入或启动任务。切换页面时任务会继续运行。可收起的进度弹窗显示任务进度，可通过右下角任务入口重新打开；游戏进程启动后会自动收起。安装和导入任务可以取消；停止游戏前会请求确认。任务注册表最多保留八个已完成快照，弹窗最多显示最近 30 行任务日志。活动任务期间关闭程序时，会询问是否取消任务，或停止游戏并退出。再次启动时会聚焦已有窗口。

在就绪窗口中拖入一个 `.zip` 或 `.mrpack` 可检查归档并打开导入对话框。选择或编辑实例名称和导入选项，检查摘要后再明确开始导入。若实例 ID 已存在，操作会被拒绝；程序不会自动替换实例。也可使用原生归档选择器。内容文件、备份/导出目标、诊断文件和 CLI 安装目录都应通过相应的原生选择器选取，以授予程序访问所选路径的权限。

高级页面用于管理托管实例。选中实例后，可安装/更新/移除加载器、查看历史和存储、预览或执行修订版本清理、还原或回滚修订版本、管理内容、创建或还原用户数据备份，以及检查/导出 Modrinth 整合包收据。共享原版条目在此页面为只读。在“发现”页选择 Modrinth 或 CurseForge，搜索并选择项目和版本，然后可查看版本或解析依赖后再安装。模组安装目标为当前选中的托管实例；整合包安装会先请求确认，因为它可能创建或更新指定实例。除非勾选，否则不会包含可选文件。

设置页将外观和游戏根目录与共享核心设置分开。维护面板提供核心下载和启动设置。Microsoft 客户端 ID 仅位于“设置”的高级折叠区；GUI 登录暂时不可用。每项分别保存。若 CLI 或其他进程修改了配置，请先重新加载；陈旧修改会被拒绝。除非 `CCL_GUI_CONFIG_PATH` 固定配置，否则可以选择本地 JSON 配置。CurseForge 密钥通过密码输入框录入并保存在系统凭据库。`CCL_CURSEFORGE_API_KEY` 会覆盖已保存值；状态只显示是否存在已保存密钥或环境变量覆盖。维护面板也分别提供 GUI 应用包更新和独立 CLI 更新控制。

健康检查只有在勾选对应选项后才会执行网络或临时写入探测。诊断导出保存在本地。原始日志预览必须明确启用，尽管会尽力脱敏，仍可能含私人信息；分享前请检查文件。CLI 更新器作用于你选定目录中的独立 CLI；GUI 更新控制只作用于当前安装的桌面程序包。两者均支持 stable（`main`）和 dev（`dev`）渠道，并会保存所选渠道。

<a id="installation"></a>

## 安装与前置条件

从 [GitHub Releases 页面](https://github.com/WinstonMeursault/CialloCraftLauncher/releases)下载适用于系统的 CLI 归档。请选用 v0.2.1 以获得本文所述行为；开发版另行发布。CLI 归档名称如下：

| 系统 | 归档文件 |
| --- | --- |
| Windows x86_64 | `ccl-x86_64-pc-windows-msvc.zip` |
| macOS arm64 | `ccl-aarch64-apple-darwin.zip` |
| Linux x86_64 | `ccl-x86_64-unknown-linux-gnu.zip` |

桌面程序包使用独立名称和安装格式。v0.2.1 桌面应用采用以下程序包格式：

| 系统 | 安装程序包 | 应用内更新程序包 |
| --- | --- | --- |
| Windows x86_64 | `ccl-gui-x86_64-pc-windows-msvc.exe`（按用户安装的 NSIS） | NSIS EXE；安装到用户可写位置 |
| macOS arm64 | `ccl-gui-aarch64-apple-darwin.dmg` | `.app.tar.gz`；将 `.app` 保存在可写位置 |
| Linux x86_64 | `ccl-gui-x86_64-unknown-linux-gnu.AppImage` 或 `.deb` | AppImage 支持应用内更新；DEB 通过包管理器更新 |

GUI 安装程序与 CLI ZIP 使用不同名称。Windows 自更新支持按用户安装的 NSIS 程序，不支持便携 EXE。macOS 和 Linux 应用包更新需要对已安装 bundle 或 AppImage 位置有写权限。受保护位置或包管理器安装应通过原安装方式重新安装更新。

CLI ZIP 发行资产附带 `.sha256` 校验文件。校验和可以发现下载损坏，但不能认证发行者。GUI 应用内更新器会验证独立签名清单及其绑定的程序包摘要。手动安装的 GUI 程序包没有操作系统签名；公钥更新签名与 Authenticode 和 macOS 公证不同。

同时下载旁边的 `.sha256` 校验文件，并在解压前校验归档。Windows PowerShell 中，请在下载目录运行：

```powershell
$archive = "ccl-x86_64-pc-windows-msvc.zip"
$expected = (Get-Content "$archive.sha256").Split()[0]
$actual = (Get-FileHash $archive -Algorithm SHA256).Hash.ToLowerInvariant()
if ($actual -ne $expected) { throw "SHA-256 mismatch" }
```

Linux 上运行 `sha256sum -c ccl-x86_64-unknown-linux-gnu.zip.sha256`。macOS 上运行 `shasum -a 256 -c ccl-aarch64-apple-darwin.zip.sha256`。请在归档和校验文件所在目录运行；只有命令报告匹配后才继续。校验和可发现归档更改或传输损坏，但不能单独验证发布者身份。

解压整个 CLI 归档文件夹，并将可执行文件、README、`docs/` 目录、`LICENSE` 和第三方声明文件一并保留。Windows 程序名为 `ccl.exe`，macOS/Linux 上为 `ccl`。在 PowerShell 的该目录中运行 `.\ccl.exe --help`；在 macOS/Linux 上运行 `./ccl --help`。如果解压时 Unix 可执行权限丢失，运行一次 `chmod u+x ./ccl`。请将解压目录保存在可写位置：CLI 更新器会在可执行文件所在目录旁创建 `.ccl-update` 来保存事务文件。可将该目录加入 `PATH`，或通过完整路径调用程序。CLI 归档不包含桌面应用。v0.2.1 也提供桌面应用；dev 预发行版可能包含较新的 GUI 改动。更新完整性说明见[启动器更新](#maintenance)。

安装 CCL 不需要 Java。首次启动游戏时，CCL 可下载与 Minecraft 版本和系统架构匹配的 Java 运行时。如需手动指定 Java，请安装兼容运行时，并通过 `--java PATH` 传入可执行文件或 Java 主目录。发现版本及下载游戏文件、托管 Java、加载器、整合包或更新需要网络连接。请确保所选游戏目录及 CCL 配置/数据目录可写。Linux 上的 Microsoft 登录和已保存的 CurseForge 密钥还需要可用的系统凭据服务；Microsoft 登录特别要求当前 D-Bus 会话可访问且已解锁的 Secret Service。

上述目标表示构建和打包覆盖范围，不代表已认证最低操作系统版本。Minecraft 主菜单验证范围更窄，见各功能章节；当前证据不包含 Windows 物理机游戏启动验证。Linux 启动游戏需要可用的显示服务和图形驱动；CI 成功本身不能验证用户机器的图形环境。


### macOS 签名检查与首次启动

macOS 构建配置会对完整 `.app`（包括 Info.plist 和资源）进行 ad-hoc 临时签名。这不是 Developer ID 发布者签名，也不是 Apple 公证。安装可信程序包后，macOS 仍可能要求通过**系统设置 → 隐私与安全性 → 仍要打开**逐应用批准，详见 [Apple 说明](https://support.apple.com/en-us/102445)。更新清单签名不能替代这项操作系统检查。

原始 v0.2.0 macOS DMG 与更新归档只有不完整的链接器签名，可能显示“App 已损坏”。下载校验和匹配不会修复此签名。v0.2.1 构建配置已修正此问题，打包时会验证完整应用包、DMG 和更新归档；现有 v0.2.0 资产不会更改。安装 v0.2.1 即可获取修正后的程序包。对于已下载且已按该发行版校验和验证的副本，可用以下命令修复本地应用包签名；请替换为实际安装路径。这只会创建本地 ad-hoc 签名，不会建立 Apple 发布者信任。若验证失败，请停止并替换该副本。随后如系统提供逐应用批准，请按 Apple 说明操作；不要全局关闭 Gatekeeper。

```sh
codesign --force --sign - "/Applications/CialloCraftLauncher.app"
codesign --verify --deep --strict --verbose=2 "/Applications/CialloCraftLauncher.app"
```

### 首次运行

运行 `ccl doctor` 检查配置、目录访问、Java 候选项和凭据库适配器状态。然后运行 `ccl init` 创建所选游戏目录，`ccl versions` 检查版本发现，再用 `ccl install VERSION` 安装版本。除非显式请求探测，`doctor` 不会写入文件。`init` 只创建游戏目录，不安装 Minecraft。使用离线玩家身份运行 `ccl launch VERSION --offline --dry-run` 可预览启动，然后移除 `--dry-run` 实际启动。首次启动可能需要下载 Java 和剩余游戏文件。

```sh
ccl doctor
ccl init
ccl versions --limit 10
ccl install 1.21.1
ccl launch 1.21.1 --offline --dry-run
```

<a id="directories"></a>

## 游戏目录

默认游戏目录按以下顺序选择：

1. 显式指定的 `--game-dir` 路径。
2. 当前工作目录中已存在的 `.minecraft` 目录。
3. 平台惯用的 Minecraft 数据目录；macOS 上可复用已存在的目录。
4. 用户主目录中的 `.minecraft`。
5. 在当前工作目录创建 `.minecraft`。

`doctor` 检查配置和目录状态，不创建或修改游戏目录。`init` 按此顺序选择并创建游戏目录。

<a id="configuration"></a>

## 设置与配置

用户配置采用 JSON，默认位置为：

- Windows：`%APPDATA%/CialloCraftLauncher/config.json`
- macOS：`~/Library/Application Support/CialloCraftLauncher/config.json`
- Linux：`$XDG_CONFIG_HOME/ciallocraftlauncher/config.json`；未设置该变量时为 `~/.config/ciallocraftlauncher/config.json`

镜像根地址必须是无凭据的 HTTPS URL，未知字段会被拒绝。显示设置时会隐藏代理凭据。CLI 在 Unix 上写入的配置文件仅允许所有者访问。默认配置文件缺失时使用默认值。读取配置的命令要求显式指定的 `--config PATH` 文件已存在，包括 `settings show` 和 `settings get`。`update status`、`update rollback` 和 `update recover` 不会读取该文件。`settings set` 和 `settings reset` 会从默认值初始化缺失文件；`settings reset --all` 也可用默认配置替换格式错误的文件。

配置字段和默认值如下：

| 设置 | 默认值 | 有效值 |
| --- | --- | --- |
| `downloads.offline` | `false` | `true` 或 `false` |
| `downloads.version_list_source` | `auto` | `auto`、`official` 或 `mirror` |
| `downloads.file_source` | `auto` | `auto`、`official` 或 `mirror` |
| `downloads.mirror_root` | `https://bmclapi2.bangbang93.com` | 不含凭据的 HTTPS URL |
| `downloads.http_proxy` | `null` | 代理 URL 或 `null` |
| `downloads.max_parallel` | `8` | 1 到 64 的整数 |
| `auth.microsoft_client_id` | `null` | 公开 OAuth 客户端 ID 或 `null` |
| `launch.java` | `null` | Java 可执行文件路径、Java 主目录或 `null` |
| `launch.max_memory_mb` | `2048` | 256 到 131072 MiB 的整数 |
| `launch.auto_install_java` | `true` | `true` 或 `false` |

以下完整配置使用所有默认值：

```json
{
  "downloads": {
    "offline": false,
    "version_list_source": "auto",
    "file_source": "auto",
    "mirror_root": "https://bmclapi2.bangbang93.com",
    "http_proxy": null,
    "max_parallel": 8
  },
  "auth": {
    "microsoft_client_id": null
  },
  "launch": {
    "java": null,
    "max_memory_mb": 2048,
    "auto_install_java": true
  }
}
```

对于支持映射的 Minecraft 地址，`auto` 先尝试官方地址再尝试配置的镜像；`official` 仅使用官方地址；`mirror` 先尝试镜像再尝试官方地址。未知主机保留原始地址。OptiFine 使用配置的镜像端点。这些设置不会重定向 Modrinth、CurseForge 或 GitHub 请求。

配置中可省略字段，省略时使用默认值。`--memory` 可在单次启动中覆盖 `launch.max_memory_mb`，接受 256–131072 MiB。版本列表和安装流程使用官方版本清单及元数据。安装会下载客户端、库、原生库、资源索引和资源文件；依据可用元数据中的哈希和大小校验，限制并发数，重试临时失败，仅在续传条件经过验证时继续下载，复用有效缓存文件，并先写入临时文件再替换目标。多个进程通过版本锁协调，也支持取消操作。已有版本 JSON 冲突时会返回冲突错误而不会覆盖。

CurseForge API 请求需要用户提供对相应 API 和文件下载有权限的密钥。[CurseForge API 文档](https://docs.curseforge.com/rest-api/)介绍密钥流程；实际访问取决于服务方授权。获得密钥后运行 `ccl credentials curseforge set`，通过隐藏输入的终端提示录入。CCL 将密钥保存到系统凭据库，按所选配置文件路径隔离，不写入 `config.json`。`ccl credentials curseforge status` 会显示是否已保存密钥及是否启用环境变量覆盖；`ccl credentials curseforge remove` 可删除。`CCL_CURSEFORGE_API_KEY` 仍可作临时环境变量覆盖，优先于已保存密钥。不要通过命令参数、shell 历史、日志或仓库文件传递密钥。必须有可用的系统凭据库（Linux 通常需要运行 Secret Service 会话）；CCL 不会降级为明文保存。仅当 CurseForge 归档选择的远程文件需要解析时才需要该密钥；不含所选远程文件的纯 overrides 归档无需密钥。密钥只会发送到获准的服务 API/CDN 请求。基于测试夹具的请求头范围检查不能证明已获得真实服务授权。参阅[导入](user-guide.zh-CN.md#imports)和[在线内容](user-guide.zh-CN.md#online)。

启动设置还包括 `launch.java`（可选的 Java 可执行文件或主目录）、`launch.max_memory_mb`（默认 `2048`）以及 `launch.auto_install_java`（默认 `true`）。`--java` 可覆盖配置路径；`--no-java-download` 可在本次启动时跳过托管 Java 安装。托管运行时安装在 `<游戏目录>/.ccl/runtimes`，使用 Mojang 官方运行时清单和校验文件。`auth.microsoft_client_id` 是可选的公开 OAuth 应用 ID，不是客户端密钥。GUI 仅在“设置 → 高级设置”显示此项；GUI 登录暂时不可用。实验性的 CLI 流程仍使用该 ID。参阅[账户设置](user-guide.zh-CN.md#accounts)。

`settings get` 和 `settings show` 会隐藏代理 URL 中的用户名和密码。为网络连接所需，配置文件仍会保存完整代理 URL，因此应保护该文件。在 Unix 系统上，启动器写入的配置文件仅允许文件所有者访问。设置会先校验，再通过原子替换写入；多个 CLI 进程之间使用锁协调更新。`settings reset --all` 也可将损坏的配置替换为默认值。

设置示例：

```sh
ccl settings show
ccl settings get downloads.max_parallel
ccl settings set downloads.max_parallel 6
ccl settings set downloads.http_proxy null
ccl settings reset launch.java
```

<a id="loaders"></a>

## 加载器与安装

CCL 可以查询加载器元数据，并将 Fabric、Quilt、Legacy Fabric、Forge、NeoForge 和 OptiFine 安装到彼此隔离的托管实例中。安装前会根据对应提供方的版本列表检查兼容关系。该检查只确认所选版本是否被列为适用于指定 Minecraft 版本；它不会验证所有模组、整合包、Java 环境或启动组合。

Fabric 和 Legacy Fabric 配置文件来自各自的 Meta API；Quilt 配置文件来自 Quilt Meta。Forge 支持处理器安装器、含嵌入式 Maven 文件的 spec 0 配置，以及较早的 `install`/`versionInfo` 通用 JAR 格式。NeoForge 使用处理器安装器和嵌入式运行库。无法识别的安装器结构会明确返回不支持格式错误。

OptiFine 版本列表通过已配置的 BMCLAPI 兼容镜像获取。CCL 会通过 HTTPS 下载全新的安装器，检查归档大小及其中声明的 Minecraft/OptiFine 版本；若安装器包含内置补丁程序，则会针对原版客户端 JAR 运行该程序。有内置 LaunchWrapper JAR 时优先安装使用；旧安装器不包含时回退到 Maven LaunchWrapper 1.12。OptiFine 此元数据接口不提供校验和，因此 CCL 不会把之前下载的安装器作为已验证缓存复用。

已完成隔离冷安装及 macOS 游戏主菜单验证：Minecraft `1.21.4` 的 Quilt `0.20.0-beta.9`、Minecraft `1.21.1` 的 NeoForge `21.1.234`，以及 Minecraft `1.20.1` 的 OptiFine `HD_U_I6`。Minecraft `1.8.9` 的 Legacy Fabric `0.20.1`、Minecraft `1.12.2` 的 Forge `14.23.5.2860` 在 macOS 和 Linux 安装成功；Linux 客户端通过虚拟显示和软件渲染完成游戏启动。这些结果不代表其他版本、平台或第三方模组已验证。

### 查询加载器版本

```sh
ccl loaders fabric 1.21.1
ccl loaders quilt 1.21.4
ccl loaders legacyfabric 1.8.9
ccl loaders forge 1.20.1 --limit 10
ccl loaders neoforge 1.21.1 --limit 10
ccl loaders optifine 1.20.1
```

命令会按从新到旧的顺序列出兼容的加载器版本。将所选版本字符串传给 `instance install` 或 `instance update`。

### 管理隔离实例

```sh
ccl instance install my-world 1.21.1 fabric 0.20.1
ccl instance install my-quilt-world 1.21.4 quilt 0.20.0-beta.9
ccl instance install my-optifine-world 1.20.1 optifine HD_U_I6
ccl instance list
ccl instance show my-world
ccl launch --instance my-world --dry-run
ccl logs --instance my-world
ccl instance update my-world 1.21.1 fabric 0.20.1
ccl instance rollback my-world
ccl instance uninstall-loader my-world
```

实例 ID 支持 UTF-8 名称，包括中文/日文、空格和安全标点。拒绝 `/`、`\`、`:`、`*`、`?`、`"`、`<`、`>`、`|`、控制字符、末尾点号/空格、`.`/`..` 以及 Windows 保留设备名。校验只改变允许使用的 ID，不会移动已有游戏目录，也不会新增元数据字段。

每个托管实例都拥有独立的游戏目录、版本元数据、模组、配置和存档，不会与其他实例共用所选的原版游戏目录。安装或更新会先构建暂存版本，成功后再切换到该版本。更新会保留上一个成功版本，供 `rollback` 切换。移除加载器会让实例恢复为原版，同时保留用户游戏数据。这些操作会在不同版本间保留模组、配置和存档。

`instance show` 中的 `loader_version` 记录最终安装配置文件的 ID，例如 `fabric-loader-0.20.1-1.21.1`；它不一定等于命令行传入的加载器发行版本字符串。启动实例时会使用其已安装配置文件和独立游戏目录。

CCL 不会捆绑 Fabric API、模组或整合包。请另行将它们安装到实例的 `mods` 目录。上游兼容元数据只用于安装前检查，不能保证特定模组组合一定可用。

每个实例同时只选择一个加载器。模组应适配该加载器及 Minecraft 版本；Fabric、Forge、NeoForge 模组文件通常不能互换。更换加载器时会保留原有 `mods` 目录，因此使用新加载器启动前应检查其中的文件。移除加载器后仍保留这些文件，但原版游戏不会加载它们。

Forge 系安装器首先使用官方 Maven SHA-1 文件校验。如果该文件不可用，CCL 会获取官方 SHA-512 文件，先验证安装器字节，再派生安装流程使用的 SHA-1。只有安装器 URL、记录的 SHA-1 和文件大小全部匹配时才会复用本地记录。如果两种上游校验和都无法获取且没有有效匹配记录，安装会失败。离线安装可以复用已验证的安装器记录，但其他所需文件也必须已在本地；查询加载器版本仍需联网。

元数据和校验和来源：[Fabric Meta](https://meta.fabricmc.net/)、[Quilt Meta](https://meta.quiltmc.org/)、[Legacy Fabric Meta API](https://github.com/Legacy-Fabric/legacy-meta)、[OptiFine 下载页](https://optifine.net/downloads)、[Forge Maven](https://maven.minecraftforge.net/) 和 [NeoForge Maven](https://maven.neoforged.net/releases/)。

这些命令也支持 `--json`。`loaders` 返回 `loader_versions`；`instance list`、`show` 和生命周期操作会返回 `managed_instances` 或 `instance` 记录。网络和安装错误遵循通用 [CLI 退出码](api.zh-CN.md#exit-codes)。

<a id="instances"></a>

## 实例生命周期与历史

这些命令操作所选游戏根目录下的托管实例，使用与启动相同的稳定实例锁；运行中的实例不能被重命名、复制、删除、恢复或清理。

```sh
ccl instance copy source copy
ccl instance rename copy renamed
ccl instance history renamed
ccl instance disk-usage renamed
ccl instance restore renamed previous
ccl instance clean renamed --keep 2
ccl instance clean renamed --keep 2 --apply
ccl instance delete renamed --yes
```

复制保留当前实例、上一版本和归档版本。可变文件、存档、配置均为独立副本；已校验资源文件可能使用硬链接共享。不会复制未完成的暂存版本。重命名改变实例 ID 并更新元数据路径，移动目录前会记录恢复信息。ID 可使用 UTF-8 字符（包括中日文）、空格和安全标点，最长 64 个字符且不超过 240 个 UTF-8 字节。拒绝控制字符、`<>:"/\|?*`、`.`/`..`、末尾点号/空格，以及 Windows 保留名 `CON`、`PRN`、`AUX`、`NUL`、`COM1`–`COM9`、`LPT1`–`LPT9`（包括带扩展名的形式）；重命名还会拒绝仅大小写不同或不区分大小写的目标冲突。

提交保留直接上一版本供 `rollback` 使用，更早的上一版本归档至 `history/`。`history` 列出 `current`、可用的 `previous` 及归档 ID。`restore ID REVISION` 从完整保留快照创建新的当前版本，包含快照中的存档和配置，并保留原当前版本供回滚。`restore` 接受 `previous` 或 `history` 输出的归档 ID。恢复前请备份较新的存档。加载器回滚会先将当前存档、设置和其他用户数据准备到上一版本，再执行交换；若操作中断，恢复流程会保留一致的当前版本和回滚版本。完整内容编辑或整合包更新后的回滚会交换完整快照。

`disk-usage` 显示当前、上一版本、历史版本及未完成暂存数据的逻辑字节数。由于硬链接，这些数字不代表独占物理磁盘占用，也不保证能释放对应空间。

`clean` 默认预览目标。`--keep N` 保留最新 N 个归档版本，默认 2；清理也会针对未完成暂存版本。`--include-previous` 显式纳入直接回滚版本。`--apply` 在持锁状态下执行计划。当前存档和内容永远不是清理目标。清理不扫描共享根目录的库/资源、其他启动器文件、账户凭据或当前实例中已安装的 Java 运行时。

`delete --yes` 永久移除整个指定实例，包括存档和全部历史，请先备份。它不会删除其他实例和共享根目录资源。实例目录外的稳定锁文件会有意保留：删除锁文件可能导致两个进程绕过彼此的锁。

<a id="packs"></a>

## 整合包、本地内容与备份

CCL 可检查、导入和导出 Modrinth .mrpack v1 归档（含 Quilt 依赖），并管理独立实例内容。CurseForge、MultiMC/Prism 导出通过[其他格式转换器](user-guide.zh-CN.md#imports)导入。桌面应用可通过文件选择器检查受支持的本地归档，详见[桌面应用与自更新](#desktop-alpha)。

已验证样本使用 Minecraft `1.21.1`、Fabric Loader `0.20.1`、Fabric API `0.116.17+1.21.1`，在 macOS arm64 及使用 CPU 渲染的 Linux x86_64 到达主菜单。macOS 上导出后重新导入的实例也到达主菜单，模组哈希与覆盖文件均匹配。这不代表其他整合包或第三方模组组合已认证。Windows 通过编译和自动化测试检查。

### 检查和导入整合包

```sh
ccl pack inspect ./pack.mrpack
ccl pack import my-pack ./pack.mrpack
ccl pack show my-pack
```

`inspect` 会验证归档并输出清单，不会安装。`show` 会显示已导入实例记录的清单和文件归属信息。导入支持整合包声明的 Minecraft、无加载器、Fabric、Quilt、Forge 或 NeoForge。默认跳过可选客户端文件；如需包含，请使用：

```sh
ccl pack import my-pack ./pack.mrpack --include-optional
```

将另一个归档导入已有托管 ID 会更新该实例。CCL 会检查托管整合包文件是否仍与记录的哈希一致。如果本地托管文件已修改或删除，或新整合包路径与未托管文件冲突，更新会以退出码 `7` 停止。请先备份本地内容；若明确放弃这些本地改动，可以使用：

```sh
ccl pack import my-pack ./pack-v2.mrpack --replace-modified
```

`--replace-modified` 允许更新按需替换或删除冲突的本地文件。请将其视为允许丢弃这些本地改动。

### 下载来源与完整性

整合包文件 URL 必须使用 HTTPS。默认允许 Modrinth CDN、GitHub 和 GitLab 主机，并允许 GitHub 发布资产重定向主机。每次重定向都会按同一 HTTPS 主机策略重新检查，最多跟随 10 次。若整合包使用其他来源，可添加明确受信任的完整主机名：

```sh
ccl pack import my-pack ./pack.mrpack --allow-host downloads.example.org
```

可重复使用 `--allow-host` 添加多个主机。CCL 在文件放入实例前会验证 SHA-1、SHA-512 和声明的文件大小。`.mrpack` 归档最多包含 100,000 个条目；单个文件最大 128 MiB，未压缩数据总量最大 512 MiB。

### 导出整合包

```sh
ccl pack export my-pack ./my-pack.mrpack
```

默认导出会保留已验证的远程文件引用，并将本地配置和游戏选项作为覆盖文件加入归档；不会上传任何内容。已修改的文件无法继续用旧远程引用表示。新增或未跟踪的模组、资源包和光影包二进制文件必须明确启用 `--include-local-content` 才会导出：

导出也支持通过 `instance install` 创建的普通托管实例，包括原版实例。Minecraft 与加载器依赖从已安装实例元数据中取得。没有整合包记录的实例不具备固定远程引用，因此导出其本地二进制文件同样需要明确启用许可选项。

```sh
ccl pack export my-pack ./my-pack.mrpack --include-local-content
```

只有在你有权重新分发所有纳入的二进制文件时，才使用该选项。CCL 不会替你检查许可证或取得授权。分享归档前，请阅读 Modrinth 的[`.mrpack` 格式规范](https://support.modrinth.com/en/articles/8802351-modrinth-modpack-format-mrpack)和[整合包权限指南](https://support.modrinth.com/en/articles/8797527-obtaining-modpack-permissions)。CCL 不会发布或上传导出的文件。

存档和 `servers.dat` 不会导出到整合包；请使用用户数据备份保存它们。

### 管理本地内容

```sh
ccl content list my-pack mods
ccl content list my-pack resourcepacks
ccl content list my-pack shaderpacks
ccl content list my-pack saves
ccl content add my-pack mods ./local-mod.jar
ccl content add my-pack saves ./world --name MyWorld
ccl content remove my-pack mods local-mod.jar
ccl content enable my-pack example.jar
ccl content disable my-pack example.jar
```

`content add` 会复制来源文件，保留原文件不动。模组必须是 `.jar` 或 `.jar.disabled` 文件；资源包和光影包必须是 `.zip` 文件；存档必须是目录。`--name` 为可选参数，用于指定目标名称。启用和禁用操作仅适用于 `.jar` 模组。

### 备份和恢复用户数据

```sh
ccl content backup my-pack ./my-pack-userdata.zip
ccl content restore my-pack ./my-pack-userdata.zip
```

备份包含 `mods`、`config`、`resourcepacks`、`shaderpacks`、`saves`、`defaultconfigs`、`datapacks`、`options.txt` 和 `servers.dat` 中的用户数据。备份不包含 Minecraft 或加载器文件、Java 运行时或账号凭据。恢复会替换归档声明的每个根目录或文件；备份中标记为缺失的根也会从实例中删除。如果还需要当前文件，请先备份再恢复。备份最多包含 100,000 个条目，文件数据总量上限为 2 GiB。

### 版本快照与正在运行的实例

整合包导入/更新和内容编辑会先在暂存的实例版本中准备，再通过实例操作锁提交。托管游戏正在持有该锁运行时，这些操作无法替换游戏目录。整合包或内容变更后执行 `instance rollback` 会恢复完整的上一版快照，包括其中的用户数据和存档。如果需要保留较新的存档，请先备份。加载器回滚会切换加载器版本，同时保留当前用户数据。

错误处理见 [CLI 退出码](api.zh-CN.md#exit-codes)，这些命令支持 `--json`。标准 Modrinth 依赖不能表示独立 OptiFine 或 Legacy Fabric；这些托管实例应使用用户数据备份，不生成会误导可移植性的导出。

<a id="imports"></a>

## CurseForge 与 MultiMC/Prism 导入

`pack inspect ARCHIVE` 无需下载或安装即可检查归档。`pack import ID ARCHIVE` 根据清单识别 Modrinth、CurseForge、MultiMC/Prism 导出。标准导出的清单可位于 ZIP 根目录或单层包裹文件夹中。Modrinth 文件无需使用 `.mrpack` 扩展名也能被识别。

```sh
ccl pack inspect ./export.zip
ccl pack import imported ./export.zip
ccl pack import imported ./export-v2.zip --replace-modified
```

### CurseForge

CCL 从 `manifest.json` 读取 Minecraft 和加载器版本，通过官方 API 解析固定的项目/文件 ID，校验声明的 SHA-1 和大小，并以事务导入覆盖文件及选定客户端文件。默认跳过可选文件，使用 `--include-optional` 可包含它们。文件下载仅向明确识别的 CurseForge HTTPS CDN 主机发送 `x-api-key`；不会把密钥加入归档 URL、收据或日志。重定向到其他允许主机时不会发送密钥。

远程文件解析需要用 `ccl credentials curseforge set` 保存自己的 API key，或临时设置 `CCL_CURSEFORGE_API_KEY`。不要把密钥放入命令参数或提交到仓库。不含选定远程文件的纯覆盖归档可在没有 key 时转换。若受限文件没有提供者授权的下载 URL，会明确报错；CCL 不猜测下载 URL，也不绕过作者的分发选择。API 和 CDN 要求见 [CurseForge 文档](https://docs.curseforge.com/rest-api/)及其 [CDN 身份验证公告](https://blog.curseforge.com/introducing-api-key-authentication-for-curseforge-file-downloads/)。

提供者行为已通过服务和格式测试夹具检查，但这不能证明生产密钥已获得在线授权或文件下载权限。不同提供者错误信息有所不同；请依据命令返回的消息及服务方访问要求排查。

### MultiMC 和 Prism

CCL 读取 `mmc-pack.json` 组件，并从 `minecraft/` 或 `.minecraft/` 导入本地游戏内容，包括支持的模组、配置、内容包、存档、选项和服务器列表。加载器/版本要求通过 CCL 现有安装器解析。不会执行导出的实例钩子，也不会导入账户数据。未知组件和自定义补丁会明确拒绝，避免静默生成不同实例。拒绝多个加载器。当前转换器不导入任意自定义 Java 参数、启动器专属设置或账户数据库。

### 边界与恢复

导入复用 Modrinth 导入的操作锁、暂存版本提交和文件归属冲突检查。`--replace-modified` 允许替换冲突的托管内容，使用前请备份本地数据。来源归档保持不变。读取内容前预检条目数、声明的未压缩大小、路径穿越、符号链接、大小写冲突及文件/目录冲突。限制为 100,000 条目、单条目 128 MiB、未压缩内容总量 512 MiB、清单 JSON 2 MiB。CurseForge 文件元数据响应也限制为 2 MiB；取消操作可中断等待中的元数据请求和文件下载。下载文件计入转换后整合包的大小限制。

这些导入会保留归档中的私人用户数据，例如存档和服务器列表。分享导出的 `.mrpack` 是另一项操作：导出排除存档/服务器列表，本地二进制文件重分发仍须使用 `--include-local-content`。格式转换不授予重分发许可。

<a id="online"></a>

## 在线搜索、依赖与更新

Modrinth 无需密钥。CurseForge 在线请求需用 `ccl credentials curseforge set` 保存用户自有密钥，或临时设置 `CCL_CURSEFORGE_API_KEY`。搜索可按项目类型、Minecraft 版本、加载器筛选。必需依赖及固定版本会自动解析并检查兼容性；可选依赖会列出，分析会报告不兼容选择。

安装其他项目会保留无关模组。更新时若受管理文件被修改、删除，或新文件与未托管内容冲突，操作会停止。`updates` 检查可用更新；使用 `install` 应用选定版本。

`online` 的平台参数接受 `modrinth` 或 `curseforge`。项目和版本参数可以使用对应平台的项目 slug/ID 和版本 ID。以下示例覆盖七个子命令：

```sh
ccl online search modrinth sodium --minecraft 1.21.1 --loader fabric --kind mod --limit 10
ccl online versions modrinth sodium --minecraft 1.21.1 --loader fabric
ccl online resolve modrinth sodium 1.21.1 fabric
ccl online install my-instance modrinth sodium --version VERSION_ID
ccl online updates my-instance
ccl online analyze my-instance
ccl online pack-install my-pack modrinth example-pack --version VERSION_ID --include-optional
```

`install` 会解析项目及必需依赖，并安装到已有实例。`pack-install` 会下载平台整合包并创建或更新实例。`updates` 和 `analyze` 会检查已有实例的收据。调用 CurseForge API 的命令需要已保存密钥或 `CCL_CURSEFORGE_API_KEY`；Modrinth 不需要密钥。密钥请求头范围测试成功不代表真实 CurseForge 密钥已获得在线授权。

这些 API 使用 `DownloadConfig` 的代理和离线设置。CurseForge 搜索、版本查询和直接下载都需要用户提供密钥；Modrinth 不需要密钥。启用离线模式时，在线请求会失败。

<a id="networking"></a>

## 网络与离线模式

CCL 将 Minecraft 启动身份与网络连接状态分开处理。将 `downloads.offline` 设为 `true` 会禁用网络请求。若游戏目录中的常规路径已有通过校验的文件，仍可复用；缺少文件或文件无效时会返回明确的离线缓存错误。

在线安装会将经过验证的版本清单保存到 `<游戏目录>/.ccl/cache/version_manifest.json`。离线安装会读取该清单，并检查常规路径中的版本元数据、库文件、资源索引和资源对象。离线模式无法发现版本或下载缺失文件。将 `downloads.offline` 改回 `false` 后即可更新元数据或补齐文件。

网络离线模式下，`versions` 命令也会读取该游戏目录中经过验证的清单，因此无需连接版本服务即可列出此目录已知的版本。

安装和启动缓存位于所选游戏目录中。Java 运行时安装遵循相同规则：已有有效的托管运行时可以离线使用。如果缓存中已有运行时清单、组件元数据和所有必需文件，也可以离线安装缺失的运行时；否则 CCL 会指出缺少的缓存项。下载源偏好和 HTTP 代理设置只在联网时生效。

下载会按元数据提供的哈希和大小进行校验。单文件传输中断后，只有保存的强 ETag 与响应 ETag 一致，且服务器通过 `Content-Range` 确认请求偏移和总大小时，才会从部分文件继续下载。任一校验失败，CCL 都会丢弃部分文件并重新开始。文件只有在完整大小和哈希校验通过后才会发布。取消操作或重试耗尽时会保留原有目标文件。

所需文件缓存完整后：

```sh
ccl --network-offline versions --limit 10
ccl --network-offline launch --instance my-world --offline
```

<a id="maintenance"></a>

## 启动器更新与诊断

v0.2.1 包含 GUI 流程修正、macOS 应用包密封和签名清单更新器。早于签名清单协议的 CLI 程序仍需手动引导更新。早于独立签名清单协议的 CLI 程序无法跨越该协议变更自行更新。首次需从 [GitHub Releases 页面](https://github.com/WinstonMeursault/CialloCraftLauncher/releases)手动下载当前 CLI ZIP，并在替换旧程序前核对 `.sha256`；之后更新器即可使用签名清单。现有设置、账户元数据、凭据和实例仍保留在原用户数据位置；兼容性检查覆盖已知的旧格式，不代表涵盖所有历史数据集。

只有显式运行更新命令时，CCL 才会检查启动器自身更新。稳定渠道跟随 `main`，本指南的目标版本为 v0.2.1。dev 渠道跟随 `dev`，可包含尚未进入稳定版的较新 GUI 改动。稳定版使用 `v0.2.1` 形式的版本标签。dev 提交只有在三个支持的 CI 目标均完成验证和打包后才会发布；保留最近十个 dev 发行版。

```sh
ccl update check
ccl update check --channel stable
ccl update apply
ccl update status
ccl update rollback
ccl update recover
ccl update recover --installation /path/to/ccl
```

使用 `--channel` 选择渠道会保存该设置。`check` 报告是否有兼容的已发布程序包。`apply` 下载并校验当前目标平台的归档，然后启动助手异步替换程序。CLI 清单命名为 `manifest-<target>.json`，并附带对应的 `.json.sig` 文件。签名覆盖清单原始字节，绑定产品、版本、渠道、分支、提交、目标平台和程序包摘要。更新器使用程序内置公钥验证签名，然后检查程序包大小、SHA-256 和归档路径再暂存。CLI ZIP 本身没有操作系统签名。Windows 遇到临时程序共享占用时最多重试 30 秒。命令返回后可用 `update status` 查看状态；若助手仍持有更新锁，请稍后重试，忙响应不等于事务失败。助手或替换过程被中断时用 `recover` 恢复；若常规程序无法运行，可通过保留的助手执行恢复，并用 `--installation` 指定原程序路径。`rollback` 只有在当前配置、账户元数据、实例和收据可由保留的旧程序读取时才会使用旧程序。二进制回退不会恢复游戏目录、存档或内容。

GUI 维护面板有独立的“检查更新”“下载并安装”“回滚 GUI 更新”和“恢复中断的更新”控制。GUI 清单命名为 `gui-manifest-<target>.json`，并附带 `.json.sig` 文件，由同一个内置更新公钥验证。签名绑定产品/版本/渠道/提交/目标平台及更新程序包摘要；操作系统发布者签名是另一种身份验证，本项目目前不提供。GUI 安装文件没有 Authenticode 签名，macOS 程序包也未经公证。

自更新只支持按用户安装的 Windows NSIS 应用、可写的 macOS `.app` 和可写的 Linux AppImage。Linux DEB 安装使用包管理器更新。助手会保留经过检查的旧应用副本，安装已验证程序包、启动新版，并等待最多 60 秒以取得启动健康确认。如果应用提前退出或未确认启动，助手会恢复并重新启动旧应用。更新成功后且备份仍可用时，可使用 GUI 回滚控制。若事务中断，可用恢复控制；条件允许时恢复到已知的旧应用。事务日志和更新数据保存在已安装应用之外。应用回滚不触及游戏文件、存档、账户和凭据。

游戏目录、存档和配置应保存在应用程序包之外。所选数据/配置路径与应用重叠时，自更新会拒绝执行。手动回滚也会拒绝覆盖更新后改动的应用文件；请先保留这些改动。Windows 回滚除恢复应用文件外，还会恢复经过检查的当前用户卸载/产品注册表信息。

若中断导致应用入口缺失，可直接运行保留的助手。默认目录为 macOS 的 `~/Library/Application Support/CialloCraftLauncher/gui/update`、Windows 的 `%APPDATA%\CialloCraftLauncher\gui\update`，或 Linux 的 `${XDG_CONFIG_HOME:-$HOME/.config}/ciallocraftlauncher/gui/update`；显式设置 `CCL_GUI_DATA_DIR` 时使用其 `update` 子目录。将 `GUI_UPDATE_DIR` 设为该目录后，macOS 执行 `"$GUI_UPDATE_DIR/helper" --ccl-gui-update-helper "$GUI_UPDATE_DIR" recover`；Linux 将程序名换为 `helper.AppImage`。PowerShell 执行 `& "$GUI_UPDATE_DIR\helper.exe" --ccl-gui-update-helper "$GUI_UPDATE_DIR" recover`。恢复完成前保留事务日志、更新负载和备份。这些命令恢复记录中的安装，不用于选择另一份应用。

同一 GUI 安装一次只允许一个应用实例或更新助手操作。如果状态或更新操作提示安装正忙，请关闭其他实例或等待助手完成后再试。`--network-offline` 会阻止 CLI 和 GUI 更新检查与下载。通过包管理器或只读方式安装的程序应通过原安装方式更新。

`doctor` 检查 Java 候选、启动器/游戏目录访问情况以及是否编译了系统凭据库适配器。它不会登录，也不验证凭据库是否解锁。添加 `--network-probe` 可访问 Mojang 版本元数据端点；添加 `--write-probe` 会在 CCL 数据目录创建并删除临时空文件。这两种探测都需显式启用。

```sh
ccl doctor
ccl doctor --network-probe
ccl doctor --write-probe
ccl diagnose --run RUN-ID
ccl diagnose --instance INSTANCE-ID --run RUN-ID
ccl diagnose --export ./ccl-diagnostic.json
ccl diagnose --run RUN-ID --export ./ccl-diagnostic.json --include-log-preview
```

运行诊断根据保存的运行记录、有大小限制的游戏日志前缀以及时间戳与该运行接近的崩溃报告，识别少数常见故障模式，并提供证据与建议；未知故障不会被强行归类。结构化诊断导出只含白名单字段，并替换本地路径；不会包含账户库、凭据库、存档或服务器列表。可选原始日志预览最多包含 16 KiB 游戏输出，并进行尽力脱敏，但仍可能包含账户名、令牌或游戏/模组写入的其他内容。分享前请检查文件。CCL 不会上传诊断报告。

更新兼容性检查覆盖已定义的历史样本，不代表涵盖所有旧配置、账户数据库或实例。GUI 程序包使用签名更新清单，但应用文件本身没有 Authenticode 签名或 macOS 公证。托管运行器启动检查和程序包目标不能认证计划中的最低操作系统版本。当前证据未包含 Windows 实机启动 Minecraft。清单详情见 API 文档，发行要求见开发指南。

<a id="accounts"></a>

## 账户

GUI“账户”页可添加、选择和移除多个已保存的离线玩家名。名称为 1–16 个 ASCII 字母、数字或下划线。这些本地档案不使用系统凭据库。游戏库直接选择已保存账户，不再要求另行输入用户名。CLI 继续支持默认离线身份；`--account ACCOUNT-ID` 使用实际已保存账户 ID（不要将 `offline` 当作占位 ID），一次性名称使用 `--offline --username NAME`。

> **CLI 实验性在线集成：** Microsoft 设备登录、Xbox Live/XSTS 交换和 Minecraft 服务集成仍保留在 CLI 中。GUI 的 Microsoft 登录暂时不可用。正版账户完整验收仍已暂缓；自动化覆盖及 OAuth 设备流程检查不能证明每个账户都能启动游戏。若 CLI 登录或启动失败，请检查报告的认证阶段和机器错误码。

应用所有者请参阅[注册及验收要求](development.zh-CN.md#microsoft-application)。

### 配置并登录

实验性的 CLI 流程可将客户端 ID 保存到启动器用户配置中。GUI 的公开客户端 ID 仅在“设置 → 高级设置”配置；GUI 登录目前不可用：

```sh
ccl settings set auth.microsoft_client_id YOUR-APPLICATION-CLIENT-ID
ccl account check-storage
ccl account login
```

`account login` 会先测试凭据库；若无法安全保存凭据，会在请求设备代码和联系 Microsoft 之前停止。`account check-storage` 可随时执行相同的写入、读取和删除探测；它只使用临时测试值，不会修改已保存账户。

也可以仅为本次登录传入 ID：

```sh
ccl account login --client-id YOUR-APPLICATION-CLIENT-ID
```

启动器会显示 Microsoft 验证网址和一次性设备代码，并自动打开默认浏览器。请在该页面登录并输入代码。无图形界面等环境可用 `account login --no-browser` 自行打开页面。自动打开失败时仍会保留网址并继续等待授权。不要分享设备代码；任何先输入该代码的人都可能授权正在等待的登录。CLI 会等待授权完成。之后可以使用：

```sh
ccl account list
ccl account select ACCOUNT-ID
ccl launch VERSION
```

选定账户后，`launch` 默认使用该账户，并在需要时刷新凭据。传入 `--offline` 可选择离线身份，也可与 `--username NAME` 组合来指定离线身份名称。这两个选项分别都与 `--account ACCOUNT-ID` 冲突；后者可为单次启动指定已保存账户。`account logout [ACCOUNT-ID]` 会删除本地凭据和账户元数据，但不会撤销 Microsoft 授权。

认证失败会标明服务阶段和 HTTP 状态，并在可用时显示白名单机器错误码；不会打印原始服务错误文本、令牌或账户标识。服务拒绝与已确认的 Java 版无权益是不同结果，不能仅凭通用 401/403 确定原因。

一次记录在案的真实重试完成了 Microsoft OAuth、Xbox Live 和 XSTS，随后 Minecraft 登录返回 HTTP 403，且未返回用户档案或权益数据。该状态本身不能确定服务端原因，也不能证明账户是否拥有游戏。排查时应提供认证阶段和错误码。

### 凭据存储

访问令牌和刷新令牌仅通过操作系统凭据库保存。账户名称和 ID 分开存储在 `accounts.json` 中，该元数据文件不包含令牌。macOS 和 Windows 使用系统凭据库。Linux 必须安装并解锁可通过当前 D-Bus 会话访问的 Secret Service。没有持久 Secret Service 的无头系统不能使用 Microsoft 登录；程序不会将凭据退回保存为明文。只要为会话提供并解锁持久的 Secret Service，无需图形桌面。

如果安全存储不可用，CLI 登录或刷新会报错。请启动带有已解锁密钥环的桌面会话；在安全存储可用前，可使用 GUI 已保存的离线档案。不要通过把令牌复制到文件或 shell 命令中来绕过限制。

<a id="troubleshooting"></a>

## 启动器日志与诊断

GUI“日志”页分别展示游戏运行日志和 CCL 启动器日志。启动器日志记录启动构建身份、任务开始/终态、GUI 错误类别，以及可选的导航事件。不会记录任意请求内容、账户名、令牌、文件系统路径、JavaScript 异常原文或游戏输出。最多保留五个文件，每个不超过 5 MiB；每次查看最多显示 1,000 行和 256 KiB。仅在需要导航事件时启用调试日志。导出会将当前显示的本地日志复制到所选位置；CCL 不会上传日志。

诊断报告是独立功能。分享前请检查报告，尤其是启用可选原始游戏日志预览时。

## 故障排查

- **版本发现或下载失败：** 检查网络和代理设置，可运行 `ccl settings show` 查看。`--network-offline` 和 `downloads.offline` 会禁用 HTTP 请求；获取缺失文件前请关闭离线模式。`ccl doctor --network-probe` 会测试访问 Mojang 版本元数据端点。
- **游戏找不到 Java：** 运行 `ccl doctor`，允许安装托管运行时，或用 `--java PATH` 指定兼容的 Java 可执行文件/主目录。若没有合适的已安装运行时，`--no-java-download` 会禁止下载并导致启动失败。
- **游戏立即退出：** 查看 `ccl logs --lines 200`，或用 `ccl logs --run RUN-ID` 选择一次运行；托管实例需添加 `--instance ID`。运行 `ccl diagnose --run RUN-ID` 检查记录结果及附近的崩溃报告。运行记录保存游戏的实际退出码；游戏异常退出时 CLI 返回 `8`。
- **配置无效：** 检查 JSON 文件和设置名称。`ccl settings reset --all` 会以默认值替换无效配置；使用 `--config PATH` 指定非默认文件。
- **CLI Microsoft 登录或启动失败：** 查看认证阶段和白名单机器错误码。确认公开客户端 ID 及凭据库可用，再区分 Microsoft 授权、Xbox/XSTS 访问、Minecraft 用户档案访问和账户权益错误。仅凭通用 HTTP 401/403 不能证明账户没有游戏权益。
- **CurseForge 请求被拒：** 确认已保存密钥或 `CCL_CURSEFORGE_API_KEY` 正在生效，并且该密钥获准访问 API 和指定文件。允许搜索的密钥不一定能下载受限文件；请遵守服务方授权及作者分发规则。
- **加载器或整合包安装报告格式不支持/文件冲突：** 检查 Minecraft/加载器版本，用 `ccl pack inspect` 检查整合包元数据，并在使用 `--replace-modified` 前检查本地已修改的托管文件。

分享日志或诊断导出前请先检查内容。原始游戏日志及可选诊断日志预览可能包含账户名、令牌或模组写入的内容。
