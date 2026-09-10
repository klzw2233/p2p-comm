# p2p-comm 架构文档

## 概述

p2p-comm 是一个端到端加密的点对点通讯工具，支持文字消息、文件传输、实时语音/视频通话。基于 P2PCore 构建，采用 QUIC 传输协议，运行在 Linux 和 Windows 10 平台。

## 架构原则

1. **关注点分离**: 无头核心 (p2p-comm-core) 与 GUI (p2p-comm-gui) 严格分离
2. **单一数据流**: 所有业务逻辑在 core，GUI 只做展示和输入
3. **测试优先**: 核心逻辑零 GUI 依赖，集成测试可在无显示器的 CI 环境运行
4. **范围约束**: 严格遵守 CONTEXT.md 定义的功能边界

## 系统架构

```
┌─────────────────────────────────────────────────────────────┐
│                      p2p-comm-gui                           │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐     │
│  │ 密码输入界面  │  │ 侧边栏 UI     │  │ 聊天视图 UI   │     │
│  └──────┬───────┘  └──────┬───────┘  └──────┬───────┘     │
│         │                 │                 │              │
│         └─────────────────┼─────────────────┘              │
│                           │                                │
│                      Node API                              │
└───────────────────────────┼────────────────────────────────┘
                            │
┌───────────────────────────▼────────────────────────────────┐
│                   p2p-comm-core                            │
│  ┌──────────────────────────────────────────────────────┐ │
│  │                     Node                             │ │
│  │  - Session 管理 (per-peer)                           │ │
│  │  - 状态机 (Connecting/Connected/Failed)              │ │
│  │  - 消息路由                                          │ │
│  │  - 文件传输状态                                       │ │
│  │  - 通话状态 (全进程单路)                             │ │
│  └──────────────────────────────────────────────────────┘ │
│                                                            │
│  ┌──────────┐  ┌──────────┐  ┌──────────┐  ┌──────────┐ │
│  │ Frame    │  │ Audio    │  │ Video    │  │ Storage  │ │
│  │ 帧编解码  │  │ Opus编解  │  │ H.264编解 │  │ 加密落盘  │ │
│  └──────────┘  └──────────┘  └──────────┘  └──────────┘ │
└───────────────────────────┬────────────────────────────────┘
                            │
┌───────────────────────────▼────────────────────────────────┐
│                      P2PCore                               │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐    │
│  │ Session      │  │ TrustStore   │  │ FileKeyStore │    │
│  │ QUIC连接抽象  │  │ 信任管理     │  │ 身份密钥封装  │    │
│  └──────────────┘  └──────────────┘  └──────────────┘    │
└────────────────────────────────────────────────────────────┘
```

## 核心组件

### 1. p2p-comm-gui (eframe 前端)

**职责**: 
- 用户输入采集 (密码、文字、文件选择、按钮点击)
- 状态展示 (消息列表、进度条、视频画面、错误提示)
- 调用 Node API 驱动业务逻辑

**约束**:
- 不包含业务逻辑
- 不直接操作 P2PCore
- 单文件 `main.rs` (截至 v1)

**关键交互**:
```rust
// 启动流程
let node = Node::new(password, data_dir)?;

// 拨号
node.dial(peer_id_hex)?;

// 发送文字
node.send_text(peer_id_hex, content)?;

// 轮询事件
while let Some(event) = node.poll()? {
    match event {
        Event::TextReceived { from, content, .. } => /* 更新 UI */,
        Event::CallInvite { from, media } => /* 弹窗 */,
        // ...
    }
}

// 快照状态
let snapshot = node.snapshot();
```

### 2. p2p-comm-core (无头核心)

#### 2.1 Node (node.rs)

**职责**: 
- Session 生命周期管理 (每个远端 Peer 最多一个 Session)
- 多会话并行协调
- 消息帧的编码/解码/路由
- 文件传输状态机 (offer → accept/reject → chunks → verify)
- 通话状态机 (invite → accept/reject → media → end)
- 全进程单路通话约束
- 事件队列 (poll 模型)

