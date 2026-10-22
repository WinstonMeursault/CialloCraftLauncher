# 模组实例与加载器

CCL 可以查询上游加载器元数据，并将 Fabric、Forge 和 NeoForge 安装到彼此隔离的托管实例中。安装前会根据上游版本列表检查兼容关系。该检查只确认上游是否将所选加载器版本列为适用于指定 Minecraft 版本；它不会验证所有模组、整合包、Java 环境或实际游戏启动组合。

Fabric 配置文件来自 Fabric Meta。Forge 和 NeoForge 使用当前支持的现代安装器格式。当前安装流程无法处理的旧版安装器格式会返回不支持格式的错误。

## 查询加载器版本

```sh
ccl loaders fabric 1.21.1
ccl loaders forge 1.20.1 --limit 10
ccl loaders neoforge 1.21.1 --limit 10
```

命令会按从新到旧的顺序列出兼容的加载器版本。将所选版本字符串传给 `instance install` 或 `instance update`。

## 管理隔离实例

```sh
ccl instance install my-world 1.21.1 fabric 0.20.1
ccl instance list
ccl instance show my-world
ccl launch --instance my-world --dry-run
ccl instance update my-world 1.21.1 fabric 0.20.1
ccl instance rollback my-world
ccl instance uninstall-loader my-world
```

每个托管实例都拥有独立的游戏目录、版本元数据、模组、配置和存档，不会与其他实例共用所选的原版游戏目录。安装或更新会先构建暂存版本，成功后再切换到该版本。更新会保留上一个成功版本，供 `rollback` 切换。移除加载器会让实例恢复为原版，同时保留用户游戏数据。这些操作会在不同版本间保留模组、配置和存档。

`instance show` 中的 `loader_version` 记录最终安装配置文件的 ID，例如 `fabric-loader-0.20.1-1.21.1`；它不一定等于命令行传入的加载器发行版本字符串。启动实例时会使用其已安装配置文件和独立游戏目录。

CCL 不会捆绑 Fabric API、模组或整合包。请另行将它们安装到实例的 `mods` 目录。上游兼容元数据只用于安装前检查，不能保证特定模组组合一定可用。

这些命令也支持 `--json`。`loaders` 返回 `loader_versions`；`instance list`、`show` 和生命周期操作会返回 `managed_instances` 或 `instance` 记录。网络和安装错误遵循通用 [CLI 退出码](cli.zh-CN.md#退出码)。
