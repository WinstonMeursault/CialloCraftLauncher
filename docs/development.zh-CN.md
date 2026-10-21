# 开发指南

## 环境

安装 Rust 稳定版工具链及 Cargo。工作区使用 Rust 2024 edition。目标平台为 Windows x86_64、macOS arm64 和 Linux x86_64。

## 构建与运行

在仓库根目录运行：

```sh
cargo build --workspace
cargo run -p ccl-cli -- --help
```

CLI 提供 `doctor`、`init`、`versions`、`install`、`instances`、`launch` 和 `logs`。运行 `cargo run -p ccl-cli -- --help` 查看当前参数。启动支持离线身份、兼容 Java 运行时探测、dry-run 计划、进程日志捕获和取消。

## 代码边界

- 可复用的领域逻辑放在 `crates/ccl-core`。
- `crates/ccl-core` 不得依赖终端或图形界面库。
- CLI 负责解析参数、格式化输出和映射退出码，不复制核心业务规则。
- 任务通过核心事件报告进度，并通过取消句柄响应取消。
- 配置错误应包含易理解的上下文；不得将秘密写入日志或输出。

## 配置与数据目录

默认配置路径见[架构说明](architecture.zh-CN.md#下载源)。JSON 配置字段为 `downloads.version_list_source`、`downloads.file_source`、`downloads.mirror_root`、`downloads.http_proxy` 和 `downloads.max_parallel`。缺失字段使用默认值；显式指定但不存在的配置文件会报错。

默认游戏目录优先级为：显式 `--game-dir`、当前工作目录中已存在的 `.minecraft`、平台惯用目录、用户主目录中的 `.minecraft`，最后在当前工作目录创建新的 `.minecraft`。macOS 已存在的惯用目录可以复用。`doctor` 只读；`init` 会创建所选目录。

版本列表和文件来源偏好相互独立，接受 `auto`、`official` 或 `mirror`。镜像根地址可配置，且必须为不含 URL 凭据的 HTTPS 地址。版本列表、安装、启动准备和进程输出捕获在共享核心中实现；CLI 只负责界面格式和退出码映射。

## 当前功能范围

核心提供配置、目录选择、HTTP 下载、版本查询与安装、Java 运行时探测、启动计划、原生库解压和子进程输出捕获。启动当前使用离线账户，不提供账户身份验证。保持核心与界面解耦；修改公开行为时同步更新中英文用户文档。