**状态维护**:
```rust
struct Node {
    live: HashMap<String, LivePeer>,        // peer_id_hex → Session + 状态
    transfers: HashMap<String, Transfer>,   // peer_id_hex → 文件传输状态
    call: Option<CallState>,                // 全进程唯一通话
    dgram_tx: HashMap<String, ...>,         // 数据报发送通道
    nicknames: Nicknames,                   // 本地昵称表
    chatlog: ChatLog,                       // 加密聊天记录
    roster: Roster,                         // 已知 Peer 列表
    inbox: Inbox,                           // 待确认文件 offer
    // ...
}
```

**关键不变量**:
- 每个 `peer_id_hex` 最多对应一个 `LivePeer::Connected` 或 `Connecting`
- `call.is_some()` 时，任何新通话邀请失败 (全进程单路)
- 断开的 Session 状态变为 `Failed`，允许重拨

#### 2.2 Frame (frame.rs)

**职责**: 可靠流帧的编码/解码

**格式** (与 p2p-chat ADR-0001 对齐):
```
[4 字节 little-endian 长度][UTF-8 JSON payload]
```

**消息变体**:
```rust
enum Message {
    Text { content: String, timestamp: u64 },
    FileOffer { name: String, size: u64, hash: String },
    FileAccept,
    FileReject,
    FileChunk { offset: u64, data: String },  // base64
    CallInvite { media: MediaType },          // Audio | AudioVideo
    CallAccept,
    CallReject,
    CallEnd,
}
```

**健壮性**:
- `#[serde(other)] Unknown` 处理未知 JSON 变体，不崩溃

#### 2.3 Audio (audio.rs)

**职责**: 
- Opus 编解码
- 音频数据报格式

**数据报格式**:
```
[0x01][u64 BE 时间戳][Opus payload]
```

**参数**:
- 采样率: 48kHz
- 声道: 单声道
- 帧长: 20ms

#### 2.4 Video (video.rs)

**职责**: 
- H.264 编解码 (openh264)
- 视频数据报格式
- NAL 切片限制

**数据报格式**:
```
[0x02][u64 BE 时间戳][一个 H.264 NAL]
```

**约束**:
- `max_slice_len = Session::max_datagram_size() - 9`
- 超限 NAL 丢弃 (不应用层重组)
- 丢包花屏是 v1 可接受行为

**参数**:
- 分辨率: 640×480
- 帧率: 15fps

#### 2.5 Audio I/O (audio_io.rs)

**职责**:
- cpal 默认麦克风/扬声器抽象
- PCM 缓冲区管理

**约束**:
- 回调线程只拷贝 PCM，不编码/不发网

#### 2.6 Video I/O (video_io.rs)

**职责**:
- nokhwa 默认摄像头抽象
- RGB → I420 转换

**约束**:
- 挂断时停止采集并释放设备 (指示灯应灭)

#### 2.7 ChatLog (chatlog.rs)

**职责**: 加密聊天记录落盘/加载

**存储格式**:
- 每个 Peer 一个 `{peer_id_hex}.jsonl`
- 每行: `[随机 nonce][ciphertext + tag]`

**密钥派生**:
```
独立随机 32 B salt (存数据目录)
    ↓
master = Argon2id(password, salt)
    ↓
per_peer_key = HKDF-Expand(master, info=peer_id)
    ↓
ChaCha20-Poly1305
```

**与身份密钥分离**: 身份走 P2PCore `FileKeyStore`，聊天记录走上述独立派生。

#### 2.8 Nicknames (nicknames.rs)

**职责**: 本地昵称表管理

**存储**: `nicknames.json` → `{"<peer_id_hex>": "<nickname>"}`

**约束**: 
- 不上线传输
- 变更立即写回
- 未设昵称显示 Peer ID 前 8 字符

#### 2.9 Roster (roster.rs)

**职责**: 已知 Peer 列表维护

#### 2.10 Inbox (inbox.rs)

**职责**: 待确认文件 offer 队列 (TOFU peer)

### 3. P2PCore (git 依赖)

**提供**:
- `Session`: QUIC 连接 + 一条可靠双向流 + 数据报能力
- `TrustStore`: 信任状态管理 (Verified / TOFU / Unknown)
- `FileKeyStore`: 身份密钥加密存储 (Argon2id + ChaCha20-Poly1305)
- `RelayConfig`: Relay 服务器配置 (v1 用 `n0_public()`)

