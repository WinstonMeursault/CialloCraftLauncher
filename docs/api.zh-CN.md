# API 文档

[使用指南](user-guide.zh-CN.md) · [开发指南](development.zh-CN.md) · [English](api.en.md)

- [CLI 命令](#cli)
- [JSON 输出](#output)
- [桌面应用](#desktop)
- [核心服务索引](#core)
- [事件、取消与错误](#tasks)
- [在线内容契约](#online-contract)
- [更新、兼容性与诊断](#maintenance)
- [退出码](#exit-codes)

<a id="cli"></a>

## CLI 命令

```text
ccl [--json] [--network-offline] [--game-dir PATH] [--config PATH] <doctor|diagnose|update|init|settings|credentials|account|versions|install|instances|loaders|instance|pack|content|online|launch|logs>
```

全局选项可以放在子命令前或后。除 `credentials curseforge set` 外，命令均以非交互方式运行；该命令需要终端并关闭输入回显，使用 `--json` 时也会提示输入密钥。

| 命令 | 行为 |
| --- | --- |
| `doctor [--network-probe] [--write-probe]` | 只读检查配置、Java、目录访问和凭据库能力，不创建游戏目录。只有显式传入参数时才进行网络或临时写入探测。 |
| `diagnose [--instance ID] [--run ID] [--export PATH] [--include-log-preview]` | 分类已有运行记录，或写入本地白名单诊断报告。原始日志预览要求同时指定 `--export`，仅尽力脱敏，仍可能包含秘密。 |
| `update check\|apply [--channel stable\|dev]`；`update status\|rollback`；`update recover [--installation PATH]` | 检查或暂存所选 GitHub 渠道的校验包。`apply` 启动助手，由助手异步替换可执行文件。`status` 查看事务日志，`rollback` 启动兼容的回退，`recover` 为当前程序或显式指定的安装恢复助手操作。 |
| `init` | 按[目录优先级](user-guide.zh-CN.md#directories)选择并创建游戏目录。 |
| `settings show\|get KEY\|set KEY VALUE\|reset [KEY\|--all]` | 显示、读取、修改或重置持久化设置；显示时会隐藏代理凭据。`set` 会将布尔值、数字和 `null` 按 JSON 解析；其他值作为字符串。 |
| `credentials curseforge set\|status\|remove` | 通过隐藏输入的终端提示录入密钥、报告是否已保存及是否存在环境变量覆盖，或删除已保存密钥。密钥按所选配置文件路径隔离并保存于系统凭据库。`CCL_CURSEFORGE_API_KEY` 优先于已保存密钥；删除操作不会清除该变量。 |
| `account check-storage\|login [--client-id CLIENT_ID] [--no-browser]\|list\|select ID\|refresh [ID]\|logout [ID]` | 检查操作系统凭据库、使用设备代码登录 Microsoft 账户、查看或选择账户、刷新凭据或删除本地凭据。登录前会先检查凭据库，显示设备代码，并在默认浏览器打开验证页面；`--no-browser` 可禁用自动打开。客户端 ID 可通过命令参数或 `auth.microsoft_client_id` 设置。参阅[Microsoft 账户设置](user-guide.zh-CN.md#accounts)。 |
| `versions [--limit N]` | 列出可用的游戏版本。`--limit N` 限制结果数量；`--limit 0` 列出全部版本。 |
| `install VERSION` | 将指定游戏版本安装到所选游戏目录。 |
| `instances` | 列出已安装的原版版本及其所需的 Java 主版本号。 |
| `loaders fabric\|quilt\|legacyfabric\|forge\|neoforge\|optifine MINECRAFT [--limit N]` | 列出上游元数据中适用于指定 Minecraft 版本的加载器版本。 |
| `instance list\|show ID\|install ID MINECRAFT LOADER LOADER_VERSION\|update ID MINECRAFT LOADER LOADER_VERSION\|rollback ID\|uninstall-loader ID` | 管理隔离的模组实例和加载器版本。参阅[模组实例与加载器](user-guide.zh-CN.md#loaders)。 |
| `instance rename ID DESTINATION\|copy ID DESTINATION\|delete ID --yes\|history ID\|restore ID REVISION\|disk-usage ID\|clean ID [--keep N] [--include-previous] [--apply]` | 重命名、复制、删除、查看、恢复和清理实例保留修订版。默认只预览清理计划；只有指定 `--apply` 才会执行。参阅[实例生命周期](user-guide.zh-CN.md#instances)。 |
| `pack inspect ARCHIVE\|show ID\|import ID ARCHIVE [--include-optional] [--replace-modified] [--allow-host HOST]\|export ID OUTPUT [--include-local-content]` | 检查并导入 Modrinth、CurseForge 和 MultiMC/Prism 导出包；导出 Modrinth `.mrpack`。参阅[导入指南](user-guide.zh-CN.md#imports)及 [Modrinth 与实例内容指南](user-guide.zh-CN.md#packs)。 |
| `content list ID KIND\|add ID KIND SOURCE [--name NAME]\|remove ID KIND NAME\|enable ID NAME\|disable ID NAME\|backup ID OUTPUT\|restore ID ARCHIVE` | 管理本地模组、资源包、光影包、存档和可移植用户数据备份。`KIND` 为 `mods`、`resourcepacks`、`shaderpacks` 或 `saves`。 |
| `online search PROVIDER QUERY [--minecraft VERSION] [--loader LOADER]... [--kind KIND] [--limit N]`；`versions PROVIDER PROJECT [--minecraft VERSION] [--loader LOADER]`；`resolve PROVIDER PROJECT MINECRAFT LOADER`；`install ID PROVIDER PROJECT [--version VERSION]`；`updates ID`；`analyze ID`；`pack-install ID PROVIDER PROJECT [--version VERSION] [--include-optional]` | 搜索 Modrinth 或 CurseForge、解析依赖、安装或更新内容，并分析已记录的在线内容。平台为 `modrinth` 或 `curseforge`；参阅[在线内容](user-guide.zh-CN.md#online)。 |
| `launch [VERSION] [--instance ID] [--username NAME] [--offline] [--account ID] [--java PATH] [--memory MiB] [--no-java-download] [--dry-run]` | 启动原版版本或隔离实例。默认使用所选 Microsoft 账户，也可显式使用离线账户。Java 可自动探测或按需从 Mojang 运行时清单安装。 |
| `logs [--instance ID] [--run ID] [--lines N]` | 输出最近一次运行日志，或指定运行记录的日志，并限制输出行数（默认 `100`）。`--instance` 选择独立实例的日志。 |

启动时会选择与版本元数据所需主版本号及主机架构匹配的 Java。离线玩家 UUID 使用 Java 离线玩家的名称派生 UUID 算法。启动前，启动器会根据可用的大小和 SHA-1 元数据校验客户端 JAR、适用的库文件和资源索引，准备资源文件映射；若版本指定了客户端日志配置，还会下载并校验该配置。平台原生库会解压到临时目录，游戏退出后删除；`--dry-run` 输出计划后也会删除该目录。

托管实例使用独立的游戏目录，可通过 `ccl launch --instance ID` 启动；此时不要再提供位置版本参数。安装前会根据上游元数据检查加载器兼容关系，但不能保证所有模组组合可用，也不代表已验证所有游戏启动组合。CCL 不会捆绑 Fabric API 或其他模组。Forge 支持现代处理器安装器、spec 0 配置及受支持的旧版 `install`/`versionInfo` 通用 JAR 格式；NeoForge 使用现代处理器安装器。加载器更新和移除会保留当前实例用户数据。加载器更新后执行 `instance rollback` 会先把当前用户数据放入上一加载器版本；完整内容编辑或整合包更新后回滚则切换到完整的上一快照，包括用户数据和存档。如果需要保留较新的存档，请先备份。`instance show` 的 `loader_version` 显示最终安装配置文件 ID，可能与命令输入的加载器发行版本字符串不同。详情和示例见 [加载器操作](user-guide.zh-CN.md#loaders)。

整合包更新会先根据归属哈希检查文件，再替换或删除文件。若整合包管理的文件已修改或删除，或新整合包路径与未托管的本地内容冲突，更新会以冲突错误（退出码 `7`）停止；`--replace-modified` 会明确允许替换或删除这些本地改动。使用前请备份本地数据。只有显式使用 `pack export --include-local-content` 才会把本地二进制文件加入导出，而且这表示你授权重新分发这些文件。内容备份可用于保存用户数据（包括存档），与 Modrinth 导出归档分开。

游戏标准输出和错误输出会捕获到 `<游戏目录>/.ccl/logs/<run-id>.log`。同目录还会写入 `<run-id>.json` 结果记录，其中包含版本、进程 ID、游戏实际 `exit_code`、取消状态、日志路径以及开始和结束时间。`logs` 命令的 JSON 响应使用 `log` 字段保存日志文本。Microsoft 账户凭据单独保存在操作系统凭据库中。Linux 必须有可用且持久的 Secret Service 和 D-Bus 会话；程序不会将凭据退回保存为明文。退出账户只删除本地凭据，不会撤销 Microsoft 授权。使用 `--offline` 或 `--username NAME` 可显式选择离线身份；`--account` 不能与 `--offline` 或 `--username` 同时使用；`--offline` 和 `--username` 可以组合。选择离线模式但未传入 `--username` 时，默认玩家名为 `CCLPlayer`。

`--game-dir PATH` 显式指定游戏目录，优先于默认选择。`--config PATH` 指定配置文件；省略时读取平台默认位置，文件不存在则使用默认配置。显式指定的配置文件缺失时会报文件系统错误（退出码 `4`），但 `settings set` 和 `settings reset` 可以创建该文件。

`--json`、`--network-offline`、`--game-dir` 和 `--config` 都是全局选项，可以放在子命令前或后。`--network-offline` 会禁用 HTTP 请求并要求使用已验证的本地数据；它与 `launch --offline`（选择离线玩家身份）不同。只有保存的强 ETag 和服务器范围响应均匹配时，下载才会续传。`--json` 会在成功或操作错误时将唯一一份命令结果 JSON 输出到标准输出。操作错误使用 `{"ok":false,"error":{"code":"...","message":"..."}}`；`error.code` 标识错误类别，`message` 为人类可读信息。进度和诊断信息输出到标准错误。`account login` 会在标准错误输出 `kind: "device_authorization"` JSON 行；若无法打开浏览器，还可能输出 `kind: "browser_open"` 行。这些进度行不属于最终 JSON 结果。clap 生成的帮助和参数解析错误仍为文本。

`update check --channel` 和 `update apply --channel` 会持久化选择的渠道。稳定版跟随 `main`，使用 `v<VERSION>` 版本标签；开发版跟随 `dev`，只有三个目标平台均完成验证和打包后才发布。既有 CLI ZIP 资产名保持为 `ccl-<target>.zip`；清单名为 `manifest-<target>.json`，含 `product: "ccl-cli"`。对应的 `manifest-<target>.json.sig` 独立签名覆盖清单原始字节，包括仓库、产品、渠道、版本、分支、commit、目标平台、数据 schema 范围、可执行文件大小/哈希及程序包大小/哈希/URL。Tauri 可信注释包含 `timestamp:`、`file:` 和 `version:`；更新器会认证该注释，并要求签名版本与清单版本一致。暂存前还会检查归档大小、SHA-256 与路径。`v0.1` CLI 尚不认识此清单格式，必须手动替换安装为首个签名清单发行版。清单及其摘要经过加密签名；单独的 SHA-256 只能确认字节，不能认证发布者身份。CLI ZIP 本身没有操作系统发布者签名。更新器使用安装目录 `.ccl-update` 下的操作系统独占文件锁，串行化渠道修改、暂存、状态读取、回退准备和助手事务。其他进程持锁时操作会报更新器忙（退出码 `17`）；进程或助手退出后锁会释放。`apply` 启动受限助手后返回。助手会尝试替换可执行文件、运行健康检查，并提交或恢复旧程序；在 Windows 上遇到临时共享或权限错误时，最多重试 30 秒。没有事务时，`update status` 返回 `update_status: null`；有事务时返回状态（`staged`、`replacing`、`checking`、`committed`、`rolling_back`、`rolled_back` 或 `failed`）、当前和候选构建标识、可选错误及 `unsigned`（经过认证的签名发行中为 `false`）。事务未完成时不能暂存其他更新；使用 `update recover` 恢复助手操作。`update rollback` 仅适用于已提交的更新，并会先检查保留的旧程序能否读取当前启动器元数据；它不会恢复游戏目录。除 `status`、`rollback` 和恢复外，更新命令需要网络；`--network-offline` 会阻止更新检查和下载。

`doctor` 默认不探测网络，也不写文件。`--network-probe` 检查官方版本元数据端点；`--write-probe` 会在启动器数据目录临时创建并删除空文件。`diagnose --run ID` 读取已保存的运行记录和有大小限制的日志前缀，以分类已知故障模式。`diagnose --export PATH` 创建新的本地文件，仅包含白名单结构化字段。添加 `--include-log-preview` 会附加最多 16 KiB 的原始游戏输出；分享前请检查，因为游戏或模组日志可能包含秘密。报告不会上传。

<a id="output"></a>

## JSON 输出

命令返回带有 `ok` 的对象。成功字段按命令区分，例如 `versions`、`instance`、`managed_instances`、`launch`、`log`。错误对象包含 `ok: false`、`error.code` 和 `error.message`。错误类别包括 `configuration_error`、`filesystem_error`、`network_error`、`invalid_version`、`instance_conflict`、`authentication_error`、`account_error`、`java_error`、`launch_error`、`loader_error`、`instance_error`、`pack_conflict`、`modpack_error`、`content_error`、`content_conflict`、`online_content_error`、`import_format_error`、`update_error`、`diagnostic_error` 和 `cancelled`。进度写入标准错误；即使启用 `--json`，clap 帮助及参数错误仍为文本。

`credentials curseforge status` 返回 `provider`、`saved` 和 `environment_override`；`set` 和 `remove` 返回 `provider` 和 `saved`。这些响应不会包含密钥。保存成功仅确认本地存储成功，不代表已获得平台授权。

```json
{"ok":false,"error":{"code":"content_conflict","message":"..."}}
```

`launch` 记录包含 `run_id`、`version`、`pid`、`exit_code`、`cancelled`、`log_path`、`started_at_ms`、`finished_at_ms`。`logs` 通过 `log` 返回文本。操作流程及配置见[使用指南](user-guide.zh-CN.md)。

<a id="gui-g3"></a>
<a id="desktop-alpha"></a>
<a id="desktop"></a>

## 桌面应用

Tauri 桌面应用为 `ccl-gui`，与 CLI ZIP 分开分发。本文接口参考对应 v0.3.0；程序包可用状态以 GitHub Releases 为准。原始 v0.2.0 macOS 签名限制和 v0.2.1 应用包签名修正仍见使用指南。通过 Tauri 的 `invoke` API 调用命令。成功时返回类型化数据；失败时以 `{ "code": "...", "message": "..." }` 格式拒绝调用。GUI 错误码与 CLI 错误码和退出码分开。

常见 GUI 错误码包括 `setup_required`（确认游戏根目录）、`library_unavailable`（无法安全读取元数据）、`configuration_error`、`task_busy`、`task_not_found`、`task_not_cancellable`、`path_not_selected`、`cancelled` 和 `update_error`。命令还可能返回其他操作专属错误码；调用方应按 `code` 而不是本地化后的消息文本分支处理。

| 命令 | 用途 |
| --- | --- |
| `bootstrap`、`initialize_root`、`select_root`、`select_config` | 返回启动状态、明确创建并保存所选游戏根目录，或通过系统选择器选择已有根目录/配置。只有 `initialize_root` 会创建缺失的游戏目录；取消选择器时返回 `null`。 |
| `save_preferences`、`set_network_offline` | 保存 GUI 外观偏好，或在共享核心配置中持久化 `downloads.offline`。 |
| `library_update` | 修改游戏库显示名称、隐藏条目或恢复所有隐藏条目。 |
| `list_versions`、`list_loaders`、`install_instance` | 查询版本/加载器，并在 CCL 实例目录中安装原版或加载器配置。 |
| `inspect_import`、`import_instance` | 选择并检查受支持的本地归档，再将其导入为 CCL 实例目录。 |
| `launch_game`、`log_runs`、`read_run_log` | 启动所选项目，读取实时或已保存的运行日志。 |
| `account_list`、`account_add_offline`、`account_select`、`account_logout`、`account_login`、`account_refresh`、`account_check_storage`、`open_login_browser` | 列出账户、添加已保存的离线档案、选择/移除账户，或使用保留的实验性 CLI/设备码登录流程及凭据库检查。 |
| `cancel_task`、`task_snapshot`、`task_history`、`set_visibility` | 取消活动任务、读取当前/历史快照，或在窗口隐藏时控制事件通知。 |
| `launcher_logs`、`launcher_log_debug`、`launcher_log_export`、`frontend_diagnostic` | 读取有界启动器日志、选择记录调试导航事件、本地导出选定日志，或记录固定前端错误类别。 |
| `instance_query`、`instance_mutate`、`online_query`、`online_mutate` | 高级页面的实例/内容管理及来源发现/变更请求。 |
| `choose_path`、`inspect_dropped`、`maintenance`、`update_installation`、`gui_self_update` | 原生路径选择/导入检查、设置/来源/诊断、所选 CLI 维护和当前 GUI 应用包更新。 |

`bootstrap` 返回 `Bootstrap` 对象，字段为 `root`、`root_exists`、`setup_required`、`launch_defaults`、`config_path`、`preferences`、`entries`、`hidden_entries` 和 `warnings`。`launch_defaults` 包含可空的 `java`、数值型 `max_memory_mb`，以及布尔型 `auto_install_java` 和 `network_offline`。`preferences` 包含 `theme`（`system`、`light`、`dark`）及 `language`（`en`、`zh-CN`）。v0.3.0 API 额外返回 `reduce_transparency`、`reduce_motion`、可空的 `last_selected_instance` 和可空的 `background_image`。每个条目包含 `key`、`id`、`display_name`、`minecraft`、可空的 `loader`、`game_directory`、`launch_version` 和 `has_revision_storage`；不再提供来源分类 `kind`。key 采用不透明形式 `instance:<64 位小写十六进制字符>`，调用方应原样传回。`has_revision_storage` 表示是否支持 CCL 修订版生命周期操作，是能力标记而非所有权分类。显示名称和隐藏状态单独保存在 `.ccl/library.json`（schema 1）中。重命名只修改 `display_name`，保留 ID 和路径；隐藏会从可见游戏库移除条目，但不会删除文件；`hidden_entries` 统计已保存的隐藏记录。`library_update` 接收 `{ operation: "rename", key, name }`、`{ operation: "hide", key }` 或 `{ operation: "restore_all" }`，并返回刷新的 `Bootstrap`。重命名/隐藏仅接受当前可发现的条目；恢复后仍可发现的隐藏条目会重新显示。bootstrap 和游戏库更新不会移动或复制发现的游戏目录。警告用于报告不可用或不兼容的游戏库元数据；bootstrap 不会改写这些文件。只有在用户明确确认后，`initialize_root` 才会创建并保存建议的根目录。`select_root` 只选择已有目录。`save_preferences` 接收 `{ preferences: { theme, language, reduce_transparency, reduce_motion } }`；GUI 偏好与共享核心配置分开保存。

`install_instance` 接收 `{ id, minecraft, loader, loader_version }`，不再定义 `shared` 字段。两个加载器字段均为 null 时，会创建带有 CCL 修订存储的原版 CCL 实例目录；同时提供两项则安装受支持的加载器配置。此创建流程不会转换或移动已发现的游戏目录。`launch_game` 接收 `{ key, identity, java, memory_mb, auto_install_java, dry_run }`；`identity` 为 `{ kind: "offline" | "microsoft", username, account_id }`。离线身份使用 `account_id: null` 和可选用户名（默认 `CCLPlayer`）；Microsoft 身份要求 `account_id`，并使用 `username: null`。key 使用上述不透明游戏库条目格式。旧 GUI 请求中的来源前缀 key 和 `shared` 字段不受支持。`inspect_import` 打开原生归档选择器，取消时返回 `null`，否则返回 `{ path, summary }`；`import_instance` 接收 `{ update_existing, id, archive, include_optional, replace_modified, allowed_hosts }`。归档必须与最近一次检查或拖入的路径一致。`select_config` 取消时返回 `null`，否则选择现有有效配置并返回刷新的 `Bootstrap`。`CCL_GUI_CONFIG_PATH` 固定配置并禁用配置选择器；`CCL_GUI_GAME_DIR` 固定根目录并禁用根目录选择。

`list_versions` 返回 `{ id, kind }` 条目；`list_loaders` 接收 `{ minecraft, loader }` 并返回加载器版本记录。`account_list`、`account_select`、`account_logout` 和 `account_add_offline` 均返回 `{ accounts, selected }`。`account_add_offline { username }` 会验证并保存本地离线档案，不使用系统凭据库；选择已保存账户会更新启动选择，移除离线账户只删除本地档案。账户元数据包含 `id`、`username`、`uuid`、可空的 `xuid`、`account_type`、可空的 `oauth_client_id` 和可空的 `last_used_unix`，不含令牌。`account_login` 接收可空的 `client_id` 和 `open_browser`；`account_refresh` 接收账户 `id`；二者均返回任务快照。`open_login_browser` 接收 `task_id`，只会打开关联活动登录任务中的验证 URI。`account_check_storage` 成功时返回 `{ available: true }`。`choose_path` 接收 `content_file`、`content_directory`、`archive`、`export`、`backup`、`diagnostic` 或 `installation` 之一，并返回可空的已选路径；后续命令必须具有匹配的原生选择器授权。`cancel_task` 接收 `{ id }`；`task_history` 最多返回八个已完成快照。

长时间操作会返回 `TaskSnapshot`，字段包括 `id`、`revision`、`state`、`kind`、`cancellable`、`phase`、`completed`、可空的 `total` 和 `pid`、`logs`、可空的 `error` 和 `error_code`、可空的 `result` 及可空的 `authorization`。`state` 为 `running`、`cancelling`、`succeeded`、`failed` 或 `cancelled`。授权信息仅包含 `user_code`、`verification_uri` 和 `expires_in`；设备码和账户令牌留在 Rust 内。`gui-task` 事件会发送任务快照。接收方应忽略比已处理 `(id, revision)` 更旧的快照。一次只运行一个任务；切换页面不会取消任务。最多保留八个已完成快照。native registry 最多保存 1,000 行任务日志，每行最多 512 个字符。发送到 WebView 的每份快照——事件以及 `task_snapshot`、`task_history`、`cancel_task` 响应——最多包含最近 100 行。窗口隐藏时，前端只保留最近待处理快照；重新显示后会请求 `task_snapshot` 以同步状态。任务活动期间还会每秒同步一次（游戏运行时每五秒），且最多一个请求在途，以恢复遗漏的终态事件；空闲时停止。窗口可见时，普通进度/日志通知合并后最多每秒 10 次；生命周期和终态转换立即发送。安装/导入取消采用协作方式；同步实例事务可能不可取消，并通过 `cancellable: false` 标明。停止游戏是单独确认的操作。

GUI 中离线身份选项与持久化网络离线设置分开。`set_network_offline { offline }` 会更新共享核心 `downloads.offline` 值并返回刷新的 `Bootstrap`，因此 CLI 和 GUI 使用同一设置。GUI 外观偏好保存在独立 `gui.json`；根目录和配置选择也保存在其中。账户元数据保存在核心配置旁。已保存的离线档案不使用凭据库。GUI 中的 Microsoft 登录暂时不可用；现有实验性 CLI 登录协议仍保留。GUI 客户端 ID 仅在“设置”的高级折叠区配置。本文不声称已完成真实账户验收。参阅[桌面 GUI 使用说明](user-guide.zh-CN.md#desktop-alpha)和[开发说明](development.zh-CN.md#gui-g3)。

`task_snapshot` 返回活动任务或 `null`；`task_history` 返回最多八个终态快照。任务注册表在前端导航切换后仍保持运行。GUI 使用可收起的进度弹窗，并保留右下角入口；游戏报告 PID 后弹窗会收起。即使弹窗关闭，快照与历史契约仍然可用。

`launcher_logs { index }` 返回当前日志（`index: 0`）或四个轮转日志之一（`1..=4`），字段为 `{ text, path, debug }`。`text` 最多包含最近 1,000 行及 256 KiB。`launcher_log_debug { enabled }` 持久化是否记录调试导航事件。`launcher_log_export { index }` 打开本地保存对话框并返回是否导出；导出已显示的有界内容，不会上传。`frontend_diagnostic { event }` 仅接受 `render_error`、`unhandled_rejection`、`operation_error` 和 `navigation`；只有启用调试日志时才记录导航。持久日志位于 GUI 数据目录的 `logs/`，最多五个文件、每个不超过 5 MiB；仅允许记录启动版本/平台/提交、任务开始/终态事件和 GUI 错误码。不会持久化任意请求/响应内容、凭据、路径或 JavaScript 异常原文。

`log_runs` 按最新优先返回最多 1,000 条已保存运行记录。`read_run_log` 接收 `{ key, run_id, lines }`，返回 `{ run_id, text, truncated }`；`run_id` 可为 null 以选择最近一次运行。它最多读取文件末尾 1 MiB，最多返回 1,000 行，并将单行截为 8,192 字符；`lines` 上限为 1,000。日志页从任务快照显示当前启动日志，启动结束后恢复读取已保存运行记录。创建 CCL 实例和默认导入会在实例锁内拒绝已有 ID；显式更新已有整合包实例时保留核心冲突检查。已有根目录版本会原地发现并启动，游戏库操作不会将其转换为带修订存储的目录。

高级页面采用统一游戏库，并根据实际能力显示操作。修改加载器、本地内容操作、修订历史/还原/回滚、清理和删除整个目录均要求已有 CCL 修订存储；没有此存储的根目录版本仍可启动，也可修改显示名称或隐藏。上述操作不会隐式纳入或移动目录。`InstanceRequest` 是带 `operation` 标签的 JSON 枚举，操作值包括 `list`、`show`、`install`、`update`、`uninstall_loader`、`copy`、`delete`、`history`、`restore`、`rollback`、`disk_usage`、`clean`、`pack_inspect`、`pack_show`、`pack_export`、`content_list`、`content_add`、`content_remove`、`content_enable`、`content_disable`、`content_backup` 和 `content_restore`；未知字段会被拒绝。变更在任务中运行。重命名、复制、删除、还原、清理和本地内容编辑等同步文件系统操作使用核心实例锁和任务准入控制，开始后不可取消；实例回滚同样如此。加载器变更、在线安装、导入和启动会收到任务取消及核心清理上下文。快照包含 `cancellable`；不可取消事务的取消请求会被拒绝，关闭需等待事务完成。

各操作字段为：`list {}`、`show { id }`、`install/update { id, minecraft, loader, loader_version }`、`uninstall_loader { id }`、`copy { id, destination }`、`delete/history/rollback/disk_usage/pack_show { id }`、`restore { id, revision }`、`clean { id, keep, include_previous, apply }`、`pack_inspect { archive }`、`pack_export { id, output, include_local_content }`、`content_list { id, kind }`、`content_add { id, kind, source, name? }`、`content_remove { id, kind, name }`、`content_enable/content_disable { id, name }`，以及 `content_backup { id, output }` / `content_restore { id, archive }`。`kind` 的序列化值为 `mod`、`resource_pack`、`shader_pack` 或 `save`（与 CLI 内容类型参数不同）。`loader` 使用小写值：`fabric`、`quilt`、`legacyfabric`、`optifine`、`forge` 或 `neoforge`。

在就绪窗口中拖入一个 `.zip` 或 `.mrpack` 会检查归档并打开导入对话框。用户可编辑实例名称和导入选项，必须明确执行导入；不会自动覆盖已有实例。默认拒绝已有 ID。明确设置 `update_existing: true` 并单独确认后，可进入核心整合包更新路径；`replace_modified` 仍需单独授权。适配器只接受与对应用途匹配的原生选择器授权路径（archive、export、backup、content file/directory、diagnostic、installation 或 drop）；拖入归档在检查前也会核验 drop 授权。

`OnlineQuery` 和 `OnlineMutation` 是带 `operation` 标签且拒绝未知字段的 JSON 枚举。查询操作为 `search { provider, query, minecraft?, loader?, kind?, limit? }`、`versions { provider, project, minecraft?, loader? }`、`resolve { provider, project, version?, minecraft, loader? }`、`updates { id }` 和 `analyze { id }`。变更操作为 `install { id, provider, project, version? }` 及 `pack_install { id, provider, project, version?, include_optional? }`。来源序列化为 `modrinth` 或 `curseforge`。搜索默认空加载器列表、`kind: "mod"`、`limit: 20`。适配器创建核心 `OnlineInstallRequest { root, instance_id, provider, project_id, version_id }` 并调用带上下文的安装路径，以验证目标实例身份及加载器兼容性，并将任务上下文传递到下载和暂存变更。CurseForge 凭据来自所选配置对应的系统凭据库，除非设置了 `CCL_CURSEFORGE_API_KEY`；状态只返回 `saved` 和 `environment_override`，不返回密钥。

`MaintenanceRequest` 是带 `operation` 标签且拒绝未知字段的 JSON 枚举，接受 `settings`、`set { key, value, expected }`、`reset { key?, expected }`、`provider_status`、`provider_set { key }`、`provider_remove`、`doctor { network_probe, write_probe }`、`diagnose { key?, run? }`、`export_diagnostics { key?, run?, output, include_log_preview }` 和 `about`。GUI 可分别编辑共享下载和启动设置。`auth.microsoft_client_id` 仍保存在共享配置中，但仅在“设置”的高级折叠区显示；它是公开应用 ID。GUI Microsoft 登录暂时不可用，现有实验性 CLI 登录协议仍保留。各字段单独保存；核心配置锁内会比较 `expected`，外部 CLI/GUI 修改会导致陈旧设置错误，不会覆盖外部改动；重新加载后再保存。重置可针对单项或全部设置，也可修复损坏配置。选择其他配置会先验证并选中该文件，然后清除已缓存的路径授权和拖入归档选择。`CCL_GUI_CONFIG_PATH` 固定配置并禁用选择器。来源密钥通过密码输入框录入，仅写入系统凭据库；环境变量状态只报告存在/不存在，且变量优先。

诊断导出保存在本地，并要求通过保存路径授权。原始日志预览默认关闭，只有明确启用才会包含，且受核心大小限制；尽力脱敏不保证移除所有秘密。分享前请人工检查导出文件。`update_installation` 使用既有 `UpdateRequest` 的 `status`、`check`、`apply`、`rollback`、`recover` 操作，每项携带 `installation_root`；`check` 和 `apply` 还携带 `channel: "stable" | "dev"`。原生选择器选取含独立 CLI 可执行文件的目录，这些请求只更新所选 CLI。

`gui_self_update` 接收带 `operation` 标签的请求，操作为 `status`、`check { channel }`、`apply { channel }`、`rollback` 或 `recover`。它只作用于当前 GUI 安装，不含 `installation_root`。响应包含 `build`、`selected_channel`、`supported`、可空的 `reason`、`status`、`check`、`helper_pid` 和 `restart_required`。`check` 包含 `available`、`build_pending`、`branch_commit` 和可空的 `candidate`；事务状态包含 `phase`、`version`、`channel`、`has_backup` 和可空的 `last_error`。启动助手前，`apply`、`rollback` 和 `recover` 都会请求确认；若已启动助手，应用会退出并重启。GUI 清单名为 `gui-manifest-<target>.json`，字段为 `schema_version`、`repository`、`product`、`channel`、`version`、`commit`、`branch`、`target`、`data_schema_min`、`data_schema_max`、`os_signed` 和 `asset { format, name, size, sha256, url }`。`asset` 标识更新负载；仅用于分发的 DMG/DEB 是独立发行资产。当前安装程序没有操作系统发布者签名，故 `os_signed` 为 false。独立签名文件 `gui-manifest-<target>.json.sig` 覆盖清单原始字节，并带有 Tauri 可信的 `timestamp:`、`file:` 和 `version:` 注释；更新器会验证受签名保护的版本与清单相符。两个产品使用同一内置发行公钥。GUI 更新器接受 Windows 按用户安装的 NSIS EXE、macOS `.app.tar.gz` 或 Linux AppImage；DEB 通过包管理器更新。助手保存上一版程序包，并在 60 秒内未收到启动健康确认时回滚。源码见 [`self_update.rs`](../crates/ccl-gui/src/self_update.rs)、[`self_update_commands.rs`](../crates/ccl-gui/src/self_update_commands.rs) 和负责独立 CLI 操作的 [`update_service.rs`](../crates/ccl-gui/src/update_service.rs)。

`update_installation` 接收由 `operation` 标记的 `UpdateRequest`：`status`、`rollback` 和 `recover` 携带 `{ installation_root }`；`check` 和 `apply` 还需携带 `{ channel }`。`UpdateResponse` 使用 `status`、`checked`、`started` 或 `recovery_started` 结果标签，对应返回构建/状态、更新检查结果或助手 PID。所有操作均作为任务分发；所选目录必须是已由原生目录选择器授权的现有 CLI 安装目录。这些请求不会更新 GUI 可执行文件。

发行目标格式为 Windows x64 按用户安装的 NSIS EXE、macOS arm64 DMG，以及 Linux x86_64 AppImage 和 DEB。GUI 更新负载为 NSIS EXE、macOS `.app.tar.gz` 和 AppImage；DEB 由包管理器更新。安装程序没有操作系统发布者签名，macOS 程序包未经公证。GUI 更新清单签名认证元数据及更新负载摘要；它与 Authenticode 和 Apple 签名/公证不同。


发现页也可针对所选 Minecraft 版本解析可选版本和可空的加载器。选定版本会解析它自身固定的依赖图；安装复用同一解析路径。版本或目标筛选改变时，GUI 清除旧计划，并要求先生成计划再安装内容。

### 外观与选择命令（v0.3.0）

`save_preferences { preferences: { theme, language, reduce_transparency, reduce_motion } }` 仅保存外观并返回最新完整 `Preferences`，保留独立更新的背景和最近选择。`select_instance { key: string | null }` 只更新最近选择并返回 `Preferences`，不会启动游戏。调用方仍需在当前游戏库解析 key，失效时不能替换为另一个实例。

`select_background_image {}` 打开原生 JPG/PNG/WebP 选择器；取消返回 `null`，成功返回更新后的 `Preferences`。输入限制为 32 MiB、单边 12,000 像素、总计 4,000 万像素，在内存分配限制下解码，缩放至不超过 1920×1080 并缓存为 JPEG。`reset_background_image {}` 清除背景选择并返回 `Preferences`。`background_image_data {}` 返回有大小限制的 JPEG data URL；未选择、缓存缺失或缓存文件不安全时返回 `null`。无效图片以 `invalid_background` 拒绝且不修改偏好。唯一管理文件名为 `background.jpg`，偏好不接受任意文件系统路径。渲染失败由前端回退。背景数据只在初始化/选择后读取，不轮询。

原生 `ccl-archive-hover` 事件携带布尔值，用于受支持归档的拖入反馈。`ccl-archive-drop` 和 `ccl-drop-error` 保留既有载荷，松手后仍需确认导入。这些新增项仅影响 GUI 本机偏好，不修改核心配置或账户 schema。

<a id="core"></a>

## 核心服务索引

可复用接口由 `ccl-core` 导出，在 Rust 中使用 `ccl_core`。库处于早期开发阶段；精确签名及类型以所用提交生成的 Rust 文档为准。

```sh
cargo doc -p ccl-core --no-deps --open
```

| 服务/模块 | 主要方法 | 职责 |
| --- | --- | --- |
| `config::ConfigStore` | `get / set / reset / update` | 校验并在跨进程锁内更新设置 |
| `install::InstallService` | `list_versions / list_versions_for / install` | 版本清单、按根目录的离线缓存、原版安装 |
| `loaders::LoaderService` | `versions / install` | 加载器元数据及安装 |
| `instances::InstanceService` | `list / metadata / install / install_vanilla / install_new_loader / acquire_launch / rollback / uninstall_loader` | 托管实例及启动锁守卫 |
| `instances::InstanceService` | `rename / duplicate / delete / history / restore_revision / disk_usage / cleanup` | 生命周期、保留修订版及清理 |
| `launch::LaunchService` | `prepare / launch` | Java 选择、校验准备及进程编排 |
| `launch` | `list_installed_versions / read_logs` | 已安装原版版本及日志 |
| `modpacks::ModpackService` | `inspect / show / import / import_new / export` | Modrinth 归档及所有权收据 |
| `import_formats::ImportFormatService` | `inspect / import / import_new` | CurseForge、MultiMC/Prism 转换 |
| `content::ContentService` | `list / add / remove / set_enabled / backup / restore` | 本地内容及可移植用户数据备份 |
| `online_content::OnlineContentService` | `search / versions / install_project / install_versions / updates_for_instance / analyze_instance / download_and_import_pack` | 平台内容、依赖解析及事务 |
| `auth::MicrosoftAuth` | `begin_device_login / complete_device_login / refresh / launch_identity` | 设备授权及启动身份 |
| `accounts::AccountStore / accounts::OsSecretStore` | `list / selected / select / upsert / credentials / remove; probe` | 账户元数据及操作系统凭据库 |
| `accounts::OsProviderKeyStore` | `for_config_file / set_curseforge / get_curseforge / remove_curseforge` | 按配置隔离并保存于系统凭据库的平台密钥；不降级为明文保存 |
| `compatibility::inspect_compatibility` | `inspect_compatibility(config_path, game_root)` | 只读检查配置、账户元数据、实例和收据，不访问凭据 |
| `update::UpdateService / update::Installation` | `check / stage; channel / set_channel / status / prepare_rollback / launch_helper / for_recovery` | 发行检查和程序更新事务 |
| `diagnostics::DiagnosticsService` | `doctor / diagnose_run / export` | 本地健康检查、有限日志分类和白名单导出 |

`LaunchRequest.game_directory` 和 `PlanOptions.game_directory` 指定资源根目录。可选的 `working_directory` 选择游戏运行目录，不会迁移客户端 JAR、库、资源或启动日志。`LaunchService` 在未指定时通过 `launch::installed_working_directory` 选择目录；直接构建计划时默认使用资源根目录。计划将该目录同时用于 `${game_directory}` 和进程工作目录。服务可以在内存中选择按校验值区分的已验证资源索引，避免覆盖其他配置所需的索引；磁盘上的版本元数据保持不变。

<a id="tasks"></a>

## 事件、取消与错误

接受任务上下文的操作可传入 `TaskContext`。`EventSink` 是界面提供的 `Arc<dyn Fn(TaskEvent) + Send + Sync>`。克隆 `CancellationToken` 后可请求取消；实例游戏运行期间应持续持有对应的启动锁守卫。

`TaskEvent` 序列化使用 `kind` 标签及 snake-case 变体：`started`、`progress`、`finished`、`process_started`、`log`、`process_exited`。`started`、`progress` 和 `finished` 包含任务名；进度还包含 `completed` 和可为 null 的 `total`。进程事件包含 PID 或可为 null 的退出码；日志事件包含 `stream` 和 `line`。取消采用协作式机制：`cancel()` 设置共享标志，接受任务上下文的操作会在取消检查点读取该标志。`TaskContext::emit` 会先检查取消状态，再调用事件接收器，因此观察到取消后不会再通过该上下文发送事件。可调用 `CancellationToken::cancel` 取消操作；CLI 也会将捕获到的 Ctrl-C 转为取消。取消操作通常以 `130` 退出。游戏进程运行期间，调用方应持续持有实例启动锁守卫。核心返回类型化错误，CLI 将它们映射为下方退出码。

```rust
use std::sync::Arc;
use ccl_core::{CancellationToken, TaskContext};

let token = CancellationToken::default();
let context = TaskContext::new(token.clone(), Arc::new(|event| {
    // Forward the event to the interface / 将事件转发给界面。
    let _ = event;
}));
// Pass Some(context) to an operation / 将 Some(context) 传给操作。
token.cancel();
```

<a id="online-contract"></a>

## 在线内容契约

`ccl-core::online_content::OnlineContentService` 提供项目搜索、兼容版本查询、依赖解析、更新检查、冲突分析和安装 API，不依赖 CLI。

### 内容平台

- Modrinth 使用公开 API 查询项目和版本，并解析必需依赖。精确指定的依赖版本会被保留，并按目标 Minecraft 版本和加载器验证。可选依赖会列出但不会自动安装；分析时会报告不兼容依赖。
- CurseForge 使用其 REST API。调用方必须通过 `OnlineConfig` 提供自己的 API 密钥；核心不持久化密钥，CLI 可将用户密钥保存到系统凭据库。下载请求只会向下载请求允许的确切 HTTPS CurseForge CDN 主机附加密钥。如果 API 未提供文件下载 URL，该文件会保持不可用；启动器不会猜测网站下载地址。

搜索支持按项目类型、Minecraft 版本和加载器筛选。两个平台的版本文件都提供可用的哈希值和大小。CurseForge 数字项目 ID 会作为字符串保留在平台无关的 API 中。

### 实例操作

`install_project` 会根据目标实例选择兼容版本、解析必需依赖、下载并校验文件，再通过一次 `InstanceService::edit_content` 修订提交整组内容。它会写入 `.ccl/online-content.json`，记录项目版本和 SHA-512 所有权哈希。其他平台的项目和本地模组会保留。更新只替换该项目拥有的文件；删除旧文件前会先检查其字节是否仍与收据一致。如果拥有的文件被修改或删除，更新会返回冲突并保留当前活动版本。

`updates_for_instance` 会向对应平台检查收据中的项目。`analyze_instance` 使用收据保存的版本元数据，报告缺少的必需依赖、不兼容选择、同一项目的多个版本，以及不同内容争用同一文件名的情况。整合包版本可下载后交给现有的 Modrinth 或 CurseForge 导入转换器。

<a id="maintenance"></a>

## 更新、兼容性与诊断

更新器只从本项目 GitHub 仓库获取发行内容。稳定版由 `main` 上的 `v<VERSION>` 标签发布；开发版由 `dev` 提交触发，Windows x64、macOS arm64 和 Linux x86_64 均通过验证后才发布，并保留最近十个开发版发行。CLI 与 GUI 清单使用不同的 `product` 值（`ccl-cli`、`ccl-gui`），各有对应 `.json.sig` 文件。Tauri CLI 2.12.4 使用同一个长期 Actions 密钥签署清单原始 JSON 字节；匹配的公钥嵌入 `ccl-core`。清单声明的数据 schema 读取范围为 `1..=1`；当前读取器继续兼容无 schema 的旧配置、账户元数据和实例元数据，并接受 schema 1 收据。严格元数据中的未知字段或未来收据 schema 会阻止兼容检查和更新器回退。安装文件本身没有操作系统发布者证书签名；发行签名认证清单及程序包摘要，不代表 Authenticode 签名或 macOS 公证。

`compatibility::inspect_compatibility` 为只读检查。它检查指定配置、相邻账户元数据以及 CCL 管理的实例代次和收据；不会打开操作系统凭据库，也不会检查存档等普通游戏文件。目前没有数据迁移。未来引入格式迁移时，必须锁定并备份受影响的元数据、暂存后原子提交、支持中断恢复，并在保留的旧读取器无法读取新数据时拒绝二进制回退。

`DiagnosticsService::doctor` 检查 Java 候选、目录访问和是否编译了凭据库适配器，但不会判断用户凭据库是否已解锁。网络和写入探测需显式启用。`diagnose_run` 检查有大小限制的游戏日志前缀，以及时间戳接近该运行记录的崩溃报告，并提供已知分类和证据；未知原因仍会保持未知。结构化导出会替换路径，并排除账户、凭据、存档和服务器列表。原始日志预览是单独的可选字段，会进行尽力脱敏但不保证全面。

CLI 归档和 GUI 安装程序没有操作系统发布者证书签名。更新器使用内置发行公钥验证签名清单及其程序包摘要；这不等于 Authenticode 签名或 macOS 公证。

<a id="exit-codes"></a>

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
| `17` | 启动器更新错误，例如网络、发行无效或恢复失败 |
| `18` | 诊断检查、运行分类或导出错误 |
| `130` | 操作已取消 |

原版版本元数据冲突、托管实例锁冲突、Modrinth 整合包冲突（包括文件归属/更新冲突），以及因实例锁而无法执行的内容或整合包操作使用退出码 `7`。加载器错误（包括加载器配置文件冲突）及其他实例错误分别使用 `11` 和 `12`。其他 Modrinth 整合包和本地内容错误分别使用 `13` 和 `14`；在线内容错误使用 `15`，归档导入格式错误使用 `16`；操作取消时使用 `130`。

游戏失败时，JSON 结果记录会保留游戏实际的 `exit_code`；CLI 使用退出码 `8` 表示启动未成功。使用 `--json` 时命令执行结果为 JSON；clap 帮助和参数解析错误为文本，并使用参数错误退出码。游戏以非零状态退出时，结果记录保留游戏实际 `exit_code`，`ok` 为 false，CLI 退出码为 `8`；操作取消会设置 `cancelled` 并以 `130` 退出。
