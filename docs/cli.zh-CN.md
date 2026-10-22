# 命令行

## 用法

```text
ccl [--json] [--network-offline] [--game-dir PATH] [--config PATH] <doctor|init|settings|account|versions|install|instances|loaders|instance|pack|content|online|launch|logs>
```

全局选项可以放在子命令前。命令以非交互方式运行，不会等待标准输入。

| 命令 | 行为 |
| --- | --- |
| `doctor` | 只读检查配置和游戏目录状态，不创建目录。 |
| `init` | 按[目录优先级](architecture.zh-CN.md#游戏目录选择)选择并创建游戏目录。 |
| `settings show|get|set|reset` | 显示、读取、修改或重置持久化设置；显示时会隐藏代理凭据。 |
| `account check-storage|login|list|select|refresh|logout` | 检查操作系统凭据库、使用设备代码登录 Microsoft 账户、查看或选择账户、刷新凭据或删除本地凭据。登录前会先检查凭据库。参阅[Microsoft 账户设置](accounts.zh-CN.md)。 |
| `versions [--limit N]` | 列出可用的游戏版本。`--limit N` 限制结果数量；`--limit 0` 列出全部版本。 |
| `install VERSION` | 将指定游戏版本安装到所选游戏目录。 |
| `instances` | 列出已安装的原版版本及其所需的 Java 主版本号。 |
| `loaders fabric\|quilt\|legacyfabric\|forge\|neoforge\|optifine MINECRAFT [--limit N]` | 列出上游元数据中适用于指定 Minecraft 版本的加载器版本。 |
| `instance list\|show ID\|install ID MINECRAFT LOADER LOADER_VERSION\|update ID MINECRAFT LOADER LOADER_VERSION\|rollback ID\|uninstall-loader ID` | 管理隔离的模组实例和加载器版本。参阅[模组实例与加载器](loaders.zh-CN.md)。 |
| `instance rename ID DESTINATION\|copy ID DESTINATION\|delete ID --yes\|history ID\|restore ID REVISION\|disk-usage ID\|clean ID [--keep N] [--include-previous] [--apply]` | 重命名、复制、删除、查看、恢复和清理实例保留修订版。默认只预览清理计划；只有指定 `--apply` 才会执行。参阅[实例生命周期](instances.zh-CN.md)。 |
| `pack inspect ARCHIVE\|show ID\|import ID ARCHIVE [--include-optional] [--replace-modified] [--allow-host HOST]\|export ID OUTPUT [--include-local-content]` | 检查并导入 Modrinth、CurseForge 和 MultiMC/Prism 导出包；导出 Modrinth `.mrpack`。参阅[导入指南](imports.zh-CN.md)及 [Modrinth 与实例内容指南](modpacks.zh-CN.md)。 |
| `content list ID KIND\|add ID KIND SOURCE [--name NAME]\|remove ID KIND NAME\|enable ID NAME\|disable ID NAME\|backup ID OUTPUT\|restore ID ARCHIVE` | 管理本地模组、资源包、光影包、存档和可移植用户数据备份。`KIND` 为 `mods`、`resourcepacks`、`shaderpacks` 或 `saves`。 |
| `online search\|versions\|resolve\|install\|updates\|analyze\|pack-install` | 搜索 Modrinth 或 CurseForge、解析依赖、安装或更新内容，并分析已记录的在线内容。参阅[在线内容](online-content.zh-CN.md)。 |
| `launch [VERSION] [--instance ID] [--username CCLPlayer] [--offline] [--account ID] [--java PATH] [--memory MiB] [--no-java-download] [--dry-run]` | 启动原版版本或隔离实例。默认使用所选 Microsoft 账户，也可显式使用离线账户。Java 可自动探测或按需从 Mojang 运行时清单安装。 |
| `logs [--instance ID] [--run ID] [--lines 100]` | 输出最近一次运行日志，或指定运行记录的日志，并限制输出行数。`--instance` 选择独立实例的日志。 |

启动时会选择与版本元数据所需主版本号及主机架构匹配的 Java。离线玩家 UUID 使用 Java 离线玩家的名称派生 UUID 算法。启动前，启动器会根据可用的大小和 SHA-1 元数据校验客户端 JAR、适用的库文件和资源索引，准备资源文件映射；若版本指定了客户端日志配置，还会下载并校验该配置。平台原生库会解压到临时目录，游戏退出后删除；`--dry-run` 输出计划后也会删除该目录。

托管实例使用独立的游戏目录，可通过 `ccl launch --instance ID` 启动；此时不要再提供位置版本参数。安装前会根据上游元数据检查加载器兼容关系，但不能保证所有模组组合可用，也不代表已验证所有游戏启动组合。CCL 不会捆绑 Fabric API 或其他模组。Forge 支持现代处理器安装器、spec 0 配置及受支持的旧版 `install`/`versionInfo` 通用 JAR 格式；NeoForge 使用现代处理器安装器。加载器更新和移除会保留当前实例用户数据。加载器更新后执行 `instance rollback` 会先把当前用户数据放入上一加载器版本；完整内容编辑或整合包更新后回滚则切换到完整的上一快照，包括用户数据和存档。如果需要保留较新的存档，请先备份。`instance show` 的 `loader_version` 显示最终安装配置文件 ID，可能与命令输入的加载器发行版本字符串不同。详情和示例见 [loaders.zh-CN.md](loaders.zh-CN.md)。

整合包更新会先根据归属哈希检查文件，再替换或删除文件。若整合包管理的文件已修改或删除，或新整合包路径与未托管的本地内容冲突，更新会以冲突错误（退出码 `7`）停止；`--replace-modified` 会明确允许替换或删除这些本地改动。使用前请备份本地数据。只有显式使用 `pack export --include-local-content` 才会把本地二进制文件加入导出，而且这表示你授权重新分发这些文件。内容备份可用于保存用户数据（包括存档），与 Modrinth 导出归档分开。

游戏标准输出和错误输出会捕获到 `<游戏目录>/.ccl/logs/<run-id>.log`。同目录还会写入 `<run-id>.json` 结果记录，其中包含版本、进程 ID、游戏实际 `exit_code`、取消状态、日志路径以及开始和结束时间。`logs` 命令的 JSON 响应使用 `log` 字段保存日志文本。Microsoft 账户凭据单独保存在操作系统凭据库中。Linux 必须有可用且持久的 Secret Service 和 D-Bus 会话；程序不会将凭据退回保存为明文。退出账户只删除本地凭据，不会撤销 Microsoft 授权。使用 `--offline` 或 `--username NAME` 可显式选择离线身份。

`--game-dir PATH` 显式指定游戏目录，优先于默认选择。`--config PATH` 指定配置文件；省略时读取平台默认位置，文件不存在则使用默认配置。显式指定的配置文件缺失时会报文件系统错误（退出码 `4`），但 `settings set` 和 `settings reset` 可以创建该文件。

`--json`、`--network-offline`、`--game-dir` 和 `--config` 都是全局选项，可以放在子命令前。`--network-offline` 会禁用 HTTP 请求并要求使用已验证的本地数据；它与 `launch --offline`（选择离线玩家身份）不同。只有保存的强 ETag 和服务器范围响应均匹配时，下载才会续传。`--json` 将命令执行结果输出为 JSON。进度输出到标准错误。clap 生成的帮助和参数解析错误仍为文本。

## 配置

JSON 配置字段为 `downloads.version_list_source`、`downloads.file_source`、`downloads.mirror_root`、`downloads.http_proxy`、`downloads.max_parallel` 和 `downloads.offline`。前两项分别控制版本列表和文件来源，接受 `auto`、`official` 或 `mirror`，默认均为 `auto`。缺失字段使用默认值。镜像根地址可配置。版本列表和安装流程使用官方版本清单及元数据。安装会下载客户端、库、原生库、资源索引和资源文件；依据可用元数据中的哈希和大小校验，限制并发数，重试临时失败，仅在续传条件经过验证时继续下载，复用有效缓存文件，并先写入临时文件再替换目标。多个进程通过版本锁协调，也支持取消操作。已有版本 JSON 冲突时会返回冲突错误而不会覆盖。

请求 CurseForge 在线 API 需要用户自行提供 `CCL_CURSEFORGE_API_KEY`；启动器不会内置或保存该密钥。仅当 CurseForge 归档选择的远程文件需要解析时才需要该密钥；不含所选远程文件的纯 overrides 归档无需密钥。密钥只会发送到获准的服务 API/CDN 请求。基于测试夹具的请求头范围检查不能证明已获得真实服务授权。参阅[导入](imports.zh-CN.md)和[在线内容](online-content.zh-CN.md)。

启动设置还包括 `launch.java`（可选的 Java 可执行文件或主目录）、`launch.max_memory_mb`（默认 `2048`）以及 `launch.auto_install_java`（默认 `true`）。`--java` 可覆盖配置路径；`--no-java-download` 可在本次启动时跳过托管 Java 安装。托管运行时安装在 `<游戏目录>/.ccl/runtimes`，使用 Mojang 官方运行时清单和校验文件。`auth.microsoft_client_id` 是可选的公开 OAuth 应用 ID，不是客户端密钥。参阅[Microsoft 账户设置](accounts.zh-CN.md)。

示例：

```sh
ccl settings show
ccl settings set downloads.max_parallel 6
ccl settings set launch.java /opt/java/bin/java
ccl settings set auth.microsoft_client_id 00000000-0000-0000-0000-000000000000
ccl settings set downloads.http_proxy null
ccl settings reset launch.java
ccl settings reset --all
ccl account login
ccl account list
ccl launch 1.21.1 --dry-run
ccl loaders fabric 1.21.1 --limit 10
ccl loaders quilt 1.21.4 --limit 10
ccl loaders legacyfabric 1.8.9 --limit 10
ccl loaders optifine 1.20.1
ccl instance install my-world 1.21.1 fabric 0.20.1
ccl launch --instance my-world --dry-run
ccl instance rollback my-world
ccl pack inspect ./pack.mrpack
ccl pack import my-pack ./pack.mrpack --include-optional
ccl content list my-pack mods
ccl content backup my-pack ./my-pack-userdata.zip
ccl pack export my-pack ./my-pack.mrpack --include-local-content
```

`settings get` 和 `settings show` 会隐藏代理 URL 中的用户名和密码。为网络连接所需，配置文件仍会保存完整代理 URL，因此应保护该文件。在 Unix 系统上，启动器写入的配置文件仅允许文件所有者访问。设置会先校验，再通过原子替换写入；多个 CLI 进程之间使用锁协调更新。`settings reset --all` 也可将损坏的配置替换为默认值。

## 退出码

| 码 | 含义 |
| --- | --- |
| `0` | 成功；启动游戏时，记录的游戏退出码也为 `0`。 |
| `2` | 参数错误 |
| `3` | 配置错误 |
| `4` | 文件系统错误 |
| `5` | 网络或安装下载错误 |
| `6` | 版本无效或未知 |
| `7` | 与原版版本元数据、托管实例锁或整合包文件归属/更新冲突 |
| `8` | 启动准备失败或游戏失败，包括游戏以非零状态退出 |
| `9` | Java 运行时探测或校验错误 |
| `10` | 账户或身份验证错误 |
| `11` | 加载器安装或元数据错误 |
| `12` | 托管实例错误 |
| `13` | Modrinth 整合包错误 |
| `14` | 本地内容或用户数据备份错误 |
| `15` | 在线内容平台或操作错误 |
| `16` | 归档导入格式错误 |
| `130` | 操作已取消 |

原版版本元数据冲突、托管实例锁冲突、Modrinth 整合包冲突（包括文件归属/更新冲突），以及因实例锁而无法执行的内容或整合包操作使用退出码 `7`。加载器错误（包括加载器配置文件冲突）及其他实例错误分别使用 `11` 和 `12`。其他 Modrinth 整合包和本地内容错误分别使用 `13` 和 `14`；在线内容错误使用 `15`，归档导入格式错误使用 `16`；操作取消时使用 `130`。

游戏失败时，JSON 结果记录会保留游戏实际的 `exit_code`；CLI 使用退出码 `8` 表示启动未成功。使用 `--json` 时命令执行结果为 JSON；clap 帮助和参数解析错误为文本，并使用参数错误退出码。
