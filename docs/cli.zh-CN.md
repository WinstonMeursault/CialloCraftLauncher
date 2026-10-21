# 命令行

## 用法

```text
ccl [--json] [--game-dir PATH] [--config PATH] <doctor|init>
```

全局选项可以放在子命令前。命令以非交互方式运行，不会等待标准输入。

| 命令 | 行为 |
| --- | --- |
| `doctor` | 只读检查配置和游戏目录状态，不创建目录。 |
| `init` | 按[目录优先级](architecture.zh-CN.md#游戏目录选择)选择并创建游戏目录。 |

`--game-dir PATH` 显式指定游戏目录，优先于默认选择。`--config PATH` 指定配置文件；省略时读取平台默认位置，文件不存在则使用默认配置。显式指定的配置文件缺失时会报文件系统错误（退出码 `4`）。

`--json` 将命令执行结果输出为 JSON。clap 生成的帮助和参数解析错误仍为文本。

## 配置

JSON 配置字段为 `downloads.version_list_source`、`downloads.file_source`、`downloads.mirror_root`、`downloads.http_proxy` 和 `downloads.max_parallel`。前两项分别控制版本列表和文件来源，接受 `auto`、`official` 或 `mirror`，默认均为 `auto`。缺失字段使用默认值。镜像根地址可配置。镜像映射、版本查询、安装、下载、启动、账户管理和实例管理命令尚未实现。

## 退出码

| 码 | 含义 |
| --- | --- |
| `0` | 成功 |
| `2` | 参数错误 |
| `3` | 配置错误 |
| `4` | 文件系统错误 |
| `5` | 网络错误 |
| `130` | 操作已取消 |

脚本应使用退出码判断结果。使用 `--json` 时命令执行结果为 JSON；clap 帮助和参数解析错误为文本，并使用参数错误退出码。
