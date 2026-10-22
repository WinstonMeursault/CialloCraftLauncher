# 导入 CurseForge、MultiMC 和 Prism 归档

`pack inspect ARCHIVE` 无需下载或安装即可检查归档。`pack import ID ARCHIVE` 根据清单识别 Modrinth、CurseForge、MultiMC/Prism 导出。标准导出的清单可位于 ZIP 根目录或单层包裹文件夹中。Modrinth 文件无需使用 `.mrpack` 扩展名也能被识别。

```sh
ccl pack inspect ./export.zip
ccl pack import imported ./export.zip
ccl pack import imported ./export-v2.zip --replace-modified
```

## CurseForge

CCL 从 `manifest.json` 读取 Minecraft 和加载器版本，通过官方 API 解析固定的项目/文件 ID，校验声明的 SHA-1 和大小，并以事务导入覆盖文件及选定客户端文件。默认跳过可选文件，使用 `--include-optional` 可包含它们。文件下载仅向明确识别的 CurseForge HTTPS CDN 主机发送 `x-api-key`；不会把密钥加入归档 URL、收据或日志。重定向到其他允许主机时不会发送密钥。

远程文件解析需要在 `CCL_CURSEFORGE_API_KEY` 中提供自己的 API key。不要把密钥放入命令参数或提交到仓库。不含选定远程文件的纯覆盖归档可在没有 key 时转换。若受限文件没有提供者授权的下载 URL，会明确报错；CCL 不猜测下载 URL，也不绕过作者的分发选择。API 和 CDN 要求见 [CurseForge 文档](https://docs.curseforge.com/rest-api/)及其 [CDN 身份验证公告](https://blog.curseforge.com/introducing-api-key-authentication-for-curseforge-file-downloads/)。

本项目目前没有所有者提供的 CurseForge API key 用于真实验收。提供者行为通过服务/格式夹具检查，这不代表已取得生产 API 授权。

## MultiMC 和 Prism

CCL 读取 `mmc-pack.json` 组件，并从 `minecraft/` 或 `.minecraft/` 导入本地游戏内容，包括支持的模组、配置、内容包、存档、选项和服务器列表。加载器/版本要求通过 CCL 现有安装器解析。不会执行导出的实例钩子，也不会导入账户数据。未知组件和自定义补丁会明确拒绝，避免静默生成不同实例。拒绝多个加载器。当前转换器不导入任意自定义 Java 参数、启动器专属设置或账户数据库。

## 边界与恢复

导入复用 Modrinth 导入的操作锁、暂存版本提交和文件归属冲突检查。`--replace-modified` 允许替换冲突的托管内容，使用前请备份本地数据。来源归档保持不变。读取内容前预检条目数、声明的未压缩大小、路径穿越、符号链接、大小写冲突及文件/目录冲突。限制为 100,000 条目、单条目 128 MiB、未压缩内容总量 512 MiB、清单 JSON 2 MiB。CurseForge 文件元数据响应也限制为 2 MiB；取消操作可中断等待中的元数据请求和文件下载。下载文件计入转换后整合包的大小限制。

这些导入会保留归档中的私人用户数据，例如存档和服务器列表。分享导出的 `.mrpack` 是另一项操作：导出排除存档/服务器列表，本地二进制文件重分发仍须使用 `--include-local-content`。格式转换不授予重分发许可。
