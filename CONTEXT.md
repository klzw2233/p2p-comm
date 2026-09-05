# CONTEXT.md

## 术语表

- **Session**: P2PCore 提供的一对一连接抽象，封装 QUIC 连接 + 一条可靠双向流 + 数据报能力
- **Peer**: 一个 Identity Key 对应的远端实体；一台设备一把 Identity Key（ADR-0003）
- **本地昵称**: 用户给远端 Peer 起的名字，存本地 JSON，不上线传输
- **信任状态**: Verified（已验证）/ TOFU（首次信任）/ Untrusted（不信任），由 P2PCore 的 TrustStore 管理
- **信令**: 通话邀请/接受/拒绝/结束等控制消息，走可靠流，复用 p2p-chat ADR-0001 帧格式
- **媒体流**: 音视频实时数据，走 QUIC 数据报（不可靠传输）
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
│      P2PCore        │  p2p-core + p2p-trust (git 依赖 feat/datagram-api 分支)
└─────────────────────┘
```

## 范围

**v1 包含**:
- 文字消息（可靠流，帧格式同 p2p-chat ADR-0001）
- 文件传输（可靠流，Verified 自动接收 / TOFU 弹窗）
- 语音通话（Opus 编解码，数据报传输）
- 视频通话（H.264/openh264 编解码，数据报传输）
- 本地昵称表（JSON 存储）
- 加密聊天记录（JSONL，密钥从身份密码派生）
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

## 依赖

- **P2PCore**: Session 抽象、信任管理（git 依赖，datagram 分支）
- **eframe/egui**: GUI 框架
- **nokhwa**: 跨平台相机采集（Linux V4L2 + Windows MSMF）
- **openh264**: H.264 编解码
- **opus**: Opus 音频编解码
- **cpal**: 音频采集/播放
- **dirs**: 平台标准数据目录
- **serde/serde_json**: 本地昵称表、聊天记录序列化

## 验证策略

- CI: GitHub Actions `ubuntu-latest` + `windows-latest` 矩阵，依赖 `P2PCORE_TOKEN` 拉取私有依赖
- CI 保证: `cargo build` / `cargo test`（不含真实设备的部分）编译通过
- 真实设备测试: 朋友异机验证（同机 Win10 宿主 + Ubuntu VM 抢设备不可靠）
