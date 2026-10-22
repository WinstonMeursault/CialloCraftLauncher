# 在线模组与整合包内容

`ccl-core::online_content::OnlineContentService` 提供项目搜索、兼容版本查询、依赖解析、更新检查、冲突分析和安装 API，不依赖 CLI。

## 内容平台

- Modrinth 使用公开 API 查询项目和版本，并解析必需依赖。精确指定的依赖版本会被保留，并按目标 Minecraft 版本和加载器验证。可选依赖会列出但不会自动安装；分析时会报告不兼容依赖。
- CurseForge 使用其 REST API。调用方必须通过 `OnlineConfig` 提供自己的 API 密钥；启动器不会内置或持久化该密钥。下载请求只会向下载请求允许的确切 HTTPS CurseForge CDN 主机附加密钥。如果 API 未提供文件下载 URL，该文件会保持不可用；启动器不会猜测网站下载地址。

搜索支持按项目类型、Minecraft 版本和加载器筛选。两个平台的版本文件都提供可用的哈希值和大小。CurseForge 数字项目 ID 会作为字符串保留在平台无关的 API 中。

## 实例操作

`install_project` 会根据目标实例选择兼容版本、解析必需依赖、下载并校验文件，再通过一次 `InstanceService::edit_content` 修订提交整组内容。它会写入 `.ccl/online-content.json`，记录项目版本和 SHA-512 所有权哈希。其他平台的项目和本地模组会保留。更新只替换该项目拥有的文件；删除旧文件前会先检查其字节是否仍与收据一致。如果拥有的文件被修改或删除，更新会返回冲突并保留当前活动版本。

`updates_for_instance` 会向对应平台检查收据中的项目。`analyze_instance` 使用收据保存的版本元数据，报告缺少的必需依赖、不兼容选择、同一项目的多个版本，以及不同内容争用同一文件名的情况。整合包版本可下载后交给现有的 Modrinth 或 CurseForge 导入转换器。

## CLI 示例

`online` 的平台参数接受 `modrinth` 或 `curseforge`。项目和版本参数可以使用对应平台的项目 slug/ID 和版本 ID。以下示例覆盖七个子命令：

```sh
ccl online search modrinth sodium --minecraft 1.21.1 --loader fabric --kind mod --limit 10
ccl online versions modrinth sodium --minecraft 1.21.1 --loader fabric
ccl online resolve modrinth sodium 1.21.1 fabric
ccl online install my-instance modrinth sodium --version VERSION_ID
ccl online updates my-instance
ccl online analyze my-instance
ccl online pack-install my-pack modrinth example-pack --version VERSION_ID --include-optional
```

`install` 会解析项目及必需依赖，并安装到已有实例。`pack-install` 会下载平台整合包并创建或更新实例。`updates` 和 `analyze` 会检查已有实例的收据。调用 CurseForge API 的命令需要在进程环境中设置 `CCL_CURSEFORGE_API_KEY`；Modrinth 不需要密钥。密钥请求头范围测试成功不代表真实 CurseForge 密钥已获得在线授权。

这些 API 使用 `DownloadConfig` 的代理和离线设置。CurseForge 搜索、版本查询和直接下载都需要用户提供密钥；Modrinth 不需要密钥。启用离线模式时，在线请求会失败。

## 验证

单元测试夹具覆盖平台搜索与版本响应解析、依赖类型和固定版本、CurseForge 文件元数据及依赖关系、密钥脱敏与请求头限定、更新版本选择、事务式所有权、回滚和冲突。回环 HTTP 夹具会执行 Modrinth 搜索、版本列表和固定依赖递归解析。另有一个可选测试直接查询公开 Modrinth API。
