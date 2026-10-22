# Modrinth 整合包与实例内容

CCL 可检查、导入和导出 Modrinth .mrpack v1 归档（含 Quilt 依赖），并管理独立实例内容。CurseForge、MultiMC/Prism 导出通过[其他格式转换器](imports.zh-CN.md)导入。目前不提供 GUI。

已验证样本使用 Minecraft `1.21.1`、Fabric Loader `0.20.1`、Fabric API `0.116.17+1.21.1`，在 macOS arm64 及使用 CPU 渲染的 Linux x86_64 到达主菜单。macOS 上导出后重新导入的实例也到达主菜单，模组哈希与覆盖文件均匹配。这不代表其他整合包或第三方模组组合已认证。Windows 通过编译和自动化测试检查。

## 检查和导入整合包

```sh
ccl pack inspect ./pack.mrpack
ccl pack import my-pack ./pack.mrpack
ccl pack show my-pack
```

`inspect` 会验证归档并输出清单，不会安装。`show` 会显示已导入实例记录的清单和文件归属信息。导入支持整合包声明的 Minecraft、无加载器、Fabric、Quilt、Forge 或 NeoForge。默认跳过可选客户端文件；如需包含，请使用：

```sh
ccl pack import my-pack ./pack.mrpack --include-optional
```

将另一个归档导入已有托管 ID 会更新该实例。CCL 会检查托管整合包文件是否仍与记录的哈希一致。如果本地托管文件已修改或删除，或新整合包路径与未托管文件冲突，更新会以退出码 `7` 停止。请先备份本地内容；若明确放弃这些本地改动，可以使用：

```sh
ccl pack import my-pack ./pack-v2.mrpack --replace-modified
```

`--replace-modified` 允许更新按需替换或删除冲突的本地文件。请将其视为允许丢弃这些本地改动。

## 下载来源与完整性

整合包文件 URL 必须使用 HTTPS。默认允许 Modrinth CDN、GitHub 和 GitLab 主机，并允许 GitHub 发布资产重定向主机。每次重定向都会按同一 HTTPS 主机策略重新检查，最多跟随 10 次。若整合包使用其他来源，可添加明确受信任的完整主机名：

```sh
ccl pack import my-pack ./pack.mrpack --allow-host downloads.example.org
```

可重复使用 `--allow-host` 添加多个主机。CCL 在文件放入实例前会验证 SHA-1、SHA-512 和声明的文件大小。`.mrpack` 归档最多包含 100,000 个条目；单个文件最大 128 MiB，未压缩数据总量最大 512 MiB。

## 导出整合包

```sh
ccl pack export my-pack ./my-pack.mrpack
```

默认导出会保留已验证的远程文件引用，并将本地配置和游戏选项作为覆盖文件加入归档；不会上传任何内容。已修改的文件无法继续用旧远程引用表示。新增或未跟踪的模组、资源包和光影包二进制文件必须明确启用 `--include-local-content` 才会导出：

导出也支持通过 `instance install` 创建的普通托管实例，包括原版实例。Minecraft 与加载器依赖从已安装实例元数据中取得。没有整合包记录的实例不具备固定远程引用，因此导出其本地二进制文件同样需要明确启用许可选项。

```sh
ccl pack export my-pack ./my-pack.mrpack --include-local-content
```

只有在你有权重新分发所有纳入的二进制文件时，才使用该选项。CCL 不会替你检查许可证或取得授权。分享归档前，请阅读 Modrinth 的[`.mrpack` 格式规范](https://support.modrinth.com/en/articles/8802351-modrinth-modpack-format-mrpack)和[整合包权限指南](https://support.modrinth.com/en/articles/8797527-obtaining-modpack-permissions)。CCL 不会发布或上传导出的文件。

存档和 `servers.dat` 不会导出到整合包；请使用用户数据备份保存它们。

## 管理本地内容

```sh
ccl content list my-pack mods
ccl content list my-pack resourcepacks
ccl content list my-pack shaderpacks
ccl content list my-pack saves
ccl content add my-pack mods ./local-mod.jar
ccl content add my-pack saves ./world --name MyWorld
ccl content remove my-pack mods local-mod.jar
ccl content enable my-pack example.jar
ccl content disable my-pack example.jar
```

`content add` 会复制来源文件，保留原文件不动。模组必须是 `.jar` 或 `.jar.disabled` 文件；资源包和光影包必须是 `.zip` 文件；存档必须是目录。`--name` 为可选参数，用于指定目标名称。启用和禁用操作仅适用于 `.jar` 模组。

## 备份和恢复用户数据

```sh
ccl content backup my-pack ./my-pack-userdata.zip
ccl content restore my-pack ./my-pack-userdata.zip
```

备份包含 `mods`、`config`、`resourcepacks`、`shaderpacks`、`saves`、`defaultconfigs`、`datapacks`、`options.txt` 和 `servers.dat` 中的用户数据。备份不包含 Minecraft 或加载器文件、Java 运行时或账号凭据。恢复会替换归档声明的每个根目录或文件；备份中标记为缺失的根也会从实例中删除。如果还需要当前文件，请先备份再恢复。备份最多包含 100,000 个条目，文件数据总量上限为 2 GiB。

## 版本快照与正在运行的实例

整合包导入/更新和内容编辑会先在暂存的实例版本中准备，再通过实例操作锁提交。托管游戏正在持有该锁运行时，这些操作无法替换游戏目录。整合包或内容变更后执行 `instance rollback` 会恢复完整的上一版快照，包括其中的用户数据和存档。如果需要保留较新的存档，请先备份。加载器回滚会切换加载器版本，同时保留当前用户数据。

错误处理见 [CLI 退出码](cli.zh-CN.md#退出码)，这些命令支持 `--json`。标准 Modrinth 依赖不能表示独立 OptiFine 或 Legacy Fabric；这些托管实例应使用用户数据备份，不生成会误导可移植性的导出。
