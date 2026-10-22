# 模组实例与加载器

CCL 可以查询加载器元数据，并将 Fabric、Quilt、Legacy Fabric、Forge、NeoForge 和 OptiFine 安装到彼此隔离的托管实例中。安装前会根据对应提供方的版本列表检查兼容关系。该检查只确认所选版本是否被列为适用于指定 Minecraft 版本；它不会验证所有模组、整合包、Java 环境或启动组合。

Fabric 和 Legacy Fabric 配置文件来自各自的 Meta API；Quilt 配置文件来自 Quilt Meta。Forge 支持处理器安装器、含嵌入式 Maven 文件的 spec 0 配置，以及较早的 `install`/`versionInfo` 通用 JAR 格式。NeoForge 使用处理器安装器和嵌入式运行库。无法识别的安装器结构会明确返回不支持格式错误。

OptiFine 版本列表通过已配置的 BMCLAPI 兼容镜像获取。CCL 会通过 HTTPS 下载全新的安装器，检查归档大小及其中声明的 Minecraft/OptiFine 版本；若安装器包含内置补丁程序，则会针对原版客户端 JAR 运行该程序。OptiFine 此元数据接口不提供校验和，因此 CCL 不会把之前下载的安装器作为已验证缓存复用。

已完成冷安装验证：Minecraft `1.21.4` 的 Quilt `0.20.0-beta.9`、Minecraft `1.21.1` 的 NeoForge `21.1.234`，以及 Minecraft `1.20.1` 的 OptiFine `HD_U_I6`。较旧的 Forge 和 Legacy Fabric 安装不包含在此验证说明中。这些结果不代表其他版本、平台或第三方模组已验证。

## 查询加载器版本

```sh
ccl loaders fabric 1.21.1
ccl loaders quilt 1.21.4
ccl loaders legacyfabric 1.8.9
ccl loaders forge 1.20.1 --limit 10
ccl loaders neoforge 1.21.1 --limit 10
ccl loaders optifine 1.20.1
```

命令会按从新到旧的顺序列出兼容的加载器版本。将所选版本字符串传给 `instance install` 或 `instance update`。

## 管理隔离实例

```sh
ccl instance install my-world 1.21.1 fabric 0.20.1
ccl instance install my-quilt-world 1.21.4 quilt 0.20.0-beta.9
ccl instance install my-optifine-world 1.20.1 optifine HD_U_I6
ccl instance list
ccl instance show my-world
ccl launch --instance my-world --dry-run
ccl logs --instance my-world
ccl instance update my-world 1.21.1 fabric 0.20.1
ccl instance rollback my-world
ccl instance uninstall-loader my-world
```

每个托管实例都拥有独立的游戏目录、版本元数据、模组、配置和存档，不会与其他实例共用所选的原版游戏目录。安装或更新会先构建暂存版本，成功后再切换到该版本。更新会保留上一个成功版本，供 `rollback` 切换。移除加载器会让实例恢复为原版，同时保留用户游戏数据。这些操作会在不同版本间保留模组、配置和存档。

`instance show` 中的 `loader_version` 记录最终安装配置文件的 ID，例如 `fabric-loader-0.20.1-1.21.1`；它不一定等于命令行传入的加载器发行版本字符串。启动实例时会使用其已安装配置文件和独立游戏目录。

CCL 不会捆绑 Fabric API、模组或整合包。请另行将它们安装到实例的 `mods` 目录。上游兼容元数据只用于安装前检查，不能保证特定模组组合一定可用。

每个实例同时只选择一个加载器。模组应适配该加载器及 Minecraft 版本；Fabric、Forge、NeoForge 模组文件通常不能互换。更换加载器时会保留原有 `mods` 目录，因此使用新加载器启动前应检查其中的文件。移除加载器后仍保留这些文件，但原版游戏不会加载它们。

Forge 系安装器首先使用官方 Maven SHA-1 文件校验。如果该文件不可用，CCL 会获取官方 SHA-512 文件，先验证安装器字节，再派生安装流程使用的 SHA-1。只有安装器 URL、记录的 SHA-1 和文件大小全部匹配时才会复用本地记录。如果两种上游校验和都无法获取且没有有效匹配记录，安装会失败。离线安装可以复用已验证的安装器记录，但其他所需文件也必须已在本地；查询加载器版本仍需联网。

元数据和校验和来源：[Fabric Meta](https://meta.fabricmc.net/)、[Quilt Meta](https://meta.quiltmc.org/)、[Legacy Fabric Meta API](https://github.com/Legacy-Fabric/legacy-meta)、[OptiFine 下载页](https://optifine.net/downloads)、[Forge Maven](https://maven.minecraftforge.net/) 和 [NeoForge Maven](https://maven.neoforged.net/releases/)。

这些命令也支持 `--json`。`loaders` 返回 `loader_versions`；`instance list`、`show` 和生命周期操作会返回 `managed_instances` 或 `instance` 记录。网络和安装错误遵循通用 [CLI 退出码](cli.zh-CN.md#退出码)。
