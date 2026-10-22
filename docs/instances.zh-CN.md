# 实例生命周期与保留版本

这些命令操作所选游戏根目录下的托管实例，使用与启动相同的稳定实例锁；运行中的实例不能被重命名、复制、删除、恢复或清理。

```sh
ccl instance copy source copy
ccl instance rename copy renamed
ccl instance history renamed
ccl instance disk-usage renamed
ccl instance restore renamed previous
ccl instance clean renamed --keep 2
ccl instance clean renamed --keep 2 --apply
ccl instance delete renamed --yes
```

复制保留当前实例、上一版本和归档版本。可变文件、存档、配置均为独立副本；已校验资源文件可能使用硬链接共享。不会复制未完成的暂存版本。重命名改变实例 ID 并更新元数据路径，移动目录前会记录恢复信息。ID 为 ASCII 字母、数字、`_` 或 `-`，最长 64 字符；拒绝仅大小写不同的重命名及不区分大小写的目标冲突。

提交保留直接上一版本供 `rollback` 使用，更早的上一版本归档至 `history/`。`history` 列出 `current`、可用的 `previous` 及归档 ID。`restore ID REVISION` 从完整保留快照创建新的当前版本，包含快照中的存档和配置，并保留原当前版本供回滚。`restore` 接受 `previous` 或 `history` 输出的归档 ID。恢复前请备份较新的存档。加载器回滚会先将当前存档、设置和其他用户数据准备到上一版本，再执行交换；若操作中断，恢复流程会保留一致的当前版本和回滚版本。完整内容编辑或整合包更新后的回滚会交换完整快照。

`disk-usage` 显示当前、上一版本、历史版本及未完成暂存数据的逻辑字节数。由于硬链接，这些数字不代表独占物理磁盘占用，也不保证能释放对应空间。

`clean` 默认预览目标。`--keep N` 保留最新 N 个归档版本，默认 2；清理也会针对未完成暂存版本。`--include-previous` 显式纳入直接回滚版本。`--apply` 在持锁状态下执行计划。当前存档和内容永远不是清理目标。清理不扫描共享根目录的库/资源、其他启动器文件、账户凭据或当前实例中已安装的 Java 运行时。

`delete --yes` 永久移除整个指定实例，包括存档和全部历史，请先备份。它不会删除其他实例和共享根目录资源。实例目录外的稳定锁文件会有意保留：删除锁文件可能导致两个进程绕过彼此的锁。
