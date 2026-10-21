# 架构与设计边界

## 目标

CialloCraftLauncher 以命令行为首个界面。可复用的 Rust 核心提供配置、目录、网络、下载和安装逻辑；命令行与未来的图形界面调用相同的核心服务。核心不依赖终端或图形界面，因此以后增加 GUI 不需要把业务逻辑移入界面层。

首批目标平台：

- Windows x86_64
- macOS arm64
- Linux x86_64

## 工作区

- `crates/ccl-core`：共享服务、配置、错误、HTTP 传输和任务事件。
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

## 下载源

版本列表和游戏文件分别设置来源，各自支持 `auto`、`official` 和 `mirror`。二者默认均为 `auto`；核心将选择适用来源和回退策略。镜像根地址可在用户配置中设置。配置结构已经存在，但来源选择、镜像 URL 映射、下载和回退流程尚未实现。

用户配置采用 JSON，默认位置为：

- Windows：`%APPDATA%/CialloCraftLauncher/config.json`
- macOS：`~/Library/Application Support/CialloCraftLauncher/config.json`
- Linux：`$XDG_CONFIG_HOME/ciallocraftlauncher/config.json`；未设置该变量时为 `~/.config/ciallocraftlauncher/config.json`

配置字段为 `downloads.version_list_source`、`downloads.file_source`、`downloads.mirror_root`、`downloads.http_proxy` 和 `downloads.max_parallel`。缺失字段使用默认值。镜像根地址必须是无凭据的 HTTPS URL，未知字段会被拒绝。默认配置文件缺失时使用默认值；通过 `--config` 显式指定的文件缺失时则报错。

## CLI 与 GUI 一致性

CLI 与 GUI 共用核心操作、配置和错误定义。CLI 不应另行实现下载或安装规则；GUI 不应要求核心启动终端或读取标准输入。`--json` 将命令执行结果格式化为 JSON。clap 帮助和参数解析错误仍为文本。CLI 命令无需交互输入即可运行。

当前核心提供配置、目录选择、HTTP 客户端和任务事件/取消基础能力。安装、下载、身份验证和 GUI 尚未实现。
