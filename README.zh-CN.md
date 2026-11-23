<div align="center">

<img src="apps/desktop/src-tauri/icons/icon.png" alt="Zibblu" width="72" height="72">

# Zibblu

**Chat freely. Share privately.**

让日常聊天、文件分享与私密交流，都更简单。

[**下载 Windows 版**](https://download.openurain.com/zibblu/windows/v0.1.10/Zibblu_0.1.10_x64_Client_Setup.exe) · [产品官网](https://zibblu.openurain.com/) · [English](README.md)

</div>

![Zibblu Windows 双栏聊天界面](assets/showcase/02-desktop-chat.png)

## 为真正重要的交流而设计

| 日常聊天 | 可靠文件分享 | 独立私密空间 |
| --- | --- | --- |
| 私聊与群聊，收到的历史消息保存在本机。 | 发送文件和文件夹，断点续传，校验下载完整性。 | 在受允许的局域网内，与已验证设备建立双方同意的加密文字直连。 |

<table>
<tr>
<td width="50%"><img src="assets/showcase/04-compact-chat.png" alt="Zibblu 窄窗口富文本聊天界面" width="100%"></td>
<td width="50%"><img src="assets/showcase/03-private-lan.png" alt="Zibblu 局域网私密聊天设备发现界面" width="100%"></td>
</tr>
<tr><td><sub>窄窗口也能舒适阅读。</sub></td><td><sub>与普通消息历史分离的局域网模式。</sub></td></tr>
</table>

### 熟悉、轻盈、可靠

- **Local-First 历史记录**：接收过的普通消息保存在设备本地 SQLite，即使离线也能浏览本机已有历史。
- **文件分享**：支持文件与文件夹传输、下载恢复和完整性校验。
- **局域网私密聊天**：双方同意后，在允许的本地网络中建立加密文字直连，不通过公网服务器中转，也没有互联网回退。
- **只读 AI 助手**：仅按单次请求的明确授权读取普通聊天消息及文件，不接触局域网私密会话内容。

> **隐私边界**：普通聊天经由所选在线服务器投递，服务器在投递期间可以访问消息及附件，**普通聊天并非端到端加密**。局域网私密聊天使用独立的设备直连通道，结束会话后清理应用管理的临时正文，但无法消除截图或操作系统痕迹。[了解安全边界](docs/SECURITY.md)。

## 下载 Zibblu

**Windows 10 / 11 · x64** — [下载安装包 v0.1.10](https://download.openurain.com/zibblu/windows/v0.1.10/Zibblu_0.1.10_x64_Client_Setup.exe) · [SHA-256 校验文件](https://download.openurain.com/zibblu/windows/v0.1.10/SHA256SUMS.txt) · [GitHub Release](https://github.com/openUrain/Zibblu/releases/tag/v0.1.10)

运行安装程序并打开 Zibblu，即可使用 **Zibblu Online** 注册或登录。如果需要连接自己的兼容服务器，在登录时选择 **Advanced** 并输入 HTTPS 地址。安装包尚未进行代码签名；设备缺少 Microsoft WebView2 时，安装程序可能需要下载它。建议根据发布的 SHA-256 文件校验安装包。

<img src="assets/showcase/01-sign-in.png" alt="Zibblu Windows 登录和账号选择界面" width="720">

## 自建服务器

使用 Docker Compose 部署 **Zibblu Online + PostgreSQL**，让团队通过自己的 HTTPS 服务完成普通消息和文件投递。

[**部署指南**](docs/ONLINE_DEPLOYMENT.md#enterprise-self-hosting) · [技术架构](docs/ARCHITECTURE.md) · [产品说明](docs/PRODUCT.md)

## 本地开发

Windows 客户端基于 **Tauri 2 / React / TypeScript**，核心逻辑与在线服务使用 **Rust**；服务端使用 PostgreSQL，客户端使用 SQLite。

```powershell
# 仓库根目录，先准备本地 PostgreSQL 数据库：
$env:DATABASE_URL = 'postgres://zibblu:<password>@127.0.0.1:5432/zibblu'
cargo run --locked -p zibblu-online
```

```powershell
# 另开终端：
cd apps/desktop
npm ci
npm run tauri dev
```

需要 Rust、Node.js 及 [Tauri 开发环境](https://v2.tauri.app/start/prerequisites/)。构建、局域网配置及移动端测试可参阅[部署文档](docs/ONLINE_DEPLOYMENT.md)、[路线图](docs/ROADMAP.md)、[工作流](.github/workflows)和[协作约定](AGENTS.md)。

---

<div align="center">
<sub>由 <a href="https://openurain.com/">openUrain</a> 开发 · <a href="docs/SECURITY.md">安全说明</a> · <a href="docs/DESIGN_SYSTEM.md">设计规范</a></sub>
</div>
