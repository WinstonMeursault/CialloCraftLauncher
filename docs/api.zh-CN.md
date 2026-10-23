# API 文档

[使用指南](user-guide.zh-CN.md) · [开发指南](development.zh-CN.md) · [English](api.en.md)

- [CLI 命令](#cli)
- [JSON 输出](#output)
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
| `launch [VERSION] [--instance ID] [--username CCLPlayer] [--offline] [--account ID] [--java PATH] [--memory MiB] [--no-java-download] [--dry-run]` | 启动原版版本或隔离实例。默认使用所选 Microsoft 账户，也可显式使用离线账户。Java 可自动探测或按需从 Mojang 运行时清单安装。 |
| `logs [--instance ID] [--run ID] [--lines N]` | 输出最近一次运行日志，或指定运行记录的日志，并限制输出行数（默认 `100`）。`--instance` 选择独立实例的日志。 |

启动时会选择与版本元数据所需主版本号及主机架构匹配的 Java。离线玩家 UUID 使用 Java 离线玩家的名称派生 UUID 算法。启动前，启动器会根据可用的大小和 SHA-1 元数据校验客户端 JAR、适用的库文件和资源索引，准备资源文件映射；若版本指定了客户端日志配置，还会下载并校验该配置。平台原生库会解压到临时目录，游戏退出后删除；`--dry-run` 输出计划后也会删除该目录。

托管实例使用独立的游戏目录，可通过 `ccl launch --instance ID` 启动；此时不要再提供位置版本参数。安装前会根据上游元数据检查加载器兼容关系，但不能保证所有模组组合可用，也不代表已验证所有游戏启动组合。CCL 不会捆绑 Fabric API 或其他模组。Forge 支持现代处理器安装器、spec 0 配置及受支持的旧版 `install`/`versionInfo` 通用 JAR 格式；NeoForge 使用现代处理器安装器。加载器更新和移除会保留当前实例用户数据。加载器更新后执行 `instance rollback` 会先把当前用户数据放入上一加载器版本；完整内容编辑或整合包更新后回滚则切换到完整的上一快照，包括用户数据和存档。如果需要保留较新的存档，请先备份。`instance show` 的 `loader_version` 显示最终安装配置文件 ID，可能与命令输入的加载器发行版本字符串不同。详情和示例见 [加载器操作](user-guide.zh-CN.md#loaders)。

整合包更新会先根据归属哈希检查文件，再替换或删除文件。若整合包管理的文件已修改或删除，或新整合包路径与未托管的本地内容冲突，更新会以冲突错误（退出码 `7`）停止；`--replace-modified` 会明确允许替换或删除这些本地改动。使用前请备份本地数据。只有显式使用 `pack export --include-local-content` 才会把本地二进制文件加入导出，而且这表示你授权重新分发这些文件。内容备份可用于保存用户数据（包括存档），与 Modrinth 导出归档分开。

游戏标准输出和错误输出会捕获到 `<游戏目录>/.ccl/logs/<run-id>.log`。同目录还会写入 `<run-id>.json` 结果记录，其中包含版本、进程 ID、游戏实际 `exit_code`、取消状态、日志路径以及开始和结束时间。`logs` 命令的 JSON 响应使用 `log` 字段保存日志文本。Microsoft 账户凭据单独保存在操作系统凭据库中。Linux 必须有可用且持久的 Secret Service 和 D-Bus 会话；程序不会将凭据退回保存为明文。退出账户只删除本地凭据，不会撤销 Microsoft 授权。使用 `--offline` 或 `--username NAME` 可显式选择离线身份；`--account` 不能与 `--offline` 或 `--username` 同时使用；`--offline` 和 `--username` 可以组合。选择离线模式但未传入 `--username` 时，默认玩家名为 `CCLPlayer`。

`--game-dir PATH` 显式指定游戏目录，优先于默认选择。`--config PATH` 指定配置文件；省略时读取平台默认位置，文件不存在则使用默认配置。显式指定的配置文件缺失时会报文件系统错误（退出码 `4`），但 `settings set` 和 `settings reset` 可以创建该文件。

`--json`、`--network-offline`、`--game-dir` 和 `--config` 都是全局选项，可以放在子命令前或后。`--network-offline` 会禁用 HTTP 请求并要求使用已验证的本地数据；它与 `launch --offline`（选择离线玩家身份）不同。只有保存的强 ETag 和服务器范围响应均匹配时，下载才会续传。`--json` 会在成功或操作错误时将唯一一份命令结果 JSON 输出到标准输出。操作错误使用 `{"ok":false,"error":{"code":"...","message":"..."}}`；`error.code` 标识错误类别，`message` 为人类可读信息。进度和诊断信息输出到标准错误。`account login` 会在标准错误输出 `kind: "device_authorization"` JSON 行；若无法打开浏览器，还可能输出 `kind: "browser_open"` 行。这些进度行不属于最终 JSON 结果。clap 生成的帮助和参数解析错误仍为文本。

`update check --channel` 和 `update apply --channel` 会持久化选择的渠道。稳定版跟随 `main`，使用 `v<VERSION>` 版本标签；开发版跟随 `dev`，只有三个目标平台均完成验证和打包后才发布。更新器会核对仓库、分支、commit、目标平台和数据读取范围，并在暂存前检查归档大小、SHA-256 与路径。发行包未签名：HTTPS 和 SHA-256 可以检测传输或归档损坏，但不能验证发布者身份。更新器使用安装目录 `.ccl-update` 下的操作系统独占文件锁，串行化渠道修改、暂存、状态读取、回退准备和助手事务。其他进程持锁时操作会报更新器忙（退出码 `17`）；进程或助手退出后锁会释放。`apply` 启动受限助手后返回。助手会尝试替换可执行文件、运行健康检查，并提交或恢复旧程序；在 Windows 上遇到临时共享或权限错误时，最多重试 30 秒。没有事务时，`update status` 返回 `update_status: null`；有事务时返回状态（`staged`、`replacing`、`checking`、`committed`、`rolling_back`、`rolled_back` 或 `failed`）、当前和候选构建标识、可选错误及 `unsigned`。事务未完成时不能暂存其他更新；使用 `update recover` 恢复助手操作。`update rollback` 仅适用于已提交的更新，并会先检查保留的旧程序能否读取当前启动器元数据；它不会恢复游戏目录。除 `status`、`rollback` 和恢复外，更新命令需要网络；`--network-offline` 会阻止更新检查和下载。

`doctor` 默认不探测网络，也不写文件。`--network-probe` 检查官方版本元数据端点；`--write-probe` 会在启动器数据目录临时创建并删除空文件。`diagnose --run ID` 读取已保存的运行记录和有大小限制的日志前缀，以分类已知故障模式。`diagnose --export PATH` 创建新的本地文件，仅包含白名单结构化字段。添加 `--include-log-preview` 会附加最多 16 KiB 的原始游戏输出；分享前请检查，因为游戏或模组日志可能包含秘密。报告不会上传。

<a id="output"></a>

## JSON 输出

命令返回带有 `ok` 的对象。成功字段按命令区分，例如 `versions`、`instance`、`managed_instances`、`launch`、`log`。错误对象包含 `ok: false`、`error.code` 和 `error.message`。错误类别包括 `configuration_error`、`filesystem_error`、`network_error`、`invalid_version`、`instance_conflict`、`authentication_error`、`account_error`、`java_error`、`launch_error`、`loader_error`、`instance_error`、`pack_conflict`、`modpack_error`、`content_error`、`content_conflict`、`online_content_error`、`import_format_error`、`update_error`、`diagnostic_error` 和 `cancelled`。进度写入标准错误；即使启用 `--json`，clap 帮助及参数错误仍为文本。

`credentials curseforge status` 返回 `provider`、`saved` 和 `environment_override`；`set` 和 `remove` 返回 `provider` 和 `saved`。这些响应不会包含密钥。保存成功仅确认本地存储成功，不代表已获得平台授权。

```json
{"ok":false,"error":{"code":"content_conflict","message":"..."}}
```

`launch` 记录包含 `run_id`、`version`、`pid`、`exit_code`、`cancelled`、`log_path`、`started_at_ms`、`finished_at_ms`。`logs` 通过 `log` 返回文本。操作流程及配置见[使用指南](user-guide.zh-CN.md)。

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
| `instances::InstanceService` | `list / metadata / install / acquire_launch / rollback / uninstall_loader` | 托管实例及启动锁守卫 |
| `instances::InstanceService` | `rename / duplicate / delete / history / restore_revision / disk_usage / cleanup` | 生命周期、保留修订版及清理 |
| `launch::LaunchService` | `prepare / launch` | Java 选择、校验准备及进程编排 |
| `launch` | `list_installed_versions / read_logs` | 已安装原版版本及日志 |
| `modpacks::ModpackService` | `inspect / show / import / export` | Modrinth 归档及所有权收据 |
| `import_formats::ImportFormatService` | `inspect / import` | CurseForge、MultiMC/Prism 转换 |
| `content::ContentService` | `list / add / remove / set_enabled / backup / restore` | 本地内容及可移植用户数据备份 |
| `online_content::OnlineContentService` | `search / versions / install_project / install_versions / updates_for_instance / analyze_instance / download_and_import_pack` | 平台内容、依赖解析及事务 |
| `auth::MicrosoftAuth` | `begin_device_login / complete_device_login / refresh / launch_identity` | 设备授权及启动身份 |
| `accounts::AccountStore / accounts::OsSecretStore` | `list / selected / select / upsert / credentials / remove; probe` | 账户元数据及操作系统凭据库 |
| `accounts::OsProviderKeyStore` | `for_config_file / set_curseforge / get_curseforge / remove_curseforge` | 按配置隔离并保存于系统凭据库的平台密钥；不降级为明文保存 |
| `compatibility::inspect_compatibility` | `inspect_compatibility(config_path, game_root)` | 只读检查配置、账户元数据、实例和收据，不访问凭据 |
| `update::UpdateService / update::Installation` | `check / stage; channel / set_channel / status / prepare_rollback / launch_helper / for_recovery` | 发行检查和程序更新事务 |
| `diagnostics::DiagnosticsService` | `doctor / diagnose_run / export` | 本地健康检查、有限日志分类和白名单导出 |

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

更新器只从本项目 GitHub 仓库获取发行内容。稳定版由 `main` 上的 `v<VERSION>` 标签发布；开发版由 `dev` 提交触发，Windows x64、macOS arm64 和 Linux x86_64 均通过验证后才发布，并保留最近十个开发版发行。清单声明的数据 schema 读取范围为 `1..=1`；当前读取器继续兼容无 schema 的旧配置、账户元数据和实例元数据，并接受 schema 1 收据。严格元数据中的未知字段或未来收据 schema 会阻止兼容检查和更新器回退。

`compatibility::inspect_compatibility` 为只读检查。它检查指定配置、相邻账户元数据以及 CCL 管理的实例代次和收据；不会打开操作系统凭据库，也不会检查存档等普通游戏文件。目前没有数据迁移。未来引入格式迁移时，必须锁定并备份受影响的元数据、暂存后原子提交、支持中断恢复，并在保留的旧读取器无法读取新数据时拒绝二进制回退。

`DiagnosticsService::doctor` 检查 Java 候选、目录访问和是否编译了凭据库适配器，但不会判断用户凭据库是否已解锁。网络和写入探测需显式启用。`diagnose_run` 检查有大小限制的游戏日志前缀，以及时间戳接近该运行记录的崩溃报告，并提供已知分类和证据；未知原因仍会保持未知。结构化导出会替换路径，并排除账户、凭据、存档和服务器列表。原始日志预览是单独的可选字段，会进行尽力脱敏但不保证全面。

发行包未签名；完整性校验不能证明发布者身份。

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
