# CONTEXT.md

## 术语表

- **Session**: P2PCore 提供的一对一连接抽象，封装 QUIC 连接 + **一条**可靠双向流 + 数据报能力。v1 不另开第二条可靠流。
- **Peer**: 一个 Identity Key 对应的远端实体；一台设备一把 Identity Key
- **本地昵称**: 用户给远端 Peer 起的名字，存本地 JSON，不上线传输
- **信任状态**: Verified（已验证）/ TOFU（首次信任）/ Untrusted（不信任），由 P2PCore 的 TrustStore 管理
- **信令**: 通话邀请/接受/拒绝/结束等控制消息，走可靠流，复用 p2p-chat ADR-0001 帧格式
- **媒体流**: 音视频实时数据。音频走 QUIC 数据报（不可靠）；视频走数据报但 NAL 必须切到 path MTU 以内（见 issue #1）
- **数据目录**: 存放身份密钥、信任记录、本地昵称表、加密聊天记录的目录
  - Linux: `~/.local/share/p2p-comm`
  - Windows: `%APPDATA%\p2p-comm`

## 架构

```
┌─────────────────────┐
│   p2p-comm-gui      │  eframe 前端，启动密码输入、多会话 UI
└──────────┬──────────┘
           │
┌──────────▼──────────┐
│  p2p-comm-core      │  无头核心：Session 管理、消息/文件/通话逻辑
└──────────┬──────────┘
           │
┌──────────▼──────────┐
│      P2PCore        │  p2p-core + p2p-trust (git 依赖 main)
└─────────────────────┘
```

权威规格: GitHub issue #1。`docs/spec-v1.md` 是同步副本。栈评估: `notes/2026-09-06-stack-architecture-review.md`。

## 范围

**v1 包含**:
- 文字消息（可靠流，帧格式同 p2p-chat ADR-0001）
- 文件传输（同一条可靠流；Verified 自动接收 / TOFU 弹窗 / Untrusted 拒绝）。传文件期间文字/信令会排队，这是 v1 已知限制
- 语音通话（Opus 编解码，数据报传输）
- 视频通话（H.264/openh264，数据报；slice 限制在 `Session::max_datagram_size()` 减去 9 字节头以内；丢包花屏可接受）
- 本地昵称表（JSON 存储）
- 加密聊天记录（JSONL；Argon2id 派生主密钥，再 HKDF 按 Peer 分密钥，ChaCha20-Poly1305）
- 多会话 UI（侧边栏昵称列表，并行聊天）
- 平台: Linux + Windows 10

**范围外**:
- 移动端（安卓/iOS）
- 回声消除（文档写明"请用耳机"）
- 设备选择（v1 只抓系统默认设备）
- 断点续传
- 多设备"同一个人"聚合展示
- 线上交换昵称/头像
- 群聊
- 离线消息投递
- 第二条 QUIC 可靠流（P2PCore Session 目前只暴露一条）
- 钥匙串 / 系统密码管理器
- 自建 relay（v1 显式 opt-in n0 公共 relay，hobby 无 SLA）

## 依赖

- **P2PCore**: Session 抽象、信任管理、身份密码封装（git 依赖 `main`）
- **eframe/egui**: 0.30（不跟 0.36：MSRV 不够）
- **nokhwa**: 0.10，`input-v4l` + `input-msmf`
- **openh264**: 0.9
- **opus**: 0.4
- **cpal**: 0.18（系统默认设备）
- **dirs**: 6.0
- **serde/serde_json**: 本地昵称表、聊天记录序列化
- **argon2 / hkdf / chacha20poly1305**: 聊天记录密钥（身份封装走 P2PCore `FileKeyStore`）

## 验证策略

- CI: GitHub Actions `ubuntu-latest` + `windows-latest` 矩阵，依赖 `P2PCORE_TOKEN` 拉取私有依赖
- 发布: 推送 `v*` tag 触发 Linux + Windows GUI `--release`，产物挂 GitHub Release
- CI 保证: `cargo build` / `cargo test`（不含真实设备的部分）编译通过
- 测试只挂 `p2p-comm-core`（假 Session 或进程内双端）
- 真实设备测试: 朋友异机验证（同机 Win10 宿主 + Ubuntu VM 抢设备不可靠）