## 数据流

### 文字消息发送
```
GUI 输入框
  → node.send_text(peer_id, content)
    → encode_text_frame(content, timestamp)
      → session.send_reliable(frame_bytes)
        → QUIC 可靠流
```

### 文字消息接收
```
QUIC 可靠流
  → session.recv_reliable()
    → decode_frame(bytes)
      → Message::Text { content, timestamp }
        → Event::TextReceived 入队
          → node.poll() 返回给 GUI
            → GUI 更新聊天视图
```

### 文件传输 (发送方)
```
GUI 文件选择
  → node.send_file(peer_id, path)
    → 计算 SHA-256
      → encode_file_offer(name, size, hash)
        → session.send_reliable(offer_frame)
          → 等待 FileAccept/FileReject
            → (accept) 按 64KiB 分块
              → enqueue_next_chunk() (一次只排队一块)
                → encode_file_chunk(offset, base64_data)
                  → session.send_reliable(chunk_frame)
                    → 块写出后 enqueue_next_chunk() 下一块
                      → 循环直到全部发完
                        → Transfer::Complete
```

### 文件传输 (接收方)
```
收到 FileOffer
  → 检查 TrustState
    → Verified: 自动 node.accept_file(peer_id)
    → TOFU: Event::FileOffered 入队 → GUI 弹窗
    → Unknown: 自动发 FileReject
  → (accept) 等待 FileChunk
    → 收到 chunk: 解码 base64, 写入 buffer
      → 循环直到 size 字节全收
        → 计算 SHA-256
          → 校验成功: 写入 unique_download_path()
          → 校验失败: Transfer::Failed, 不落盘
```

### 语音通话
```
GUI 点语音按钮
  → node.invite_audio(peer_id)
    → encode_call_invite(Audio)
      → session.send_reliable(invite_frame)
        → 等待 CallAccept/CallReject
          → (accept) 启动音频采集/播放线程
            → 捕获: cpal 麦克风 → Opus 编码 → [0x01][时间戳][opus] → session.send_datagram()
            → 播放: session.recv_datagram() → [0x01][时间戳][opus] → Opus 解码 → cpal 扬声器
              → 点挂断: node.hangup()
                → encode_call_end()
                  → session.send_reliable(end_frame)
                    → 停止采集/播放线程
```

### 视频通话
```
(同语音通话，media=AudioVideo)
  → 视频捕获: nokhwa 相机 → RGB→I420 → OpenH264 编码 → 按 NAL 分包
    → 每个 NAL: [0x02][时间戳][nal_bytes] → session.send_datagram()
  → 视频播放: session.recv_datagram() → [0x02][时间戳][nal] → OpenH264 解码 → I420
    → 复用 egui TextureHandle 每帧更新
```

## 并发模型

- **GUI 线程**: eframe 主循环，轮询 `node.poll()` 拉取事件
- **I/O 线程**: P2PCore 内部管理 QUIC 网络 I/O
- **音频回调线程**: cpal 控制，只拷贝 PCM 到/从缓冲区
- **视频采集线程**: node 内部，调用 nokhwa + openh264
- **视频播放线程**: node 内部，调用 openh264 解码

**同步原则**:
- GUI 通过 `node.poll()` 无锁拉取事件 (内部用 `mpsc::UnboundedReceiver`)
- 音频线程通过 `Arc<Mutex<RingBuffer>>` 与编解码线程同步
- 视频线程持有 `Session` 的 `dgram_tx` 克隆，可并发发送

## 安全模型

### 身份与信任

- **身份密钥**: P2PCore `FileKeyStore` 派生，Argon2id + 随机 16 B salt + ChaCha20-Poly1305
- **信任状态**: 
  - `Verified`: 已验证，文件自动接收
  - `TOFU`: 首次信任，文件需确认
  - `Unknown`: 不信任，文件自动拒绝

### 聊天记录加密

