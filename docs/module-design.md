# p2p-comm 模块设计文档

## 概述

本文档详细描述 p2p-comm 各模块的职责边界、接口契约、状态机、数据结构与实现约束。

---

## 1. p2p-comm-gui (crates/p2p-comm-gui)

### 1.1 职责

- 用户输入采集
- 状态展示 (消息、进度、视频画面)
- 驱动 Node 轮询循环

### 1.2 约束

- **零业务逻辑**: 所有逻辑在 p2p-comm-core
- **单向依赖**: 只调用 Node API，不被 core 回调
- **单文件**: v1 实现在 `main.rs`

### 1.3 关键数据结构

```rust
struct App {
    node: Option<Node>,
    password_input: String,
    dial_input: String,
    text_input: String,
    selected_peer: Option<String>,
    video_texture: Option<TextureHandle>,
    error: Option<String>,
}
```

### 1.4 主循环

```rust
impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // 1. 轮询事件
        while let Some(event) = self.node.poll()? {
            self.handle_event(event);
        }
        
        // 2. 渲染 UI
        self.render_sidebar(ctx);
        self.render_chat_view(ctx);
        
        // 3. 重绘策略
        if self.should_repaint_immediately() {
            ctx.request_repaint();
        } else {
            ctx.request_repaint_after(Duration::from_secs(1));
        }
    }
}
```

### 1.5 重绘策略 (issue #25)

**立即重绘** (`request_repaint()`):
- `Connecting`
- 文件 `Offered` / `Transferring`
- 通话中
- 待确认文件
- 待接听邀请

**低频重绘** (`request_repaint_after(1s)`):
- 仅 `Connected`，无上述活动

---

## 2. p2p-comm-core (crates/p2p-comm-core)

### 2.1 模块树

```
p2p-comm-core/
├── lib.rs          # 公开 API 导出
├── peer_id.rs      # PeerIdHex newtype
├── node.rs         # 核心状态机
├── frame.rs        # 可靠流帧编解码
├── audio.rs        # Opus 编解码
├── video.rs        # H.264 编解码
├── audio_io.rs     # cpal 封装
├── video_io.rs     # nokhwa 封装
├── chatlog.rs      # 加密聊天记录
├── nicknames.rs    # 本地昵称表
├── roster.rs       # 已知 Peer 列表
└── inbox.rs        # 聊天记录 + 未读
```

---

## 3. Node (node.rs)

### 3.1 职责

- Session 生命周期管理
- 消息路由
- 文件传输状态机
- 通话状态机
- 事件队列

### 3.2 公开 API

```rust
impl Node {
    pub async fn start(dir: &Path, password: &str) -> Result<Self, Error>;
    pub fn dial(&mut self, input: &str) -> Result<(), Error>; // nickname-or-hex
    pub fn send_text(&mut self, peer_id_hex: &PeerIdHex, content: &str) -> Result<(), Error>;
    pub fn send_file(&mut self, peer_id_hex: &PeerIdHex, path: &Path) -> Result<(), Error>;
    pub fn accept_file(&mut self, peer_id_hex: &PeerIdHex);
    pub fn reject_file(&mut self, peer_id_hex: &PeerIdHex);
    pub fn invite_audio(&mut self, peer_id_hex: &PeerIdHex) -> Result<(), Error>;
    pub fn invite_video(&mut self, peer_id_hex: &PeerIdHex) -> Result<(), Error>;
    pub fn accept_call(&mut self, peer_id_hex: &PeerIdHex);
    pub fn reject_call(&mut self, peer_id_hex: &PeerIdHex);
    pub fn hangup(&mut self);
    pub fn poll(&mut self) -> bool;
    pub fn snapshot(&self) -> Snapshot;
    pub fn set_nickname(&mut self, peer_id_hex: &PeerIdHex, nickname: &str) -> Result<(), Error>;
    pub fn remove_nickname(&mut self, peer_id_hex: &PeerIdHex) -> Result<(), Error>;
}
```

### 3.3 核心数据结构

