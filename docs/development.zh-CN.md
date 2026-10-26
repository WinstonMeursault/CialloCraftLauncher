# 开发指南

[使用指南](user-guide.zh-CN.md) · [API 文档](api.zh-CN.md) · [English](development.en.md)

- [环境](#environment)
- [构建与运行](#build)
- [桌面 GUI 与应用包更新](#gui-g3)
- [架构与工作区](#architecture)
- [代码边界](#boundaries)
- [配置与数据目录](#data)
- [实例与事务](#transactions)
- [更新、兼容性与诊断](#maintenance)
- [Microsoft 应用设置与可选登录验收](#microsoft-application)
- [验证与预览包](#validation)
- [发行清单与命令](#release)
- [v0.2.1 发行说明](#release-notes)
- [当前功能范围](#scope)

<a id="environment"></a>

## 环境

安装 Rust 稳定版工具链及 Cargo；工作区使用 Rust 2024 edition。发行工作流当前使用 Rust 1.99.0，常规 CI 使用 Rust stable 通道。还需要安装目标平台的本地链接器和 SDK：

- Windows x86_64：MSVC C++ Build Tools 和 Windows SDK（`x86_64-pc-windows-msvc`）。
- macOS arm64：arm64 Mac 和 Xcode Command Line Tools（`aarch64-apple-darwin`）。
- Linux x86_64：C 编译器、链接器、`make`、`pkg-config` 和 glibc 开发头文件（例如 Ubuntu 的 `build-essential`；目标为 `x86_64-unknown-linux-gnu`）。

发行工作流在对应的 GitHub 托管运行器上构建每个平台的程序包。本地交叉编译也需要所选目标对应的可用链接器和 SDK。组装程序包及生成依赖声明需要 Python 3。Unix ZIP 打包需要 `zip`、`unzip` 和 `sha256sum` 或 `shasum`；tar.gz 打包需要 `tar` 和 SHA-256 工具。Linux 运行账户和服务商凭据功能还需要持久且已解锁的 Secret Service/D-Bus 会话；不会回退到明文保存。

<a id="build"></a>

## 构建与运行

在仓库根目录运行：

```sh
cargo build --workspace
cargo run -p ccl-cli -- --help
```

`Cargo.lock` 固定工作区依赖版本。发行检查和构建应使用 `--locked`，让 Cargo 在清单与已提交锁文件不一致时直接失败。`keyring` 使用系统原生凭据库后端；HTTP 使用 Rustls，因此构建启动器不需要 OpenSSL。

用户流程见[使用指南](user-guide.zh-CN.md)，命令、输出及核心服务见 [API 文档](api.zh-CN.md)。

<a id="gui-preview"></a>
<a id="gui-g3"></a>

## 桌面 GUI 与应用包更新

桌面应用是独立的 Tauri 产品 `ccl-gui`，与 CLI 共享核心代码，但具有独立产品身份和更新清单。CLI 发行 ZIP 也继续提供，桌面程序包格式如下：



| 目标 | 分发程序包 | 应用内更新负载 | 安装方式 |
| --- | --- | --- | --- |
| Windows x64 | 按用户安装的 NSIS `.exe` | NSIS `.exe` | 安装到用户可写位置；GUI 更新器不支持便携版和系统级安装 |
| macOS arm64 | `.dmg` | 包含应用 bundle 的 `.app.tar.gz` | 可写的 `.app`；程序包未经公证 |
| Linux x64 | `.AppImage` 和 `.deb` | AppImage | AppImage 文件位置需可写；DEB 通过系统包管理器更新 |

发行工作流目标为 Windows 2025、macOS 15 和 Ubuntu 24.04，并会在各主机启动桌面应用并确认前端就绪标记：Linux 使用 Xvfb 和软件渲染，Windows 使用 WebView2。这些运行器不能证明物理机游戏启动或兼容较旧系统。Windows 10（含 WebView2）、macOS 13 和 glibc 2.39 的 Linux 是预期最低版本，目前尚未认证。

`crates/ccl-gui` 是 Rust 适配器/库，默认 feature 集为空（`default = []`）；可选的 `desktop` feature 启用 Tauri 2。`apps/ccl-gui` 是 React/TypeScript/Vite 前端。工作区默认成员只有 `ccl-core` 和 `ccl-cli`，因此普通默认 `cargo build` 或 `cargo test` 不包含 GUI；需使用 `--workspace` 或显式指定 `ccl-gui`。常规 CLI 和核心构建不依赖 Node.js 或 WebView。前端使用 Node 24 和 pnpm 11.25.0。

`gui-validation` feature 会启用仅供测试的 GUI 更新驱动和钩子。前端维护页性能测试入口另由 `VITE_CCL_GUI_PERF_HARNESS=true` 控制；隔离性能验收场景必须同时启用这两个开关。正常发行包不要启用任一开关。

macOS 程序包明确设置 `bundle.macOS.signingIdentity: "-"`，由 Tauri 密封完整应用包，而非依赖可执行文件的链接器签名。`scripts/package-gui-release.py` 在 macOS 上调用 `scripts/verify-macos-bundle.py`：构建应用包、更新归档解压后的应用包及只读 DMG 内的应用包都必须通过 `codesign --verify --deep --strict`。签名失败会中止 GUI 验证、dev 和 stable 工作流的打包。这些检查验证应用包完整性，不代表 Developer ID 信任或公证；浏览器下载后的 Gatekeeper 批准仍需单独处理。v0.2.1 的该修正源自 `dev` 的 `c366ce4` 提交。原始 v0.2.0 macOS DMG 和更新归档仍保留不完整的链接器签名；本地恢复方法见使用指南。

应用需要先确认游戏根目录才能打开游戏库。初始界面可通过系统目录选择器选择目录，或确认建议的目录；只有确认后才会创建缺失目录。已有根目录和托管实例会复用。可创建原版和托管实例，包括共享原版安装，以及使用受支持加载器的托管实例。系统文件选择器可先检查受支持的 Modrinth、CurseForge 和 MultiMC/Prism 归档，再导入。启动时选择已保存的离线账户，并可设置 Java 路径、内存、托管 Java 安装及 dry-run。GUI Microsoft 登录暂时不可用。

GUI 已保存离线档案仅使用账户元数据，不访问系统凭据库。GUI 中的 Microsoft 登录暂时不可用；实验性的 CLI 设备码登录协议仍保留。“设置”的高级折叠区保留公开 Microsoft 客户端 ID。GUI 原生任务注册表保留一个活动任务及最多八个已完成快照；界面通过可收起进度弹窗展示任务，右下角入口可重新打开；游戏 PID 出现后弹窗会收起。后端最多保存 1,000 行任务日志，每行 512 个字符；每份 WebView 快照最多显示最近 100 行。进度/日志通知合并为每秒最多 10 次，生命周期更新立即发送。安装/导入任务可协作式取消；停止游戏需单独确认。活动任务期间关闭程序前会询问；再次启动会唤起已有窗口。游戏日志与启动器诊断分开显示。

GUI 外观偏好（`theme`、`language` 和所选根目录）保存在独立的 `gui.json` 中。网络离线开关不同：它会更新并持久化共享启动器配置中的核心设置 `downloads.offline`，因此 CLI 和 GUI 使用相同值。离线玩家身份只是启动选项，不会禁用网络。`CCL_GUI_DATA_DIR` 指定 GUI 偏好数据目录，`CCL_GUI_CONFIG_PATH` 指定共享核心配置，`CCL_GUI_GAME_DIR` 固定游戏根目录并禁用根目录修改。测试时请使用互相隔离的临时路径。读取已有游戏库时可能获取实例锁，或恢复中断的实例事务。

设置面板分别编辑共享下载和启动值。Microsoft 客户端 ID 仍保存在共享配置中，但只在“设置”的高级折叠区显示；它是公开应用 ID。GUI 中的 Microsoft 登录暂时不可用；实验性的 CLI 流程仍保留。适配器把已读取的设置文档作为 `expected` 传入；核心配置锁会在写入时比较它并拒绝陈旧修改，因此 CLI 外部修改后需先重新加载。可按字段或全部重置，也可用于修复损坏配置。配置选择器接受经过验证的本地 JSON 配置；`CCL_GUI_CONFIG_PATH` 固定配置并禁用选择。切换配置会清除原生路径授权和已选的拖入归档。

高级页面使用类型化的 `InstanceRequest`、`OnlineQuery` 和 `OnlineMutation` 适配器。来源安装会创建带上下文的核心 `OnlineInstallRequest`，使用所选游戏根目录和实例 ID，在通过目标身份及加载器兼容性校验后，经实例事务执行下载。文件操作的选择器授权绑定到相应用途的路径。拖入一个 `.zip` 或 `.mrpack` 会检查归档并打开可编辑导入对话框；必须明确执行导入，已有 ID 会被拒绝，不会覆盖。同步文件系统变更开始后不可取消；带任务上下文的异步操作协作响应取消并完成核心清理。

维护面板只通过与配置绑定的系统凭据库存储 CurseForge 密钥；`CCL_CURSEFORGE_API_KEY` 是明确的环境变量覆盖，UI/状态响应不会泄露密钥。诊断原始日志预览默认关闭，受大小限制且仅尽力脱敏；导出保存在本地，分享前需人工复核。维护面板分别提供已安装 GUI 应用包和所选独立 CLI 的更新控制，两者均使用 stable（`main`）或 dev（`dev`）渠道。

启动器日志与游戏/任务输出分开。仅记录白名单启动版本/平台/提交、任务开始/终态事件、`GuiError` 错误码和可选的固定类别导航事件。不会持久化任意请求/响应内容、凭据、路径或 JavaScript 异常原文。最多轮转五个文件，每个 5 MiB。“日志”页最多读取 1,000 行/256 KiB，并可在本地导出当前显示内容；不会上传日志。`frontend_diagnostic` 仅接受固定事件类别。调试导航日志默认关闭。日志目录为 `<GUI 数据目录>/logs`。GUI 偏好文件和启动器日志与共享核心配置及账户元数据分开；本次 UI 工作未增加配置/账户 schema 字段。

### 构建并启动桌面应用

在仓库根目录执行；需先安装 Node 24、pnpm 11.25.0、Rust 及目标平台的本地依赖：

```sh
cd apps/ccl-gui
pnpm install --frozen-lockfile
pnpm test
pnpm build
cd ../..
target="$(rustc -vV | sed -n 's/^host: //p')"
cargo fetch --locked --target "$target"
python3 scripts/gui-notices.py --target "$target"
cargo test -p ccl-gui --locked
cargo run -p ccl-gui --features desktop --release --locked
# 打包前先退出开发窗口。
cd crates/ccl-gui
pnpm --package=@tauri-apps/cli@2.12.4 dlx tauri build --features desktop
```

上述示例使用 Bash（macOS/Linux 或 Git Bash）。PowerShell 中可用 `$target = ((rustc -vV | Select-String '^host: ').Line -replace '^host: ', '')` 设置目标，再将 `$target` 传给相同的 fetch/通知生成命令。前端命令在 `apps/ccl-gui` 执行，原生 Cargo 命令从仓库根目录执行，`tauri build` 在 `crates/ccl-gui` 执行。桌面构建前需生成通知：Tauri 资源包含 `target/gui-notices`，涵盖 Rust、生产 JavaScript、项目许可证及双语公开文档。`pnpm` 仅用于构建打包，不是运行时依赖。


Node 测试检查任务快照排序；`.github/workflows/gui.yml` 会在构建前端前运行该测试。工作流还运行格式检查、适配器测试、桌面版 Clippy、打包应用，并在 Windows x64、macOS arm64 和 Linux x86_64 上启动 UI、确认前端就绪标记。发行工作流还会生成分发程序包和更新负载。独立发行验收门会检查成功更新后手动回退、坏程序包触发自动回退，以及替换中断后的恢复，并确认游戏数据标记文件仍完好。验收证据以对应精确提交的工作流结果为准。Linux 需要 WebKitGTK 4.1 开发库、Xvfb 和 FUSE，用于打包/启动检查。Tauri 使用系统 WebView：Windows 为 WebView2，macOS 为 WKWebView，Linux 为 WebKitGTK 4.1；参见 [Tauri v2 前置要求](https://v2.tauri.app/start/prerequisites/)。

两个更新器使用 Tauri CLI 2.12.4 生成独立 `.json.sig` 签名资产。CLI 清单为 `manifest-<target>.json`，`product: "ccl-cli"`；GUI 清单为 `gui-manifest-<target>.json`，`product: "ccl-gui"`。每个签名覆盖完整 JSON 原始字节，并带有受签名保护的 Tauri 版本 trusted comment。清单绑定仓库、产品、渠道、版本、分支、完整 commit、目标平台、数据 schema 范围及程序包 SHA-256/大小/URL。验证公钥嵌入 `ccl-core`；`.sig` 签名的不是操作系统安装程序本身。GUI 清单中的 `os_signed: false` 表示没有操作系统发布者签名。

发行构建设置 `CCL_BUILD_CHANNEL`、`CCL_BUILD_COMMIT` 和 `CCL_BUILD_TARGET`，使 GUI 报告其 stable/dev 渠道、完整提交及目标平台。普通本机构建使用 unpublished 提交标识；不得将其当作可发布更新。

两个工作流使用同一组 GitHub Actions 密钥 `CCL_UPDATE_SIGNING_PRIVATE_KEY` 与 `CCL_UPDATE_SIGNING_PRIVATE_KEY_PASSWORD`，为 CLI 和 GUI 清单签名。应将其视为长期更新信任密钥：不得将内容写入源码或日志，并由发行所有者保管受保护的恢复副本，不要随意轮换或删除。已安装更新器信任程序内置公钥；丢失匹配的私钥会导致这些安装无法验证后续发行。不要在文档或发行说明中填写密钥内容。

<a id="architecture"></a>

## 架构与工作区

### 目标

CialloCraftLauncher 最初以命令行为首个界面。可复用的 Rust 核心提供配置、目录、网络、下载和安装逻辑；命令行和桌面 GUI 调用相同的核心服务。核心不依赖任何一种界面，因此桌面功能可以继续扩展，而无需把业务逻辑移入界面层。

首批目标平台：

- Windows x86_64
- macOS arm64
- Linux x86_64

### 工作区

- `crates/ccl-core`：共享服务、配置、错误、HTTP 传输、任务事件、安装、Java 探测和启动编排。
- `crates/ccl-cli`：命令行参数、文本或 JSON 输出、退出码，以及对核心的调用。
- `crates/ccl-gui` 和 `apps/ccl-gui`：可选桌面适配器及 React/TypeScript/Vite 前端；范围和检查见[桌面 GUI 与应用包更新](#gui-g3)。
- `docs`：面向用户和贡献者的公开文档。

核心通过任务上下文报告进度，并提供协作式取消句柄。界面展示事件并收集输入；核心执行工作、验证配置并返回结构化错误。核心不得依赖界面。

<a id="boundaries"></a>

## 代码边界

`loaders`、`instance` 及 `launch --instance` 在 CLI 和桌面 GUI 中复用相同核心服务。`modpack_format` 负责 Modrinth 归档解析、校验和写入；`import_formats` 检测 CurseForge 和 MultiMC/Prism 归档，并通过事务化整合包流程转换受支持的输入；`modpacks` 协调整合包导入/导出并管理所有权收据；`content` 管理本地文件和用户数据备份。在线服务集成位于 `online_content`。整合包导入和内容编辑使用与实例操作相同的暂存事务和跨进程锁。现代 Forge/NeoForge 处理器由核心启动，通过任务事件报告输出，不得直接写入 CLI 标准输出。NeoForge SHA-512 回退会先校验安装器字节，再由安装流程计算 SHA-1。

- 可复用的领域逻辑放在 `crates/ccl-core`。
- `crates/ccl-core` 不得依赖终端或图形界面库。
- CLI 负责解析参数、格式化输出和映射退出码，不复制核心业务规则。
- 任务通过核心事件报告进度，并通过取消句柄响应取消。
- 配置错误应包含易理解的上下文；不得将秘密写入日志或输出。设置更新使用核心的 `ConfigStore::update` 事务，通过跨进程锁串行化，并在验证后原子保存。账户令牌必须保存在操作系统凭据库中；Linux 需要可用且持久的 Secret Service/D-Bus 会话，不得退回保存明文。

<a id="data"></a>

## 配置与数据目录

配置字段及默认路径统一维护于[使用指南](user-guide.zh-CN.md#configuration)。

`auth.microsoft_client_id` 是公开应用 ID，不是秘密。账户元数据（如账户 ID、名称、档案 ID 和当前选中账户）与 OAuth/Minecraft 令牌分开保存；令牌必须留在按账户索引文件路径隔离的操作系统凭据库中。CurseForge API 密钥保存在另一个按配置文件路径隔离的系统凭据项中；可选的 `CCL_CURSEFORGE_API_KEY` 环境变量会覆盖已保存密钥。两类凭据都不能放入 JSON 配置、日志、版本控制或命令行参数。

默认游戏目录优先级为：显式 `--game-dir`、当前工作目录中已存在的 `.minecraft`、平台惯用目录、用户主目录中的 `.minecraft`，最后在当前工作目录创建新的 `.minecraft`。macOS 已存在的惯用目录可以复用。`doctor` 只读；`init` 会创建所选目录。

版本列表和文件来源偏好相互独立，接受 `auto`、`official` 或 `mirror`。镜像根地址可配置，且必须为不含 URL 凭据的 HTTPS 地址。全局选项 `--network-offline` 禁用 HTTP，并且只使用已验证的本地元数据和文件；它与离线玩家身份不同。只有保存的强 ETag 和服务器范围响应验证未完成文件后才会续传。版本列表、安装、启动准备和进程输出捕获在共享核心中实现；CLI 只负责界面格式和退出码映射。

版本列表和游戏文件分别设置来源，各自支持 `auto`、`official` 和 `mirror`。二者默认均为 `auto`；镜像根地址可在用户配置中设置。当前版本列表和安装流程使用官方版本清单及版本元数据。安装会下载客户端、库、原生库、资源索引和资源文件。依据上游可用的哈希和大小校验文件，限制并发下载数，重试临时失败，仅在 ETag 与范围响应检查通过时续传，复用有效缓存文件，并先写入临时文件再替换目标。跨进程版本锁可避免多个安装同时写入同一版本。任务支持协作式取消。已有版本 JSON 与请求元数据冲突时会报告冲突而不会替换。NeoForge 安装器在 SHA-1 旁文件不可用时可使用官方 SHA-512 校验和；只有完成字节校验后才会为安装流程计算 SHA-1。

<a id="transactions"></a>

## 实例与事务

`loaders` 服务发现兼容的 Fabric、Quilt、Legacy Fabric、Forge、NeoForge 和 OptiFine 版本，并安装受支持的配置。`forge` 引擎解释现代安装器的客户端处理器，通过任务事件报告输出，并检查声明的输出哈希。Forge 支持处理器安装器及受支持的旧版元数据格式；NeoForge 使用处理器安装器。继承配置保留原版客户端归属目录；子配置的库版本覆盖匹配的父配置产物，同时保留不同的原生库分类。上游兼容元数据只是安装前检查，并不证明所有第三方模组组合都能启动。参阅[加载器与实例](user-guide.zh-CN.md#loaders)。

`instances` 服务管理 `<游戏根目录>/.ccl/instances/<id>/game`。安装和更新在独立暂存修订版中构建，成功后才提交。事务日志支持修订版切换中断后的恢复，并保留前一成功修订版以便回滚。加载器更新和移除会保留当前用户数据。整合包导入/更新和内容编辑也使用相同的暂存版本事务与实例跨进程锁；游戏正在持锁运行时，修改不能替换其目录。

实例生命周期操作可重命名、复制、删除实例，查看和恢复保留的修订版，报告逻辑磁盘用量，并预览或执行归档修订版及未完成暂存目录的清理。清理不会选择活动世界或内容；硬链接意味着逻辑字节数不等于实际磁盘占用，也不能保证释放同等空间。参阅[实例生命周期](user-guide.zh-CN.md#instances)。

归档导入可识别 Modrinth、CurseForge 和 MultiMC/Prism 导出包。CurseForge 远程文件解析使用官方 API，并要求用户自行提供 API 密钥；请求范围的测试夹具不代表已获得真实 API 授权。MultiMC/Prism 导入支持受支持的原版/加载器组件和本地游戏内容，并会拒绝不支持的自定义补丁。这些转换不会执行归档钩子，也不会导入账户数据库。参阅[归档导入](user-guide.zh-CN.md#imports)。

在线内容服务支持 Modrinth 和 CurseForge 的搜索、版本查询、依赖解析、更新检查、冲突分析及事务化安装/更新。CurseForge 密钥可用或请求头范围测试均不代表真实密钥已获得生产授权。参阅[在线内容](user-guide.zh-CN.md#online)。

CLI 通过 `ccl credentials curseforge set` 接收 CurseForge 密钥：程序会在隐藏输入的终端提示中读取密钥，并将其保存在与所选配置路径关联的操作系统凭据库中。`CCL_CURSEFORGE_API_KEY` 是环境变量覆盖项。CCL 不会把密钥写入 JSON 设置或账户元数据，且只向获准的 CurseForge API/CDN 主机发送密钥。凭据库可用或本地保存成功都不能证明 CurseForge 已授权真实密钥用于生产访问；发行前应由密钥持有人或服务商确认。参阅[凭据处理](user-guide.zh-CN.md#configuration)和 [API 行为](api.zh-CN.md)。

`modpack_format` 模块读取和写入 Modrinth 格式版本 1 归档，校验清单和目标路径，限制 ZIP 解压规模，并按顺序应用 `overrides/` 与客户端覆盖。`modpacks` 会事务性导入所选文件和覆盖内容，并在收据中记录整合包拥有路径的哈希。更新会把当前文件与归属哈希比较；整合包拥有的文件被修改或删除，或新路径与未托管本地内容冲突时，会停止更新，除非调用者明确授权替换。整合包或内容变更后，`instance rollback` 会恢复上一份完整实例快照，包括用户数据和存档；如果要保留较新的存档，应先备份。`content` 管理本地内容和可移植用户数据备份。备份包含用户数据与存档，但不包含游戏/加载器文件、Java 运行时或账户凭据。Modrinth 导出与备份用途不同：导出不包含存档和私有服务器列表，本地二进制文件需要明确授权重新分发。命令和安全说明见[Modrinth 整合包与实例内容](user-guide.zh-CN.md#packs)。命令及支持边界另见[加载器与实例](user-guide.zh-CN.md#loaders)。

<a id="maintenance"></a>

## 更新、兼容性与诊断

CLI `update::UpdateService` 和 GUI 应用包更新器只查询 `WinstonMeursault/CialloCraftLauncher`。稳定版使用 `main` 上的 `v<VERSION>` 标签；每次向 `dev` 推送都会启动 Windows x64、macOS arm64 和 Linux x86_64 的验证与打包；所有任务通过后，才会将对应提交发布为不可变的 `dev-<完整 SHA>` 预发行版。工作流保留最近十个开发版发行。两个产品都要求包含 `product`、仓库、渠道、版本、分支、完整 commit、目标平台、负载摘要和数据读取范围（`1..=1`）的签名清单。

`UpdateService::check` 会区分可用程序包与尚未完成发行的分支提交。`stage` 下载并校验有大小限制的 ZIP、归档路径及哈希，然后暂存候选程序。`Installation` 管理渠道选择和持久事务日志；`launch_helper` 启动受限旧程序助手，由 `execute_helper` 异步执行替换、健康检查或回滚；Windows 对临时共享占用有界重试。`prepare_rollback` 会先检查当前数据兼容性。二进制更新器不会恢复游戏数据。当前格式无需迁移；读取器保留无 schema 配置/账户/实例元数据的读取能力，并接受 schema 1 收据。未知的未来配置或实例字段以及大于 1 的收据 schema 会被拒绝。首轮回归使用从基线 `2edbf0e` 的 Git 归档生成的配置与账户元数据；夹具已脱敏，测试账户凭据没有写入文件。

每份清单都配有独立 `.json.sig` 资产。Tauri CLI 2.12.4 对清单原始字节签名，并带有受签名保护的版本 trusted comment。CLI 和 GUI 共用保存在 GitHub Actions Secrets 中的长期私钥；匹配公钥嵌入 `ccl-core`。若丢失该私钥，或未计划客户端迁移就轮换密钥，已安装程序内置的信任根将无法验证后续签名。不要将密钥内容放入源码、日志或文档，应由发行所有者保管受保护的恢复副本。

清单签名认证清单及其程序包摘要；单独的 SHA-256 只能发现字节变化，不能认证程序包发布者。这不等同于 Windows Authenticode 或 macOS Developer ID 签名/公证。当前 GUI 安装程序没有操作系统发布者签名，macOS 程序包也未经公证。CLI ZIP 同样没有操作系统签名。`update status`、`rollback` 和 `recover` 不需要网络；更新检查和应用需要网络，且会被 `--network-offline` 阻止。首次使用新 `product` 字段和独立签名清单的 CLI 发行，必须先手动覆盖安装旧版 CLI，因为其更新器不能解析新格式。

GUI 更新器与 CLI 二进制回退是不同事务。GUI 更新器只支持 Windows 按用户安装且可写的 NSIS 安装、可写的 macOS `.app` 和可写的 Linux AppImage。Linux DEB 由包管理器控制。助手保留经过检查的旧应用树，等待最多 60 秒以取得更新后 GUI 的启动健康确认；确认失败时恢复旧树。GUI 事务中断后的恢复也会尽可能回到上一版。该事务不包含游戏数据。About 响应报告 `os_signed: false` 和 `update_manifest_signatures_required: true`；核心版本号根据实际链接的核心构建动态报告。

`compatibility::inspect_compatibility(config_path, game_root)` 是只读检查，不访问操作系统凭据库，只检查 CCL 管理的元数据。未来迁移必须锁定并备份受影响数据、暂存并原子提交、支持安全的中断恢复；若保留的旧读取器无法读取新数据，则拒绝回退。不得通过恢复整个游戏目录来回退启动器。

`DiagnosticsService::doctor` 报告 Java 候选、目录访问和是否编译凭据库适配器；不探测凭据库解锁状态。网络与写入探测需显式启用。`diagnose_run` 检查有大小限制的运行日志和时间戳接近的崩溃报告，仅识别已支持模式。导出只序列化白名单字段并替换路径；原始输出预览需单独显式选择，最多 16 KiB，并尽力脱敏。它仍可能包含游戏或模组写入的秘密，CCL 不会上传该报告。

较早的 CLI 维护验收（历史上称为 P7）覆盖三平台发行验证和事务更新样本，包括 Windows 程序共享锁、健康检查失败和入口丢失恢复。首个使用该更新协议的 CLI 版本需要覆盖安装，因为更早的二进制没有更新协议。当前数据格式无需迁移；旧写入器样本不代表所有历史收据版本。常规性能工作流在三个平台上使用固定本地 HTTP 安装夹具，测量冷/热安装、缓存复用、CLI 启动和 ZIP 解析，以及真实 CLI 更新助手的提交/回滚；记录请求数、传输字节数、逻辑磁盘用量及平台可用的峰值内存。历史回归门槛在有三个兼容的先前观测后启用，并比较最多五个滚动样本；即使门槛失败也会上传报告。它不验证公网吞吐、Minecraft 主菜单就绪或实际分配磁盘块。

要直接调查性能，可手动触发 `.github/workflows/performance.yml` 并设置 `compare_ref` 为基线分支、标签或完整 40 位提交 SHA。配对比较在同一个 Ubuntu 24.04 运行器上以 Rust 1.99.0 构建两个版本，使用相同 Linux 目标平台。工作流将候选版的 `p7_benchmark.rs` 和 `update_process.rs` 复制到基线工作区，让两个版本使用相同测量程序；报告会记录两个提交、运行器信息和测量程序哈希，并为两个版本分别构建 CLI、基准示例和更新测试。基准以交替顺序执行五组基线/候选配对。每组每个构建记录三次冷/热安装及更新器测量（各共 15 次），另记录七次 CLI/解析测量（共 35 次）。比较冷/热安装、元数据解析、更新提交和健康检查失败回滚的配对中位数；中位退化超过 25% 会导致门槛失败。报告保留 90 天。该比较仅覆盖 Linux，并补充定时/推送触发的历史性能工作流。

每周或手动触发的上游元数据回归覆盖原版正式版/快照目录和已验证 Minecraft 版本的加载器目录。

<a id="microsoft-application"></a>

## Microsoft 应用设置与可选的在线登录验收

> **当前范围：** GUI 中的 Microsoft 登录暂时不可用。现有 CLI 设备码登录协议仍属实验功能；真实账户验证和验收仍已暂缓。GUI 已保存离线档案和 CLI 离线启动继续受支持。一次记录在案的实时重试通过 Microsoft OAuth、Xbox Live 和 XSTS，随后 Minecraft 登录在返回用户档案/权益前响应 HTTP 403，原因尚未确认。不得暗示 GUI 当前可以启动 Microsoft 登录。

### 注册公开桌面应用

启动器使用 Microsoft 设备授权流程和 `/consumers` 权限机构。它是公开桌面客户端，不能使用客户端密钥。Microsoft 当前的[桌面应用配置指南](https://learn.microsoft.com/en-us/entra/identity-platform/scenario-desktop-app-configuration)说明公开客户端配置；[设备代码流程文档](https://learn.microsoft.com/en-us/entra/identity-platform/v2-oauth2-device-code)列出支持的权限机构。

1. 使用有权创建应用注册的账户登录 [Microsoft Entra 管理中心](https://entra.microsoft.com/)。
2. 打开 **标识 → 应用程序 → 应用注册 → 新注册**。
3. 为应用填写清楚的启动器名称。选择支持个人 Microsoft 账户的账户类型，以匹配启动器使用的 `/consumers` 权限机构。Microsoft 的[应用注册快速入门](https://learn.microsoft.com/en-us/entra/identity-platform/quickstart-register-app)介绍了可选账户类型。如果门户没有兼容选项，请先确认注册方案，再选择更宽泛的类型。
4. 注册应用并复制 **应用程序（客户端）ID**。这个 GUID 是公开配置，不是密码。
5. 打开 **身份验证**。在 **高级设置** 中将 **允许公共客户端流** 设为 **是** 并保存。设备代码客户端不使用客户端密钥或浏览器重定向 URI。
6. 不要创建客户端密钥，也不要把访问令牌或刷新令牌放入启动器设置。
7. 确认你的应用当前需要满足的 Xbox 和 Minecraft 服务访问要求。完成注册本身不能证明完整登录流程可用。如果服务拒绝该应用，请根据错误和该服务的官方支持流程解决访问问题。

桌面应用配置指南说明如何启用公开客户端流。设备代码客户端不需要重定向 URI。完成注册和较早的令牌交换步骤，并不能证明已获准面向公众使用 API。

### 未来真实账户登录验收的可选清单

如果所有者未来希望将 Microsoft 在线登录标记为已通过真实账户验收，请记录以下证据：

- 已注册的公开客户端 ID 与启动器配置一致，并已完成 Microsoft 要求的审核；
- 设备代码登录可用于拥有 Minecraft Java 版权益的账户；
- 拒绝、过期、取消、刷新、退出和凭据库不可用等路径均符合预期；
- CLI 输出、日志、错误文本、普通配置和账户元数据中均没有令牌；
- Linux 版本在真实且持久的 Secret Service/D-Bus 会话中可用；
- 已核实当前 Microsoft、Xbox 和 Minecraft 对真实账户登录及分发的服务/API 要求。

最近一次记录的验证没有可用的 Minecraft Java 版正版账户。此限制仅应附加在在线真实账户登录的验收声明中。所有者选择保留离线身份流程，并推迟这项可选验收工作。

<a id="validation"></a>

## 验证与预览包

执行与 CI 相同的检查：

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --all-targets --locked
cargo build --workspace --release --locked
```

CI 覆盖 Windows x64、macOS arm64、Linux x86_64。Windows 另执行需主动运行的操作系统凭据库探测。真实平台及账户验证与服务夹具应分别记录。预览脚本打包程序、许可证、README 和全部双语公开指南；整理文档时同步更新链接、CLI 诊断路径及两份打包脚本。

CI 矩阵会运行格式检查、将警告视为错误的 Clippy、全部工作区测试，以及三个目标平台的优化发行构建。Windows 还会分别运行账户凭据和服务商密钥的凭据库往返探测。每周/手动上游元数据工作流是独立的只读检查（默认 Linux，也可选择三个平台）。性能工作流同样独立，会记录基准报告，不能代替发行验证。单独的预览工作流会构建和打包制品，但不会发布发行版本。

单元测试夹具覆盖平台搜索与版本响应解析、依赖类型和固定版本、CurseForge 文件元数据及依赖关系、密钥脱敏与请求头限定、更新版本选择、事务式所有权、回滚和冲突。回环 HTTP 夹具会执行 Modrinth 搜索、版本列表和固定依赖递归解析。另有一个可选测试直接查询公开 Modrinth API。

<a id="release"></a>

## 发行清单与命令

### 稳定版发行前

1. 准确描述身份验证和来源支持范围。GUI 已保存离线档案及 CLI 离线身份仍受支持。GUI Microsoft 登录暂时不可用，CLI 集成仍属实验功能，真实账户验收已暂缓。不要根据测试夹具或本地保存密钥推断 CurseForge 已授权线上访问。检查当前所有平台的性能制品；调查疑似退化时，使用选定基线执行 Linux 配对比较。
2. 只有在明确批准正式稳定版发行后，才从干净工作区获取远端引用并将已验收的 `dev` 提交集成至 `main`。保留本地 main 的独立改动，解决并复核合并冲突，不重置分支或强制推送。将 `crates/ccl-core/Cargo.toml`、`crates/ccl-cli/Cargo.toml` 和 `crates/ccl-gui/Cargo.toml` 的 `version` 字段设为相同的发行版本，并同步 `crates/ccl-gui/tauri.conf.json` 和 `apps/ccl-gui/package.json`。GUI 和 CLI 版本必须与稳定标签一致。按需更新 `Cargo.lock` 并检查差异：

   ```sh
   git fetch origin
   git switch main
   git pull --ff-only origin main
   git merge --squash origin/dev
   # 解决并复核冲突，然后按需修改全部三个 package 版本。
   cargo check --workspace
   python3 scripts/check-package-versions.py --expected X.Y.Z
   test "$(python3 -c 'import tomllib; print(tomllib.load(open("crates/ccl-gui/Cargo.toml", "rb"))["package"]["version"])')" = X.Y.Z
   test "$(python3 -c 'import json; print(json.load(open("crates/ccl-gui/tauri.conf.json"))["version"])')" = X.Y.Z
   test "$(node -p 'require("./apps/ccl-gui/package.json").version')" = X.Y.Z
   ```

检查 `git diff -- Cargo.lock`，只保留工作区版本变更所需的锁文件更新。发行准备时不要顺带刷新无关依赖版本。

3. 验证稳定发行前，更新两份 README、使用指南及 API/开发指南的版本可用性说明，使其描述预期发布的版本；正式发布前仍须明确标记尚未发布的草稿。运行本机验证并检查差异。这些是发行工作流在本机目标平台上的源码检查；托管工作流还会在所有发行目标上重复检查。

   ```sh
   cargo fmt --all -- --check
   cargo clippy --workspace --all-targets --locked -- -D warnings
   cargo test --workspace --all-targets --locked
   cargo build --workspace --release --locked
   git diff --check
   ```

4. 将版本变更和发行就绪的源码提交并推送到 `main`。若目标版本已经正确，合并后没有额外版本变更，则跳过下面仅用于版本变更的 add/commit 命令。只暂存审核过的文件，包括 package 版本清单和锁文件。确认目标提交可从 `origin/main` 到达，然后创建并推送完全匹配的标签：

   ```sh
   git add Cargo.lock crates/ccl-core/Cargo.toml crates/ccl-cli/Cargo.toml crates/ccl-gui/Cargo.toml crates/ccl-gui/tauri.conf.json apps/ccl-gui/package.json
   git commit -m "chore: prepare vX.Y.Z"
   git push origin main
   git fetch origin main
   git merge-base --is-ancestor HEAD origin/main
   # 等待 main 分支 CI 通过，再创建版本标签。
   git tag -a vX.Y.Z -m "CCL vX.Y.Z"
   git push origin vX.Y.Z
   ```

推送稳定标签不会自动启动或发布发行。发行所有者明确批准正式稳定版并推送唯一的 `vX.Y.Z` 标签后，在 Actions → Stable Release → Run workflow 中选择该标签，并输入准确确认词 `PUBLISH`。手动触发的工作流会确认标签位于 `main` 且 `ccl-core`、`ccl-cli`、`ccl-gui` 版本一致，然后验证源码，在 Windows x64、macOS arm64 和 Linux x86_64 上构建 CLI ZIP 与 GUI 安装包，并尝试在每个托管运行器启动 GUI。所有任务通过后才会创建草稿、签署两类清单、重新下载并验证发行资产，最后发布。稳定标签必须唯一且不能移动到其他提交。dev 预发行版不代表未来稳定版获批；未经用户明确批准，不得将未发布改动合并到 `main`、创建新的稳定标签或发布。

如有需要，可在打标签前使用对应脚本打包一个本机目标版本。三个平台的 CLI 更新程序包都是 ZIP；GUI 格式见[桌面 GUI 与应用包更新](#gui-g3)：

```sh
# Linux 本机；Apple 芯片 Mac 改用 aarch64-apple-darwin。
TARGET=x86_64-unknown-linux-gnu
cargo build --release --locked --target "$TARGET" -p ccl-cli
TARGET="$TARGET" ARCHIVE_EXT=zip bash scripts/package-preview.sh
```

```powershell
$target = 'x86_64-pc-windows-msvc'
cargo build --release --locked --target $target -p ccl-cli
$env:TARGET = $target
./scripts/package-preview.ps1
```

普通本机构建在未提供发行构建环境时使用 unpublished 提交标识；可用于本地检查，不能代替工作流产生的更新程序包。CLI 打包脚本要求对应的 `target/<三元组>/release/ccl` 程序存在（Windows 文件名为 `ccl.exe`）。归档根目录包含项目许可证、两份 README、配对的中英文指南、生成的 `THIRD-PARTY.en.md` / `THIRD-PARTY.zh-CN.md` 清单，以及包含原始许可证和声明文本的 `THIRD-PARTY-LICENSES.en.md` / `THIRD-PARTY-LICENSES.zh-CN.md` 双语文本包。`scripts/third-party-notices.py` 会读取 `ccl-cli` 在所选目标下的常规及构建依赖图；每份清单都链接到对应语言文本包内的确定性锚点。两份文本包会翻译介绍标签，同时保留完全一致的原始许可证文本字节；第三方原文统一收录在这两个文件中。打包脚本会验证四个声明文件均已进入归档，且 ZIP 不超过 256 个条目，以确保当前读取器可以安装。GUI 程序包还会嵌入由 Rust 和生产前端依赖生成的第三方许可证/声明材料。CLI ZIP 和 GUI 安装程序均无操作系统发布者签名；独立的签名清单可供更新器验证其元数据和程序包摘要。

### 开发版

当前工作区 package 版本为 0.2.1。本文档对应 v0.2.1 GUI 和 CLI 发行版；已发布程序包的可用状态以 GitHub Releases 为准。开发预发行版可能包含较新改动。

向 `dev` 推送已验证的提交即可触发 `.github/workflows/release-dev.yml`：

```sh
git switch dev
git pull --ff-only origin dev
# 完成并提交预期的改动
git push origin dev
```

工作流要求三个平台验证、打包和尝试 GUI 启动检查期间，被测提交仍是 `dev` 当前分支头。成功后，它会创建不可变标签 `dev-<完整提交 SHA>`，校验 CLI ZIP、GUI 程序包、校验和与签名清单，发布预发行版，并保留最新十个符合条件的开发版发行。不要手动创建或推送该标签。如果验证时 `dev` 前进了，旧提交不会发布；应改为验证新分支头。dev 预发行版不代表正式稳定版获批；未经用户明确批准，不得合并到 `main`、创建稳定标签或发布正式稳定版。

### 发行门槛故障排查

- **稳定版标签被拒绝：** 确认标签提交是 `origin/main` 的祖先、core/CLI/GUI Cargo 版本及 GUI 配置元数据与去掉 `v` 后的标签相同，并已提交 `Cargo.lock`。使用 `python3 scripts/check-package-versions.py --expected X.Y.Z` 和发行清单中的 GUI 元数据检查。
- **`--locked` 报告锁文件需要变更：** 更新三个 Rust package 版本并同步 Tauri 配置和前端 package 版本，运行 `cargo check --workspace`，检查并只提交所需的 `Cargo.lock` 变更，然后重跑 locked 检查。
- **本地链接或构建失败：** 安装[环境](#environment)中列出的目标编译器/链接器和 SDK。本机目标构建通过不能证明其他目标可以链接。
- **凭据库探测失败：** 解锁操作系统凭据服务。Linux 需要持久的 Secret Service/D-Bus 会话；CCL 会返回错误，不会将凭据明文写入文件。
- **开发版发行未生成：** 查看三个平台的验证任务，并确认 `dev` 是否已超过被测提交。任务失败或分支头过期不会生成不可变程序包。
- **哈希校验通过：** 这只能确认字节与摘要一致，不能认证发布者身份。更新器客户端还会验证独立清单签名；它与操作系统发布者签名不同。

<a id="release-notes"></a>

## v0.2.1 发行说明

以下说明对应 v0.2.1，供获批准的稳定版发行工作流使用。发布状态以 GitHub Releases 为准。原始 v0.2.0 资产保持不变。

<!-- release-notes:start -->
# CialloCraftLauncher 0.2.1

本次补丁改进桌面使用流程，并修正 macOS 应用包完整性。

- 实例支持 UTF-8 名称，包括中日文和空格。游戏库发现和启动使用一致的文件名校验，修复已有命名配置触发 `invalid_launch_key` 的问题；仍遵守 Windows 文件名限制。
- 在账户页添加、选择和删除已保存离线账户，游戏库直接选择这些账户。离线档案不使用凭据库。GUI Microsoft 登录暂不可用，公开 Client ID 移至设置 → 高级设置；CLI 实验性 Microsoft 流程仍保留。
- 下载、安装、导入等操作改用可收起的进度弹窗，移除任务中心页面。进度同步可恢复遗漏的完成事件，游戏进程启动后自动收起弹窗。
- 下载中心、实例工具和设置改为可读结果，移除原始 JSON、调试状态、多余实例文案及侧栏标语。
- CCL 启动器日志与游戏日志分离，支持有界轮转、可选导航日志和本地导出。持久日志仅记录允许的事件，不记录任意请求数据、凭据或用户路径。
- macOS 打包采用完整应用包 ad-hoc 签名，并验证构建应用、DMG 和更新归档，修正原始 v0.2.0 资产签名不完整的问题。仍无 Apple Developer ID 签名和公证，macOS 可能需要逐应用批准。
- 中英文指南已与以上行为对齐。

程序包覆盖 Windows x86_64（CLI ZIP / GUI NSIS）、macOS arm64（CLI ZIP / GUI DMG 及更新归档）和 Linux x86_64（CLI ZIP / GUI AppImage 及 DEB）。CLI 和 GUI 更新清单分别使用独立签名认证各自负载；程序包没有操作系统发布者证书签名。Linux DEB 由包管理器更新。

现有启动器配置、账户和实例元数据无需 schema 迁移即可读取。替换内容前请备份重要存档。Minecraft 真实正版账户验收仍已暂缓，CurseForge 需要用户提供获授权的 API key。托管 CI 和受控启动夹具不代表所有平台的物理机 Minecraft 验收。
<!-- release-notes:end -->

`scripts/release-notes.py` 会提取标记间的中英文正文，确认标题版本号与指定程序包版本一致，再合并为 `.github/workflows/release-stable.yml` 使用的 GitHub 发行说明。请保持两段标记内容准确，并让标题版本号与 Cargo package 版本一致。以上内容对应 v0.2.1；发布仍须经过获批准的稳定版工作流。未来发行前要检查当前三个平台的性能制品；调查疑似退化时，可选择基线运行 Linux 配对比较。历史验收结果不能证明当前发行提交的性能。

<a id="scope"></a>

## 当前功能范围

核心提供配置、目录选择、HTTP 下载、版本查询与安装、安装托管 Java 运行时到 `.ccl/runtimes`、实验性的 Microsoft 账户身份验证协议（目前未由 GUI 登录入口提供）、离线身份启动、启动计划、原生库解压、子进程输出捕获、Modrinth 整合包处理、CurseForge 和 MultiMC/Prism 归档转换、Modrinth/CurseForge 在线内容操作、隔离实例生命周期与保留修订版清理、Fabric/Quilt/Legacy Fabric/Forge/NeoForge/OptiFine 加载器、本地内容管理和用户数据备份。整合包收据记录其拥有路径的哈希；更新会使用这些哈希检查本地修改、删除和路径冲突，然后再替换数据。磁盘用量报告统计逻辑字节数；存在硬链接时不承诺对应的物理空间回收量。用户数据备份与 Modrinth 导出用途不同：导出不包含存档和私有服务器列表，并且导出本地二进制文件需要明确授权重新分发。支持 CurseForge API 密钥不代表真实密钥已获得生产授权。Microsoft 在线登录属于实验功能；所有者于 2026-10-23 推迟了真实账户验收，离线身份仍受支持。保持核心与界面解耦；修改公开行为时同步更新中英文用户文档。