- **密钥派生**: 独立 32 B salt → Argon2id(password, salt) → master → HKDF-Expand(info=peer_id)
- **AEAD**: ChaCha20-Poly1305，每条消息独立随机 nonce
- **磁盘**: 仅密文 JSONL，明文禁止

### 传输安全

- **QUIC TLS 1.3**: P2PCore 提供端到端加密
- **文件完整性**: SHA-256 校验，失败不落盘
- **路径穿越**: `safe_filename()` 去除 `/` `\\` `..`

## 已知约束与权衡

### 文件 HOL (Head-of-Line Blocking)

**现状**: FileChunk 与 Text/Call 信令共用 Session 唯一可靠流。大文件期间，文字/挂断等当前 64KiB chunk 写完。

**权衡**: v1 接受此限制。等 P2PCore 暴露第二条可靠流后可优化。

### 视频丢包

**现状**: H.264 NAL 走 DATAGRAM (不可靠)。丢包导致花屏/冻结。

**权衡**: v1 可接受。升级路径：一帧一条 QUIC 单向流，过时 reset (MoQ 风格)。

### 回声消除

**现状**: 不做。README 写明"请用耳机"。

**权衡**: v1 简化音频处理。

### 设备选择

**现状**: 只抓系统默认麦克风/扬声器/摄像头。

**权衡**: v1 简化 UI。

### Relay 依赖

**现状**: `RelayConfig::n0_public()`，n0 公共 relay 是 hobby (无 SLA、有限速)。

**权衡**: 直连失败时视频会卡。生产需自建 relay。

### 平台支持

**现状**: Linux + Windows 10。macOS 见 ADR-0001 推迟。

**权衡**: 先验证核心流程，macOS 需解决 nokhwa AVFoundation 适配。

## 测试策略

### 集成测试 (p2p-comm-core)

**seam**: Node 公开 API

**运输层**: 假 Session (test_node + 字节 sink + push_incoming_bytes + poll + snapshot)

**覆盖**:
- 身份密码: 设置/解锁/错密码/空密码
- 昵称: 增改删/重启仍在
- 帧: 编解码/未知变体不崩
- 文字: 双端收发/失败路径
- 文件: offer/accept/reject/分块/SHA-256/信任三态/断开失败
- 通话信令: invite/accept/reject/end/Untrusted 拒绝/占线失败
- 编解码: 合成 PCM/YUV roundtrip

**不测**: 
- 真实设备 (麦克风/摄像头/网卡)
- egui 布局
- 主观质量 (延迟/画质)

### 真实设备测试

**方式**: 朋友异机验证 (同机 Win10 + Ubuntu VM 抢设备不可靠)

**验收**:
- 文字消息双向
- 文件传输 (发送/接收/进度/哈希)
- 语音通话 (可听/延迟 < 500ms)
- 视频通话 (可看/有声音)
- 聊天记录加密重启仍在
- 昵称设置/显示/删除

### CI

**矩阵**: ubuntu-latest + windows-latest

**检查**: `cargo build` + `cargo test`

**覆盖**: 编译通过 + 单元/集成测试 (不含真设备)

## 未来演进

### 短期 (post-v1 bugfix)

- [ ] 补充 FileChunk 与 Text/CallEnd 交织的集成测试
- [ ] 补充未知 JSON 变体不崩溃的 roundtrip 测试
- [ ] 确认 `TrustState::Unknown` 与 spec "Untrusted" 的映射

### 中期 (架构改进)

- [ ] 引入 `PeerIdHex(String)` newtype (消除 Primitive Obsession)
- [ ] 提取 `PeerState` struct (消除 Data Clumps)
- [ ] Transfer 逻辑封装为方法 (消除 Shotgun Surgery)

### 长期 (功能扩展)

- [ ] macOS 客户端 (ADR-0001)
- [ ] 第二条 QUIC 可靠流 (文件传输与信令分离)
- [ ] 视频: 一帧一流 + 过时 reset (消除丢包花屏)
- [ ] 设备选择 UI
- [ ] 回声消除
- [ ] 自建 relay 文档

## 参考

- 权威 spec: GitHub issue #1
- 术语表: CONTEXT.md
- 架构决策: docs/adr/
- 交接信息: HANDOFF.md
- 开发规范: CLAUDE.md