```rust
struct Node {
    peers: HashMap<PeerIdHex, PeerState>,
    call: Option<CallState>,
    nicknames: NicknameStore,
    roster: Roster,
    inbox: Inbox,
    keys: ChatKeys,
    io_events: mpsc::UnboundedReceiver<IoEvent>,
    io_tx: mpsc::UnboundedSender<IoEvent>,
}

struct PeerState {
    live: Option<mpsc::UnboundedSender<Vec<u8>>>,
    transfer: Option<Transfer>,
    pending: Option<IncomingOffer>,
    dgram_tx: Option<mpsc::UnboundedSender<Vec<u8>>>,
    max_dgram: Option<usize>,
    trust: TrustState,
}

struct Transfer {
    direction: Direction,
    filename: String,
    size: u64,
    hash: String,
    status: TransferStatus,
    buffer: Vec<u8>,
    next_offset: u64,
}

enum TransferStatus {
    Offered,
    Transferring { sent: u64 },
    Complete,
    Failed { reason: String },
}

struct CallState {
    peer_id_hex: PeerIdHex,
    media: MediaType,
    phase: CallPhase,
    result: Option<CallResult>,  // issue #24
}

enum CallPhase {
    Inviting,
    Ringing,
    Active,
}

enum CallResult {
    Rejected,
    TimedOut,
}
```

### 3.4 状态机

#### 3.4.1 Session 状态

```
     dial()
       ↓
  Connecting ──────→ Connected
       │                │
       │                │ disconnect
       ↓                ↓
     Failed ←──────── Failed
       ↑
       └── dial() 可重拨 (issue #20)
```

**不变量**:
- 每个 `peer_id_hex` 最多一个 `Connecting` 或 `Connected`
- `Failed` 状态允许重拨
- 已 `Connected` / `Connecting` 时 `dial()` 只切换视图

#### 3.4.2 文件传输状态

**发送方**:
```
send_file()
    ↓
FileOffer ──→ wait FileAccept/Reject
    │              │
    │ accept       │ reject
    ↓              ↓
Transferring ──→ Complete (SHA-256 验证)
    │
    │ disconnect / error
    ↓
  Failed
```

**接收方**:
```
收到 FileOffer
    ↓
检查 TrustState
    ├─ Verified: 自动 accept
    ├─ TOFU: Event::FileOffered (弹窗)
    └─ Unknown: 自动 reject
    │
    │ accept
    ↓
接收 FileChunk (循环)
    ↓
SHA-256 验证
    ├─ 成功: unique_download_path() 落盘
    └─ 失败: Failed (不落盘)
```

**issue #22 约束**:
- 发送方一次只排队一块 64KiB FileChunk
- 上一块写出后才 `enqueue_next_chunk()` 下一块
- 文字/挂断可插在块之间

**issue #23 约束**:
- 目标名已存在: `name (1).ext`, `name (2).ext`...
- 校验失败: `Failed`，不落盘
- 路径穿越: `safe_filename()` 去除 `/` `\\` `..`

#### 3.4.3 通话状态

```
invite_audio/video()
    ↓
  Inviting ──→ wait CallAccept/Reject
    │              │
    │ accept       │ reject
    ↓              ↓
  Active ──────→ CallResult::Rejected (issue #24)
    │
    │ hangup / CallEnd / disconnect
    ↓
  (清除)
```

