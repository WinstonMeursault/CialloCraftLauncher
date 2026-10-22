# Microsoft 账户设置

> **预览状态：** Microsoft 登录已经实现，但尚未使用真实账户验证。CCL 的 OAuth 应用尚未注册。必须使用该应用通过完整的 Microsoft、Xbox 和 Minecraft 登录流程，才能满足 CLI 预览版的账户验收要求。

## 注册公开桌面应用

启动器使用 Microsoft 设备授权流程。它是公开桌面客户端，不能使用客户端密钥。Microsoft 的[桌面应用注册指南](https://learn.microsoft.com/en-us/entra/identity-platform/scenario-desktop-app-registration)和[设备代码流程文档](https://learn.microsoft.com/en-us/entra/identity-platform/v2-oauth2-device-code)介绍了桌面应用注册与公开客户端配置。

1. 使用有权创建应用注册的账户登录 [Microsoft Entra 管理中心](https://entra.microsoft.com/)。
2. 打开 **标识 → 应用程序 → 应用注册 → 新注册**。
3. 为应用填写清楚的启动器名称。当前实现使用 `/consumers` 权限机构，因此账户类型请选择 **仅个人 Microsoft 账户**。Minecraft Java 版持有者使用个人 Microsoft 账户。如果门户没有此选项，请先确认注册方案，不要直接改用更宽泛的账户类型。
4. 注册应用并复制 **应用程序（客户端）ID**。这个 GUID 是公开配置，不是密码。
5. 打开 **身份验证**。在 **高级设置** 中将 **允许公共客户端流** 设为 **是** 并保存。设备代码客户端不使用客户端密钥或浏览器重定向 URI。
6. 不要创建客户端密钥，也不要把访问令牌或刷新令牌放入启动器设置。
7. 确认你的应用当前需要满足的 Xbox 和 Minecraft 服务访问要求。完成注册本身不能证明完整登录流程可用。如果服务拒绝该应用，请根据错误和该服务的官方支持流程解决访问问题。

Microsoft 的[应用注册快速入门](https://learn.microsoft.com/en-us/entra/identity-platform/quickstart-register-app)说明了账户类型；桌面应用指南说明何时应将桌面客户端设为公开客户端，以及如何启用公开客户端流。本项目尚未验证当前应用注册和 API 访问是否已获准用于公众。

## 配置并登录

将客户端 ID 保存到启动器的用户配置中：

```sh
ccl settings set auth.microsoft_client_id YOUR-APPLICATION-CLIENT-ID
ccl account check-storage
ccl account login
```

`account login` 会先测试凭据库；若无法安全保存凭据，会在请求设备代码和联系 Microsoft 之前停止。`account check-storage` 可随时执行相同的写入、读取和删除探测；它只使用临时测试值，不会修改已保存账户。

也可以仅为本次登录传入 ID：

```sh
ccl account login --client-id YOUR-APPLICATION-CLIENT-ID
```

启动器会显示 Microsoft 验证网址和一次性设备代码。请在浏览器中打开该网址，登录并输入代码。不要分享设备代码；任何先输入该代码的人都可能授权正在等待的登录。CLI 会等待授权完成。之后可以使用：

```sh
ccl account list
ccl account select ACCOUNT-ID
ccl launch VERSION
```

选定账户后，`launch` 默认使用该账户，并在需要时刷新凭据。离线启动可传入 `--offline` 或 `--username NAME`。`--account ACCOUNT-ID` 可为单次启动指定已保存的账户。`account logout [ACCOUNT-ID]` 会删除本地凭据和账户元数据，但不会撤销 Microsoft 授权。

## 凭据存储

访问令牌和刷新令牌仅通过操作系统凭据库保存。账户名称和 ID 分开存储在 `accounts.json` 中，该元数据文件不包含令牌。macOS 和 Windows 使用系统凭据库。Linux 必须安装并解锁可通过当前 D-Bus 会话访问的 Secret Service。没有持久 Secret Service 的无头系统不能使用 Microsoft 登录；程序不会将凭据退回保存为明文。只要为会话提供并解锁持久的 Secret Service，无需图形桌面。

如果安全存储不可用，登录或刷新会报错。请启动带有已解锁密钥环的桌面会话；在安全存储可用前，可使用离线启动。不要通过把令牌复制到文件或 shell 命令中来绕过限制。

## 应用所有者的验证清单

将账户登录称为可供公众使用的预览功能前，请记录以下验证证据：

- 已注册的公开客户端 ID 与启动器配置一致，并已完成 Microsoft 要求的审核；
- 设备代码登录可用于拥有 Minecraft Java 版权益的账户；
- 拒绝、过期、取消、刷新、退出和凭据库不可用等路径均符合预期；
- CLI 输出、日志、错误文本、普通配置和账户元数据中均没有令牌；
- Linux 版本在真实且持久的 Secret Service/D-Bus 会话中可用；
- 服务/API 权限及分发方式符合当前 Microsoft、Xbox 和 Minecraft 要求。

这些检查完成前，必须将身份验证功能和预览包标注为**未经验证，不适合公开分发**。
