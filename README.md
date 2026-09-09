# p2p-comm

端到端加密的点对点通讯工具，支持文字、文件、语音和视频通话。

基于 [P2PCore](https://github.com/klzw2233/P2PCore) 构建。

## 特性

- 🔒 端到端加密（基于 Ed25519 身份密钥）
- 💬 文字消息（可靠传输）
- 📁 文件传输（Verified 对端自动接收，TOFU 对端弹窗确认）
- 🎤 语音通话（Opus 编解码，实时数据报传输）
- 📹 视频通话（H.264 编解码，实时数据报传输）
- 📝 加密聊天记录（密钥从身份密码派生）
- 👥 多会话并行（侧边栏联系人列表）

## 平台支持

- ✅ Linux
- ✅ Windows 10

## 快速开始

### 前置要求

- Rust 1.80+

### 编译

```bash
git clone https://github.com/klzw2233/p2p-comm.git
cd p2p-comm
cargo build --release
```

### 运行

```bash
cargo run --release -p p2p-comm-gui
```

### Windows GUI（GitHub Actions）

Actions → **Win10 GUI** → Run workflow。编完下载 artifact `p2p-comm-gui-windows`（`p2p-comm-gui.exe`）。

### 发布

推送 `v*` tag 会编 Linux / Windows release 并创建 GitHub Release：

```bash
git tag v0.1.0
git push origin v0.1.0
```

首次启动会提示输入身份密码，用于加密本地存储的身份密钥。

## 使用说明

1. **启动后输入身份密码**（首次启动会自动生成身份密钥）
2. **拨号**: 侧边栏底部输入对方 64 字符十六进制 Peer ID，或已有本地昵称
3. **本地昵称**: 聊天视图里保存 / 清除，或侧边栏右键。只存在本机，重启后仍在
4. **文字**: 连上后在输入框发送；历史加密落盘，重启后仍在
5. **文件**: 聊天视图点 "Send file" 选文件发送。入站按信任状态处理：Verified 自动接收、TOFU 弹窗确认、Untrusted（无信任记录）直接拒绝。收完按 SHA-256 校验，通过才落到系统 Downloads 目录；同名已存在则写成 `name (1).ext`，不覆盖
6. **语音**: 聊天视图点 "Voice"。被叫弹窗接受/拒绝（同意前不占麦）。Untrusted 入站邀请直接拒绝。全进程同时一路通话；任意聊天视图都能挂断当前那一路。通话期间同一 Session 仍能发文字
7. **视频**: 聊天视图点 "Video"。信令同语音（`CallInvite { media: AudioVideo }`）。同意前不打开摄像头。接通后聊天视图顶部显示对方 640×480 画面（按窗口宽度缩放），同时有语音。丢包可能导致花屏（v1 不做 FEC/NACK）
8. **通话时请使用耳机**（v1 不含回声消除；只用系统默认设备，无设备选择界面）
9. v1 使用 n0 公共 relay（hobby，无 SLA）。直连失败时音视频可能被限速
10. v1 已知限制：文件传输期间文字/信令会排在当前 64KiB 分块之后（同一条可靠流的队头阻塞，不是等整份文件）；传输中断开连接则该次传输失败、不续传

## 数据目录

- Linux: `~/.local/share/p2p-comm`
- Windows: `%APPDATA%\p2p-comm`

包含:
- 加密身份密钥
- 信任记录
- 本地昵称表
- 加密聊天记录

## 架构

```
p2p-comm-gui (eframe 前端)
    ↓
p2p-comm-core (无头核心)
    ↓
P2PCore (Session 抽象 + 信任管理)
```

详见 [CONTEXT.md](./CONTEXT.md)、[HANDOFF.md](./HANDOFF.md) 和规格 [issue #1](https://github.com/klzw2233/p2p-comm/issues/1)。

## 开发

参考 [CLAUDE.md](./CLAUDE.md) 中的工作流程。

## License

MIT OR Apache-2.0