**全进程单路约束**:
- `call.is_some()` 时，任何新邀请失败
- `hangup()` 不依赖 `selected_peer` (issue #24)

**CallResult 清除规则** (issue #24):
- 再发起邀请
- 接受/拒绝新邀请
- 在该 Peer 视图发送文字
- 切换到别的 Peer

### 3.5 事件模型

```rust
pub enum Event {
    TextReceived { from: String, content: String, timestamp: u64 },
    TextSendFailed { to: String, error: String },
    FileOffered { from: String, name: String, size: u64 },
    FileTransferProgress { peer: String, sent: u64, total: u64 },
    FileTransferComplete { peer: String },
    FileTransferFailed { peer: String, reason: String },
    CallInvite { from: String, media: MediaType },
    CallAccepted { peer: String },
    CallRejected { peer: String },
    CallEnded { peer: String },
    Disconnected { peer: String, error: String },
    // ...
}
```

**轮询模型**:
```rust
pub fn poll(&mut self) -> Result<Option<Event>> {
    // 1. 处理 I/O 事件
    while let Ok(io_event) = self.io_rx.try_recv() {
        self.handle_io_event(io_event)?;
    }
    
    // 2. 轮询所有 Session
    for (peer_id, live) in &mut self.live {
        if let LivePeer::Connected { session, .. } = live {
            self.poll_session(peer_id, session)?;
        }
    }
    
    // 3. 返回一个用户事件
    Ok(self.event_rx.try_recv().ok())
}
```

### 3.6 文件排队实现 (issue #22)

```rust
fn enqueue_next_chunk(&mut self, peer_id_hex: &PeerIdHex) {
    let Some(xfer) = self.peers.get_mut(peer_id_hex).and_then(|p| p.transfer.as_mut()) else {
        return;
    };
    // 只取 64KiB；写完一块等 SendProgress 再入队下一块（issue #22）
    let offset = usize::try_from(xfer.next_offset).unwrap_or(usize::MAX);
    if offset >= xfer.bytes.len() {
        return;
    }
    let end = (offset + 64 * 1024).min(xfer.bytes.len());
    let frame = encode_file_chunk(xfer.next_offset, &xfer.bytes[offset..end]);
    xfer.next_offset = u64::try_from(end).unwrap_or(u64::MAX);
    self.send_to_live(peer_id_hex, frame);
}
```

### 3.7 挂断停相机 (issue #24)

```rust
pub fn hangup(&mut self) -> Result<()> {
    let call = self.call.take().ok_or("no active call")?;
    
    // 1. 发 CallEnd
    let session = self.get_session(&call.peer_id_hex)?;
    session.send_reliable(&encode_call_end())?;
    
    // 2. 停媒体
    if matches!(call.media, MediaType::AudioVideo) {
        self.stop_video_capture()?;  // 释放摄像头
    }
    self.stop_audio_io()?;  // 释放麦/扬声器
    
    Ok(())
}

fn stop_video_capture(&mut self) -> Result<()> {
    // 向视频线程发停止信号
    self.video_stop_tx.send(())?;
    
    // 允许短超时 join (避免 GUI 卡死在 camera.frame())
    if self.video_thread.join_timeout(Duration::from_millis(500)).is_err() {
        // 超时后仍已发出停止信号，线程最终会退出
    }
    
    Ok(())
}
```

**三条停止路径** (issue #24):
1. 本端 `hangup()`
2. 收到对方 `CallEnd`
3. Session `Disconnected`

---

## 4. Frame (frame.rs)

### 4.1 职责

- 可靠流帧的编码/解码
- JSON 消息的序列化/反序列化

### 4.2 接口

```rust
pub fn encode_frame(msg: &Message) -> Vec<u8>;
pub fn decode_frame(bytes: &[u8]) -> Result<Decoded>;

pub enum Decoded {
    Message(Message),
    Ignored,  // 未知 JSON 变体 (issue #5)
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Message {
    Text { content: String, timestamp: u64 },
    FileOffer { name: String, size: u64, hash: String },
    FileAccept,
    FileReject,
    FileChunk { offset: u64, data: String },  // base64
    CallInvite { media: MediaType },
    CallAccept,
    CallReject,
    CallEnd,
    #[serde(other)]
    Unknown,  // 不崩溃 (issue #5)
}
```

### 4.3 帧格式

```
+-------------------+-------------------+
| 4 字节 LE 长度     | UTF-8 JSON       |
+-------------------+-------------------+
```

**编码**:
```rust
pub fn encode_frame(msg: &Message) -> Vec<u8> {
    let json = serde_json::to_string(msg).unwrap();
    let len = json.len() as u32;
    let mut buf = Vec::with_capacity(4 + json.len());
    buf.extend_from_slice(&len.to_le_bytes());
    buf.extend_from_slice(json.as_bytes());
    buf
}
```

**解码**:
```rust
pub fn decode_frame(bytes: &[u8]) -> Result<Decoded> {
    if bytes.len() < 4 {
        return Err("too short");
    }
    let len = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize;
    let json_bytes = &bytes[4..4 + len];
    match serde_json::from_slice(json_bytes) {
        Ok(Message::Unknown) => Ok(Decoded::Ignored),
        Ok(msg) => Ok(Decoded::Message(msg)),
        Err(_) => Ok(Decoded::Ignored),  // 解析失败也不崩
    }
}
```

---

## 5. Audio (audio.rs)

### 5.1 职责

- Opus 编解码
- 音频数据报格式

### 5.2 接口

```rust
pub struct AudioEncoder {
    encoder: opus::Encoder,
}

impl AudioEncoder {
    pub fn new() -> Result<Self>;
    pub fn encode(&mut self, pcm: &[i16]) -> Result<Vec<u8>>;
}

pub struct AudioDecoder {
    decoder: opus::Decoder,
}

impl AudioDecoder {
    pub fn new() -> Result<Self>;
    pub fn decode(&mut self, opus_data: &[u8]) -> Result<Vec<i16>>;
}

pub fn encode_audio_datagram(timestamp: u64, opus: &[u8]) -> Vec<u8>;
pub fn decode_audio_datagram(bytes: &[u8]) -> Result<(u64, Vec<u8>)>;
```

### 5.3 数据报格式

```
+------+--------------------+-------------------+
| 0x01 | u64 BE 时间戳       | Opus payload     |
+------+--------------------+-------------------+
  1 B        8 B                  变长
```

### 5.4 参数

- 采样率: 48000 Hz
- 声道: 1 (单声道)
- 帧长: 20ms → 960 samples
- 比特率: 24000 bps

---

## 6. Video (video.rs)

### 6.1 职责

- H.264 编解码
- 视频数据报格式
- NAL 切片限制

### 6.2 接口

```rust
pub struct VideoEncoder {
    encoder: openh264::encoder::Encoder,
    max_nal_len: usize,
}

impl VideoEncoder {
    pub fn new(max_datagram_size: usize) -> Result<Self> {
        let max_nal_len = max_datagram_size.saturating_sub(9);  // issue #8
        let mut encoder = openh264::encoder::Encoder::new()?;
        encoder.set_max_slice_len(max_nal_len)?;
        Ok(Self { encoder, max_nal_len })
    }
    
    pub fn encode(&mut self, yuv: &[u8]) -> Result<Vec<Vec<u8>>>;  // 多个 NAL
}

pub struct VideoDecoder {
    decoder: openh264::decoder::Decoder,
}

impl VideoDecoder {
    pub fn new() -> Result<Self>;
    pub fn decode(&mut self, nal: &[u8]) -> Result<Option<YuvFrame>>;
}

pub fn encode_video_datagram(timestamp: u64, nal: &[u8]) -> Vec<u8>;
pub fn decode_video_datagram(bytes: &[u8]) -> Result<(u64, Vec<u8>)>;
```

### 6.3 数据报格式

```
+------+--------------------+-------------------+
| 0x02 | u64 BE 时间戳       | H.264 NAL        |
+------+--------------------+-------------------+
  1 B        8 B                  ≤ max_dgram-9
```

### 6.4 参数

- 分辨率: 640×480
- 帧率: 15 fps
- `max_slice_len`: `Session::max_datagram_size().unwrap_or(1200) - 9`

### 6.5 丢包处理

**v1 行为**: 超限 NAL 丢弃，丢包导致花屏/冻结可接受。

**未来**: 一帧一条 QUIC 单向流，过时 reset (MoQ 风格)。

---

## 7. Audio I/O (audio_io.rs)

### 7.1 职责

- cpal 默认麦克风/扬声器封装
- PCM 缓冲区管理

### 7.2 接口

```rust
pub struct AudioCapture {
    stream: cpal::Stream,
    buffer_rx: mpsc::Receiver<Vec<i16>>,
}

impl AudioCapture {
    pub fn start() -> Result<Self>;
    pub fn read_frame(&mut self) -> Option<Vec<i16>>;  // 960 samples
    pub fn stop(self) -> Result<()>;
}

pub struct AudioPlayback {
    stream: cpal::Stream,
    buffer_tx: mpsc::Sender<Vec<i16>>,
}

impl AudioPlayback {
    pub fn start() -> Result<Self>;
    pub fn write_frame(&mut self, pcm: &[i16]) -> Result<()>;
    pub fn stop(self) -> Result<()>;
}
```

### 7.3 约束

- **回调线程只拷贝 PCM**: 不编码/不发网/不解码
- **缓冲区**: `Arc<Mutex<RingBuffer>>`，回调线程与编解码线程同步

---

## 8. Video I/O (video_io.rs)

### 8.1 职责

- nokhwa 默认摄像头封装
- RGB → I420 转换

### 8.2 接口

```rust
pub struct VideoCapture {
    camera: nokhwa::Camera,
    stop_rx: mpsc::Receiver<()>,
}

impl VideoCapture {
    pub fn start() -> Result<Self>;
    pub fn read_frame(&mut self) -> Result<YuvFrame>;
    pub fn stop(self) -> Result<()>;
}
```

### 8.3 约束 (issue #24)

- **挂断停采集**: 三条路径 (本端挂断 / 对方 CallEnd / Session 断开) 都停止
- **释放设备**: `stop()` 后摄像头指示灯应灭
- **join 超时**: 允许 500ms 超时，避免 GUI 卡死在 `camera.frame()`

---

## 9. ChatLog (chatlog.rs)

### 9.1 职责

- 加密聊天记录落盘/加载
- 密钥派生

### 9.2 接口

```rust
pub struct ChatKeys {
    dir: PathBuf,
    master: [u8; 32],
}

impl ChatKeys {
    pub fn unlock(dir: &Path, password: &str) -> Result<Self, Error>;
    pub fn load(&self, peer_id_hex: &str) -> Result<Vec<ChatMessage>, Error>; // 文件名/HKDF info
    pub fn append(&self, peer_id_hex: &str, msg: &ChatMessage) -> Result<(), Error>;
}
```

### 9.3 密钥派生

```
独立随机 32 B salt (存 data_dir/chat_salt)
    ↓
master = Argon2id(password, salt)
    time_cost=2, mem_cost=19456, parallelism=1
    ↓
per_peer_key = HKDF-Expand(master, info=peer_id_hex.as_bytes())
    ↓
ChaCha20-Poly1305
```

**与身份密钥分离**: 身份走 P2PCore `FileKeyStore`，聊天记录走上述独立派生。

### 9.4 文件格式

**路径**: `{data_dir}/{peer_id_hex}.jsonl`

**每行**:
```
[12 字节随机 nonce][ciphertext + 16 字节 tag]
```

**明文**: JSON 序列化的 `Message`

---

## 10. Nicknames (nicknames.rs)

### 10.1 职责

- 本地昵称表管理
- JSON 持久化

### 10.2 接口

```rust
pub struct NicknameStore {
    path: PathBuf,
    by_peer: BTreeMap<PeerIdHex, String>,
}

impl NicknameStore {
    pub fn load(dir: &Path) -> Result<Self, Error>;
    pub fn get(&self, peer_id_hex: &PeerIdHex) -> Option<&str>;
    pub fn set(&mut self, peer_id_hex: &PeerIdHex, nickname: &str) -> Result<(), Error>;
    pub fn remove(&mut self, peer_id_hex: &PeerIdHex) -> Result<(), Error>;
    pub fn display_name(&self, peer_id_hex: &PeerIdHex) -> String; // nickname or PeerIdHex::short()
}
```

### 10.3 文件格式

**路径**: `{data_dir}/nicknames.json`

**内容**:
```json
{
  "abc123...": "Alice",
  "def456...": "Bob"
}
```

**约束**:
- 变更立即写回 (不缓存)
- 不上线传输

---

## 11. Roster (roster.rs)

### 11.1 职责

- 维护已知 Peer 列表
- 返回侧边栏显示项

### 11.2 接口

```rust
pub struct Roster {
    peers: BTreeMap<PeerIdHex, PeerStatus>,
    selected: Option<PeerIdHex>,
    errors: BTreeMap<PeerIdHex, ChatError>,
}
```

---

## 12. Inbox (inbox.rs)

### 12.1 职责

- 内存中的聊天记录 + 未读计数。磁盘走 `ChatKeys`。
- TOFU 文件 offer 不在这里，在 `PeerState.pending`。

### 12.2 接口

```rust
pub struct Inbox {
    by_peer: BTreeMap<PeerIdHex, Vec<ChatMessage>>,
    unread: BTreeMap<PeerIdHex, u32>,
}
```

---

## 13. 模块依赖图

```
     ┌───────────────┐
     │  p2p-comm-gui │
     └───────┬───────┘
             │ Node API
     ┌───────▼───────┐
     │     Node      │
     └───┬───┬───┬───┘
         │   │   │
    ┌────┘   │   └────┐
    │        │        │
┌───▼───┐ ┌─▼──┐ ┌───▼────┐
│ Frame │ │I/O │ │Storage │
└───┬───┘ └─┬──┘ └───┬────┘
    │       │        │
┌───▼───────▼────────▼───┐
│       P2PCore          │
└────────────────────────┘
```

**依赖规则**:
- GUI 只依赖 Node 公开 API
- Node 协调所有子模块
- 子模块不互相依赖 (除 Frame 被 Node 用于编解码)

---

## 14. 测试覆盖

### 14.1 单元测试

**Frame**:
- 编码/解码往返
- 未知 JSON 变体返回 `Decoded::Ignored`
- 长度前缀边界

**Audio/Video**:
- 合成 PCM/YUV → 编码 → 解码 → roundtrip
- 数据报格式解析

### 14.2 集成测试 (Node)

**seam**: Node 公开 API + `test_node` 假 Session

**覆盖**:
- 身份密码 (设置/解锁/错密码/空密码)
- 昵称 (增改删/重启仍在)
- 文字 (双端收发/失败路径)
- 文件 (offer/accept/reject/分块/SHA-256/信任三态/断开失败)
- 通话信令 (invite/accept/reject/end/Untrusted 拒绝/占线失败)
- 断线重拨 (issue #20)
- 文件排队 (issue #22: 两块间能插 Text)
- Downloads 同名 (issue #23: `name (1).ext`)
- CallResult 清除 (issue #24)

**不测**:
- 真实设备 (麦克风/摄像头)
- egui 布局

---

## 15. 性能考量

### 15.1 内存

**Snapshot 深拷贝**:
- `node.snapshot()` 克隆所有消息/状态
- GUI 每帧调用一次
- **优化方向**: `Arc<[Message]>` + 增量更新

### 15.2 文件传输

**64KiB 块排队**:
- 避免整文件 base64 占内存
- 权衡: 等当前块写完才能打字/挂断 (v1 接受)

### 15.3 视频编解码

**线程模型**:
- 捕获线程: `nokhwa.frame()` → RGB→I420 → `encoder.encode()` → `send_datagram()`
- 播放线程: `recv_datagram()` → `decoder.decode()` → I420
- GUI 线程: I420 → egui `TextureHandle::set()`

**TextureHandle 复用** (issue #8):
- 禁止每帧 `ctx.load_texture()` 新 ID
- 复用同一个 handle，每帧 `set()` 更新内容

---

## 16. 错误处理

### 16.1 策略

- **可恢复**: Session 断开 → `Failed` 状态，允许重拨
- **不可恢复**: 密码错误 → 返回 `Err`，不重建身份
- **静默**: 超限 NAL 丢弃 (不上报错误)

### 16.2 Event 错误

```rust
Event::TextSendFailed { to, error }
Event::FileTransferFailed { peer, reason }
Event::Disconnected { peer, error }
```

---

## 17. 未来改进

### 17.1 架构 (code review 发现)

- [x] `PeerIdHex(String)` newtype (消除 Primitive Obsession) — issue #34 PR1 (#36)
- [x] `PeerState` struct (消除 Data Clumps) — issue #34 PR2 (#37)
- [ ] `Transfer::try_enqueue_chunk()` (消除 Shotgun Surgery；issue #34 可选)
- [ ] `Snapshot` 增量更新 (减少深拷贝)

### 17.2 测试 (spec review 发现)

- [ ] FileChunk 与 Text/CallEnd 交织测试 (issue #22)
- [ ] 未知 JSON 变体不崩溃 roundtrip 测试 (issue #5)

### 17.3 功能

- [ ] 第二条 QUIC 可靠流 (文件与信令分离)
- [ ] 视频: 一帧一流 + 过时 reset

---

## 参考

- 架构总览: docs/architecture.md
- 权威 spec: GitHub issue #1
- 术语表: CONTEXT.md
- 开发规范: CLAUDE.md
