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
- 配置 `P2PCORE_TOKEN` 环境变量（GitHub Personal Access Token，用于拉取私有 P2PCore 依赖）

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

首次启动会提示输入身份密码，用于加密本地存储的身份密钥。

## 使用说明

1. **启动后输入身份密码**（首次启动会自动生成身份密钥）
2. **添加联系人**: 需要对方的 Peer ID（64 字符十六进制字符串）
3. **拨号连接**: 选择联系人，点击连接
4. **通话时请使用耳机**（v1 不含回声消除）
5. v1 使用 n0 公共 relay（hobby，无 SLA）。直连失败时音视频可能被限速

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
