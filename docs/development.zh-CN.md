# 开发指南

## 环境

安装 Rust 稳定版工具链及 Cargo。工作区使用 Rust 2024 edition。目标平台为 Windows x86_64、macOS arm64 和 Linux x86_64。

## 构建与运行

在仓库根目录运行：

```sh
cargo build --workspace
cargo run -p ccl-cli -- --help
```

CLI 提供 `doctor`、`init`、`settings`、`account`、`versions`、`install`、`instances`、`loaders`、`instance`、`pack`、`content`、`online`、`launch` 和 `logs`。运行 `cargo run -p ccl-cli -- --help` 查看当前参数。用户操作流程和边界参阅[实例生命周期](instances.zh-CN.md)、[归档导入](imports.zh-CN.md)、[网络与离线模式](networking.zh-CN.md)、[在线内容](online-content.zh-CN.md)、[加载器与实例](loaders.zh-CN.md)以及 [Modrinth 整合包与内容](modpacks.zh-CN.md)。启动支持 Microsoft 或离线身份、兼容 Java 运行时探测和托管安装、dry-run 计划、进程日志捕获和取消。Microsoft 账户授权和实时端到端验证仍待完成；参阅[账户注册指南](accounts.zh-CN.md)。

## 代码边界

加载器发现与独立实例流程见[模组实例与加载器](loaders.zh-CN.md)。`loaders`、`instance` 及 `launch --instance` 与未来 GUI 复用相同核心服务。`modpack_format` 负责 Modrinth 归档解析、校验和写入；`import_formats` 检测 CurseForge 和 MultiMC/Prism 归档，并通过事务化整合包流程转换受支持的输入；`modpacks` 协调整合包导入/导出并管理所有权收据；`content` 管理本地文件和用户数据备份。在线服务集成位于 `online_content`。整合包导入和内容编辑使用与实例操作相同的暂存事务和跨进程锁。现代 Forge/NeoForge 处理器由核心启动，通过任务事件报告输出，不得直接写入 CLI 标准输出。NeoForge SHA-512 回退会先校验安装器字节，再由安装流程计算 SHA-1。

- 可复用的领域逻辑放在 `crates/ccl-core`。
- `crates/ccl-core` 不得依赖终端或图形界面库。
- CLI 负责解析参数、格式化输出和映射退出码，不复制核心业务规则。
- 任务通过核心事件报告进度，并通过取消句柄响应取消。
- 配置错误应包含易理解的上下文；不得将秘密写入日志或输出。设置更新使用核心的 `ConfigStore::update` 事务，通过跨进程锁串行化，并在验证后原子保存。账户令牌必须保存在操作系统凭据库中；Linux 需要可用且持久的 Secret Service/D-Bus 会话，不得退回保存明文。

## 配置与数据目录

默认配置路径见[架构说明](architecture.zh-CN.md#下载源)。JSON 配置字段包括 `downloads.*`、`auth.microsoft_client_id`、`launch.java`、`launch.max_memory_mb` 和 `launch.auto_install_java`。缺失字段使用默认值，以兼容旧配置；显式指定但不存在的配置文件会报错。客户端 ID 属于公开配置。代理凭据会为网络请求保留，但在设置输出中隐藏。

默认游戏目录优先级为：显式 `--game-dir`、当前工作目录中已存在的 `.minecraft`、平台惯用目录、用户主目录中的 `.minecraft`，最后在当前工作目录创建新的 `.minecraft`。macOS 已存在的惯用目录可以复用。`doctor` 只读；`init` 会创建所选目录。

版本列表和文件来源偏好相互独立，接受 `auto`、`official` 或 `mirror`。镜像根地址可配置，且必须为不含 URL 凭据的 HTTPS 地址。全局选项 `--network-offline` 禁用 HTTP，并且只使用已验证的本地元数据和文件；它与离线玩家身份不同。只有保存的强 ETag 和服务器范围响应验证未完成文件后才会续传。版本列表、安装、启动准备和进程输出捕获在共享核心中实现；CLI 只负责界面格式和退出码映射。

## 当前功能范围

核心提供配置、目录选择、HTTP 下载、版本查询与安装、安装托管 Java 运行时到 `.ccl/runtimes`、Microsoft 账户身份验证、启动计划、原生库解压、子进程输出捕获、Modrinth 整合包处理、CurseForge 和 MultiMC/Prism 归档转换、Modrinth/CurseForge 在线内容操作、隔离实例生命周期与保留修订版清理、Fabric/Quilt/Legacy Fabric/Forge/NeoForge/OptiFine 加载器、本地内容管理和用户数据备份。整合包收据记录其拥有路径的哈希；更新会使用这些哈希检查本地修改、删除和路径冲突，然后再替换数据。磁盘用量报告统计逻辑字节数；存在硬链接时不承诺对应的物理空间回收量。用户数据备份与 Modrinth 导出用途不同：导出不包含存档和私有服务器列表，并且导出本地二进制文件需要明确授权重新分发。支持 CurseForge API 密钥不代表真实密钥已获得生产授权。Microsoft 账户批准和实时端到端验证尚未完成；在这些条件满足前，不要把账户登录描述为可公开使用。保持核心与界面解耦；修改公开行为时同步更新中英文用户文档。
