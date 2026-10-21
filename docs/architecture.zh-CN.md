# 架构与设计边界

## 目标

CialloCraftLauncher 以命令行为首个界面。可复用的 Rust 核心提供配置、目录、网络、下载和安装逻辑；命令行与未来的图形界面调用相同的核心服务。核心不依赖终端或图形界面，因此以后增加 GUI 不需要把业务逻辑移入界面层。

首批目标平台：

- Windows x86_64
- macOS arm64
- Linux x86_64

## 工作区

- `crates/ccl-core`：共享服务、配置、错误、HTTP 传输、任务事件、安装、Java 探测和启动编排。
- `crates/ccl-cli`：命令行参数、文本或 JSON 输出、退出码，以及对核心的调用。
- `docs`：面向用户和贡献者的公开文档。

核心通过任务上下文报告进度，并提供协作式取消句柄。界面展示事件并收集输入；核心执行工作、验证配置并返回结构化错误。核心不得依赖界面。

## 游戏目录选择

默认游戏目录按以下顺序选择：

1. 显式指定的 `--game-dir` 路径。
2. 当前工作目录中已存在的 `.minecraft` 目录。
3. 平台惯用的 Minecraft 数据目录；macOS 上可复用已存在的目录。
4. 用户主目录中的 `.minecraft`。
5. 在当前工作目录创建 `.minecraft`。

`doctor` 检查配置和目录状态，不创建或修改游戏目录。`init` 按此顺序选择并创建游戏目录。

CLI 还提供 `versions`，读取官方版本清单，并支持通过 `--limit N` 限制结果数量（`0` 表示全部版本）；`install VERSION` 会在游戏目录中安装指定版本。`settings` 提供有类型的用户配置读取、校验、更新和重置。跨进程锁会串行化每次读取、修改和写入事务；配置会先校验，再通过原子文件替换保存。

`instances` 列出已安装的原版版本及其 Java 要求。`launch VERSION` 会选择与元数据所需主版本号和主机架构匹配的 Java 运行时，准备并校验已安装文件，然后使用已选择的 Microsoft 账户启动，也可显式指定离线身份。缺少 Java 时，可从 Mojang 官方组件清单安装到 `.ccl/runtimes`，并校验运行时文件。Microsoft 访问令牌和刷新令牌保存在操作系统凭据库中，账户元数据单独存储。Linux 必须有持久的 Secret Service/D-Bus 支持，不提供明文回退。账户登录已实现，但应用的实时批准和端到端验证尚未完成；参阅[账户设置](accounts.zh-CN.md)。启动器会捕获进程的两个输出流，并在 `.ccl/logs` 下写入运行日志和 JSON 结果记录。`logs` 读取这些记录对应的文本日志；`--dry-run` 会准备结构化启动计划，并清理临时原生库文件，但不启动游戏。启动和安装行为在共享核心中实现，与 GUI 解耦。

## 下载源

版本列表和游戏文件分别设置来源，各自支持 `auto`、`official` 和 `mirror`。二者默认均为 `auto`；镜像根地址可在用户配置中设置。当前版本列表和安装流程使用官方版本清单及版本元数据。安装会下载客户端、库、原生库、资源索引和资源文件。要求并校验每个下载文件的 SHA-1，限制并发下载数，重试临时失败，复用有效缓存文件，并先写入临时文件再替换目标。跨进程版本锁可避免多个安装同时写入同一版本。任务支持协作式取消。如果已有版本 JSON 与请求的元数据冲突，将报告冲突而不会替换文件。

用户配置采用 JSON，默认位置为：

- Windows：`%APPDATA%/CialloCraftLauncher/config.json`
- macOS：`~/Library/Application Support/CialloCraftLauncher/config.json`
- Linux：`$XDG_CONFIG_HOME/ciallocraftlauncher/config.json`；未设置该变量时为 `~/.config/ciallocraftlauncher/config.json`

配置字段包括下载偏好、可选的 `auth.microsoft_client_id`（公开客户端 ID），以及 `launch.java`、`launch.max_memory_mb`（默认 2048）和 `launch.auto_install_java`（默认 true）。缺失字段会使用默认值，以兼容旧配置。镜像根地址必须是无凭据的 HTTPS URL，未知字段会被拒绝。显示设置时会隐藏代理凭据。CLI 在 Unix 上写入的配置文件仅允许所有者访问。默认配置文件缺失时使用默认值；通过 `--config` 显式指定的文件缺失时则报错。

## CLI 与 GUI 一致性

CLI 与 GUI 共用核心操作、配置和错误定义。CLI 不应另行实现下载或安装规则；GUI 不应要求核心启动终端或读取标准输入。`--json` 将命令执行结果格式化为 JSON。clap 帮助和参数解析错误仍为文本。CLI 命令无需交互输入即可运行。

核心提供配置、目录选择、HTTP 传输、任务事件与取消、版本列表、安装、托管 Java 运行时安装、账户存储与身份验证、已安装版本列表、启动准备和进程输出捕获。应用注册和凭据存储要求见[Microsoft 账户设置](accounts.zh-CN.md)。Microsoft 实时批准与身份验证验证仍未完成；启用账户的公开预览尚不能分发。
