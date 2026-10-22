# 命令行

## 用法

```text
ccl [--json] [--game-dir PATH] [--config PATH] <doctor|init|settings|account|versions|install|instances|launch|logs>
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
| `launch VERSION [--username CCLPlayer] [--offline] [--account ID] [--java PATH] [--memory MiB] [--no-java-download] [--dry-run]` | 默认使用所选 Microsoft 账户启动，也可显式使用离线账户。Java 可自动探测或按需从 Mojang 运行时清单安装。 |
| `logs [--run ID] [--lines 100]` | 输出最近一次运行日志，或指定运行记录的日志，并限制输出行数。 |

启动时会选择与版本元数据所需主版本号及主机架构匹配的 Java。离线玩家 UUID 使用 Java 离线玩家的名称派生 UUID 算法。启动前，启动器会根据可用的大小和 SHA-1 元数据校验客户端 JAR、适用的库文件和资源索引，准备资源文件映射；若版本指定了客户端日志配置，还会下载并校验该配置。平台原生库会解压到临时目录，游戏退出后删除；`--dry-run` 输出计划后也会删除该目录。

游戏标准输出和错误输出会捕获到 `<游戏目录>/.ccl/logs/<run-id>.log`。同目录还会写入 `<run-id>.json` 结果记录，其中包含版本、进程 ID、游戏实际 `exit_code`、取消状态、日志路径以及开始和结束时间。`logs` 命令的 JSON 响应使用 `log` 字段保存日志文本。Microsoft 账户凭据单独保存在操作系统凭据库中。Linux 必须有可用且持久的 Secret Service 和 D-Bus 会话；程序不会将凭据退回保存为明文。退出账户只删除本地凭据，不会撤销 Microsoft 授权。使用 `--offline` 或 `--username NAME` 可显式选择离线身份。

`--game-dir PATH` 显式指定游戏目录，优先于默认选择。`--config PATH` 指定配置文件；省略时读取平台默认位置，文件不存在则使用默认配置。显式指定的配置文件缺失时会报文件系统错误（退出码 `4`），但 `settings set` 和 `settings reset` 可以创建该文件。

`--json` 将命令执行结果输出为 JSON。它是全局选项，可以放在子命令前。进度输出到标准错误。clap 生成的帮助和参数解析错误仍为文本。

## 配置

JSON 配置字段为 `downloads.version_list_source`、`downloads.file_source`、`downloads.mirror_root`、`downloads.http_proxy` 和 `downloads.max_parallel`。前两项分别控制版本列表和文件来源，接受 `auto`、`official` 或 `mirror`，默认均为 `auto`。缺失字段使用默认值。镜像根地址可配置。版本列表和安装流程使用官方版本清单及元数据。安装会下载客户端、库、原生库、资源索引和资源文件；要求并校验每个下载文件的 SHA-1，限制并发数，重试临时失败，复用有效缓存文件，并通过临时文件替换目标文件。多个进程通过版本锁协调，也支持取消操作。如果已有版本 JSON 与请求的元数据冲突，将返回冲突错误而不会覆盖。

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
| `7` | 已有版本元数据与请求的安装内容冲突 |
| `8` | 启动准备失败或游戏失败，包括游戏以非零状态退出 |
| `9` | Java 运行时探测或校验错误 |
| `10` | 账户或身份验证错误 |
| `130` | 操作已取消 |

游戏失败时，JSON 结果记录会保留游戏实际的 `exit_code`；CLI 使用退出码 `8` 表示启动未成功。使用 `--json` 时命令执行结果为 JSON；clap 帮助和参数解析错误为文本，并使用参数错误退出码。
