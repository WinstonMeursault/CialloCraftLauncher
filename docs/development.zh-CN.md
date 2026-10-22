# 开发指南

[使用指南](user-guide.zh-CN.md) · [API 文档](api.zh-CN.md) · [English](development.en.md)

- [环境](#environment)
- [构建与运行](#build)
- [架构与工作区](#architecture)
- [代码边界](#boundaries)
- [配置与数据目录](#data)
- [实例与事务](#transactions)
- [更新、兼容性与诊断](#maintenance)
- [Microsoft 应用注册与验收](#microsoft-application)
- [验证与预览包](#validation)
- [当前功能范围](#scope)

<a id="environment"></a>

## 环境

安装 Rust 稳定版工具链及 Cargo。工作区使用 Rust 2024 edition。目标平台为 Windows x86_64、macOS arm64 和 Linux x86_64。

<a id="build"></a>

## 构建与运行

在仓库根目录运行：

```sh
cargo build --workspace
cargo run -p ccl-cli -- --help
```

用户流程见[使用指南](user-guide.zh-CN.md)，命令、输出及核心服务见 [API 文档](api.zh-CN.md)。

<a id="architecture"></a>

## 架构与工作区

### 目标

CialloCraftLauncher 以命令行为首个界面。可复用的 Rust 核心提供配置、目录、网络、下载和安装逻辑；命令行与未来的图形界面调用相同的核心服务。核心不依赖终端或图形界面，因此以后增加 GUI 不需要把业务逻辑移入界面层。

首批目标平台：

- Windows x86_64
- macOS arm64
- Linux x86_64

### 工作区

- `crates/ccl-core`：共享服务、配置、错误、HTTP 传输、任务事件、安装、Java 探测和启动编排。
- `crates/ccl-cli`：命令行参数、文本或 JSON 输出、退出码，以及对核心的调用。
- `docs`：面向用户和贡献者的公开文档。

核心通过任务上下文报告进度，并提供协作式取消句柄。界面展示事件并收集输入；核心执行工作、验证配置并返回结构化错误。核心不得依赖界面。

<a id="boundaries"></a>

## 代码边界

`loaders`、`instance` 及 `launch --instance` 与未来 GUI 复用相同核心服务。`modpack_format` 负责 Modrinth 归档解析、校验和写入；`import_formats` 检测 CurseForge 和 MultiMC/Prism 归档，并通过事务化整合包流程转换受支持的输入；`modpacks` 协调整合包导入/导出并管理所有权收据；`content` 管理本地文件和用户数据备份。在线服务集成位于 `online_content`。整合包导入和内容编辑使用与实例操作相同的暂存事务和跨进程锁。现代 Forge/NeoForge 处理器由核心启动，通过任务事件报告输出，不得直接写入 CLI 标准输出。NeoForge SHA-512 回退会先校验安装器字节，再由安装流程计算 SHA-1。

- 可复用的领域逻辑放在 `crates/ccl-core`。
- `crates/ccl-core` 不得依赖终端或图形界面库。
- CLI 负责解析参数、格式化输出和映射退出码，不复制核心业务规则。
- 任务通过核心事件报告进度，并通过取消句柄响应取消。
- 配置错误应包含易理解的上下文；不得将秘密写入日志或输出。设置更新使用核心的 `ConfigStore::update` 事务，通过跨进程锁串行化，并在验证后原子保存。账户令牌必须保存在操作系统凭据库中；Linux 需要可用且持久的 Secret Service/D-Bus 会话，不得退回保存明文。

<a id="data"></a>

## 配置与数据目录

配置字段及默认路径统一维护于[使用指南](user-guide.zh-CN.md#configuration)。

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

`modpack_format` 模块读取和写入 Modrinth 格式版本 1 归档，校验清单和目标路径，限制 ZIP 解压规模，并按顺序应用 `overrides/` 与客户端覆盖。`modpacks` 会事务性导入所选文件和覆盖内容，并在收据中记录整合包拥有路径的哈希。更新会把当前文件与归属哈希比较；整合包拥有的文件被修改或删除，或新路径与未托管本地内容冲突时，会停止更新，除非调用者明确授权替换。整合包或内容变更后，`instance rollback` 会恢复上一份完整实例快照，包括用户数据和存档；如果要保留较新的存档，应先备份。`content` 管理本地内容和可移植用户数据备份。备份包含用户数据与存档，但不包含游戏/加载器文件、Java 运行时或账户凭据。Modrinth 导出与备份用途不同：导出不包含存档和私有服务器列表，本地二进制文件需要明确授权重新分发。命令和安全说明见[Modrinth 整合包与实例内容](user-guide.zh-CN.md#packs)。命令及支持边界另见[加载器与实例](user-guide.zh-CN.md#loaders)。

<a id="maintenance"></a>

## 更新、兼容性与诊断

`update::UpdateService` 只查询 `WinstonMeursault/CialloCraftLauncher`。稳定版使用 `main` 上的 `v<VERSION>` 标签。每次向 `dev` 推送都会启动 Windows x64、macOS arm64 和 Linux x86_64 的验证与打包；所有任务通过后，才会将对应提交发布为不可变的 `dev-<完整 SHA>`。工作流保留最近十个开发版发行。稳定标签必须匹配 CLI crate 版本，且其提交位于 `main`。清单绑定仓库、渠道、分支、完整 commit、目标平台、程序哈希和数据读取范围（`1..=1`）。

`UpdateService::check` 会区分可用程序包与尚未完成发行的分支提交。`stage` 下载并校验有大小限制的 ZIP、归档路径及哈希，然后暂存候选程序。`Installation` 管理渠道选择和持久事务日志；`launch_helper` 启动受限旧程序助手，父进程退出后由 `execute_helper` 执行替换、健康检查或回滚。`prepare_rollback` 会先检查当前数据兼容性。二进制更新器不会恢复游戏数据。当前格式无需迁移；读取器保留无 schema 配置/账户/实例元数据的读取能力，并接受 schema 1 收据。未知的未来配置或实例字段以及大于 1 的收据 schema 会被拒绝。首轮回归使用从基线 `2edbf0e` 的 Git 归档生成的配置与账户元数据；夹具已脱敏，测试账户凭据没有写入文件。

发行归档未签名。HTTPS、大小和 SHA-256 检查可以发现损坏，但不能认证发布者身份；不得将其称为签名或发布者验证。尚未实现系统签名或公证。`update status`、`rollback` 和 `recover` 不需要网络；`check` 和 `apply` 需要网络，且会被 `--network-offline` 阻止。

`compatibility::inspect_compatibility(config_path, game_root)` 是只读检查，不访问操作系统凭据库，只检查 CCL 管理的元数据。未来迁移必须锁定并备份受影响数据、暂存并原子提交、支持安全的中断恢复；若保留的旧读取器无法读取新数据，则拒绝回退。不得通过恢复整个游戏目录来回退启动器。

`DiagnosticsService::doctor` 报告 Java 候选、目录访问和是否编译凭据库适配器；不探测凭据库解锁状态。网络与写入探测需显式启用。`diagnose_run` 检查有大小限制的运行日志和时间戳接近的崩溃报告，仅识别已支持模式。导出只序列化白名单字段并替换路径；原始输出预览需单独显式选择，最多 16 KiB，并尽力脱敏。它仍可能包含游戏或模组写入的秘密，CCL 不会上传该报告。

P7 仍在进行中，尚未全面验收。当前性能工作流只测 CLI 启动和一个确定性 Modrinth 夹具解析，并与同一 runner 的上次基线比较。它不代表完整安装、内存/磁盘、Minecraft 启动、发行故障注入或实机升级已覆盖。请将这些尚未覆盖项目与通过 CI 的项目分开记录。

<a id="microsoft-application"></a>

## Microsoft 应用注册与验收

> **预览状态：** Microsoft 登录已经实现，但尚未使用真实账户验证。CCL 的 OAuth 应用尚未注册。必须使用该应用通过完整的 Microsoft、Xbox 和 Minecraft 登录流程，才能满足 CLI 预览版的账户验收要求。

### 注册公开桌面应用

启动器使用 Microsoft 设备授权流程。它是公开桌面客户端，不能使用客户端密钥。Microsoft 的[桌面应用注册指南](https://learn.microsoft.com/en-us/entra/identity-platform/scenario-desktop-app-registration)和[设备代码流程文档](https://learn.microsoft.com/en-us/entra/identity-platform/v2-oauth2-device-code)介绍了桌面应用注册与公开客户端配置。

1. 使用有权创建应用注册的账户登录 [Microsoft Entra 管理中心](https://entra.microsoft.com/)。
2. 打开 **标识 → 应用程序 → 应用注册 → 新注册**。
3. 为应用填写清楚的启动器名称。当前实现使用 `/consumers` 权限机构，因此账户类型请选择 **仅个人 Microsoft 账户**。Minecraft Java 版持有者使用个人 Microsoft 账户。如果门户没有此选项，请先确认注册方案，不要直接改用更宽泛的账户类型。
4. 注册应用并复制 **应用程序（客户端）ID**。这个 GUID 是公开配置，不是密码。
5. 打开 **身份验证**。在 **高级设置** 中将 **允许公共客户端流** 设为 **是** 并保存。设备代码客户端不使用客户端密钥或浏览器重定向 URI。
6. 不要创建客户端密钥，也不要把访问令牌或刷新令牌放入启动器设置。
7. 确认你的应用当前需要满足的 Xbox 和 Minecraft 服务访问要求。完成注册本身不能证明完整登录流程可用。如果服务拒绝该应用，请根据错误和该服务的官方支持流程解决访问问题。

Microsoft 的[应用注册快速入门](https://learn.microsoft.com/en-us/entra/identity-platform/quickstart-register-app)说明了账户类型；桌面应用指南说明何时应将桌面客户端设为公开客户端，以及如何启用公开客户端流。本项目尚未验证当前应用注册和 API 访问是否已获准用于公众。

### 应用所有者的验证清单

将账户登录称为可供公众使用的预览功能前，请记录以下验证证据：

- 已注册的公开客户端 ID 与启动器配置一致，并已完成 Microsoft 要求的审核；
- 设备代码登录可用于拥有 Minecraft Java 版权益的账户；
- 拒绝、过期、取消、刷新、退出和凭据库不可用等路径均符合预期；
- CLI 输出、日志、错误文本、普通配置和账户元数据中均没有令牌；
- Linux 版本在真实且持久的 Secret Service/D-Bus 会话中可用；
- 服务/API 权限及分发方式符合当前 Microsoft、Xbox 和 Minecraft 要求。

这些检查完成前，必须将身份验证功能和预览包标注为**未经验证，不适合公开分发**。

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

单元测试夹具覆盖平台搜索与版本响应解析、依赖类型和固定版本、CurseForge 文件元数据及依赖关系、密钥脱敏与请求头限定、更新版本选择、事务式所有权、回滚和冲突。回环 HTTP 夹具会执行 Modrinth 搜索、版本列表和固定依赖递归解析。另有一个可选测试直接查询公开 Modrinth API。

<a id="scope"></a>

## 当前功能范围

核心提供配置、目录选择、HTTP 下载、版本查询与安装、安装托管 Java 运行时到 `.ccl/runtimes`、Microsoft 账户身份验证、启动计划、原生库解压、子进程输出捕获、Modrinth 整合包处理、CurseForge 和 MultiMC/Prism 归档转换、Modrinth/CurseForge 在线内容操作、隔离实例生命周期与保留修订版清理、Fabric/Quilt/Legacy Fabric/Forge/NeoForge/OptiFine 加载器、本地内容管理和用户数据备份。整合包收据记录其拥有路径的哈希；更新会使用这些哈希检查本地修改、删除和路径冲突，然后再替换数据。磁盘用量报告统计逻辑字节数；存在硬链接时不承诺对应的物理空间回收量。用户数据备份与 Modrinth 导出用途不同：导出不包含存档和私有服务器列表，并且导出本地二进制文件需要明确授权重新分发。支持 CurseForge API 密钥不代表真实密钥已获得生产授权。Microsoft 账户批准和实时端到端验证尚未完成；在这些条件满足前，不要把账户登录描述为可公开使用。保持核心与界面解耦；修改公开行为时同步更新中英文用户文档。
