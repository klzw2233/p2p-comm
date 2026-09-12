# CONTEXT.md

## 术语表

- **Session**: P2PCore 提供的一对一连接抽象,封装 QUIC 连接 + **一条**可靠双向流 + 数据报能力。v1 不另开第二条可靠流。
- **Peer**: 一个 Identity Key 对应的远端实体；一台设备一把 Identity Key
- **本地昵称**: 用户给远端 Peer 起的名字，存本地 JSON，不上线传输
- **信任状态**: Verified（已验证）/ TOFU（首次信任）/ Unknown（不信任，等价于 spec 中的"Untrusted"），由 P2PCore 的 TrustStore 管理。P2PCore 的 `TrustState` 枚举只有三个值：`Verified`、`Tofu`、`Unknown`，其中 `Unknown` 对应 spec 文档中提到的"Untrusted"语义
- **信令**: 通话邀请/接受/拒绝/结束等控制消息，走可靠流，复用 p2p-chat ADR-0001 帧格式
- **媒体流**: 音视频实时数据。音频走 QUIC 数据报（不可靠）；视频走数据报但 NAL 必须切到 path MTU 以内（见 issue #1）
- **数据目录**: 存放身份密钥、信任记录、本地昵称表、加密聊天记录、设置、日志的目录
  - Linux: `~/.local/share/p2p-comm`
  - Windows: `%APPDATA%\p2p-comm`
  - `--profile NAME`（同机第二身份）: 平台数据目录下的 `p2p-comm-NAME`
  - macOS（spec #42，未实现）: `~/Library/Application Support/p2p-comm`
- **未接来电**: 入站邀请在被叫未 Accept/Reject 的情况下结束（主叫取消、90s 超时、振铃时断线）。聊天里瞬时红字，不进 JSONL。info 日志记一行。

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

权威规格: GitHub issue #1。`docs/spec-v1.md` 是同步副本。macOS：增量 spec [issue #42](https://github.com/klzw2233/p2p-comm/issues/42)，本地 [`docs/spec-macos.md`](docs/spec-macos.md)，见 [ADR-0002](docs/adr/0002-macos-client.md)。CLI/日志：[issue #48](https://github.com/klzw2233/p2p-comm/issues/48) / [`docs/spec-cli-flags.md`](docs/spec-cli-flags.md)，[ADR-0003](docs/adr/0003-optional-custom-relay.md)。注意力/设置：[issue #49](https://github.com/klzw2233/p2p-comm/issues/49) / [`docs/spec-ux-attention.md`](docs/spec-ux-attention.md)。#41 已关。ADR-0001 已 superseded。栈评估: `notes/2026-09-06-stack-architecture-review.md`。

## 范围

**v1 包含**:
- 文字消息（可靠流，帧格式同 p2p-chat ADR-0001）
- 文件传输（同一条可靠流；Verified 自动接收 / TOFU 弹窗 / Untrusted 拒绝）。传文件期间文字/信令会等当前 64KiB FileChunk 写完，这是 v1 已知限制
- 语音通话（Opus 编解码，数据报传输）
- 视频通话（H.264/openh264，数据报；slice 限制在 `Session::max_datagram_size()` 减去 9 字节头以内；丢包花屏可接受）
- 本地昵称表（JSON 存储）
- 加密聊天记录（JSONL；Argon2id 派生主密钥，再 HKDF 按 Peer 分密钥，ChaCha20-Poly1305）
- 多会话 UI（侧边栏昵称列表，并行聊天）
- 平台: Linux + Windows 10。macOS 增量 spec 见 issue #42 / ADR-0002 / `docs/spec-macos.md`（未实现；#41 已关）
- CLI: `--profile`；可选 `--relay` / `--no-relay` / `--debug`（spec 未实现）
- 默认 info 文件日志 `{data_dir}/logs/`（spec 未实现）
- 后台注意力：提示音 + 任务栏闪烁；设置页两个总开关（spec 未实现）

**范围外**:
- macOS 客户端实现（spec 在 #42；#41 已关，尚未写代码；不是架构限制）
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
- 设置页配置 relay / 热切换 relay（CLI `--relay` / `--no-relay` 见 spec-cli-flags；默认仍 n0）
- 托盘、消息气泡、Dock 徽章、Mac 通知中心
- 联系人页 / 搜索 / 按加入时间排序
- 信任验证 UI / SAS / 入站 Session 同意门（第一次连上仍是 TOFU；Verified 目前 GUI 走不到）
- 改密（须连聊天盐一起迁）

## 依赖

- **P2PCore**: Session 抽象、信任管理、身份密码封装、`RelayConfig::{n0_public, custom, disabled}`（git 依赖 `main`）
- **eframe/egui**: 0.30（不跟 0.36：MSRV 不够）
- **nokhwa**: 0.10，`input-v4l` + `input-msmf`（macOS `input-avfoundation` 属 #42，未实现）
- **openh264**: 0.9
- **opus**: 0.4
- **cpal**: 0.18（系统默认设备）
- **dirs**: 6.0
- **serde/serde_json**: 本地昵称表、聊天记录序列化
- **argon2 / hkdf / chacha20poly1305**: 聊天记录密钥（身份封装走 P2PCore `FileKeyStore`）

## 验证策略

- CI: GitHub Actions `ubuntu-latest` + `windows-latest` 矩阵（`macos-latest` 属 #42，未实现）
- 发布: 推送 `v*` tag 触发 Linux + Windows GUI `--release`，产物挂 GitHub Release（Mac `.app` zip 属 #42）
- CI 保证: `cargo build` / `cargo test`（不含真实设备的部分）编译通过
- 测试只挂 `p2p-comm-core`（假 Session 或进程内双端）
- 真实设备测试: 朋友异机验证（issue #41 已关；同机 Win10 宿主 + Ubuntu VM 抢设备不可靠）
