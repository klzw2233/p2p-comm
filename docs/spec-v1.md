# p2p-comm v1 核心功能实现

## 目标

基于 P2PCore（git 依赖 `main`）构建跨平台（Linux + Windows 10；macOS 见 issue #42 / ADR-0002，未实现）P2P 通讯工具，支持：

- 文字消息
- 文件传输
- 实时语音通话
- 实时视频通话
- 加密聊天记录落盘
- 本地昵称管理
- 多会话并行

## 技术栈（已锁定）

- **核心**: P2PCore (git dep, `main`)
- **GUI**: eframe 0.30 / egui 0.30（不跟 0.36：MSRV 1.80）
- **摄像头**: nokhwa 0.10 (Linux V4L2 + Windows MSMF；macOS AVFoundation 属 #42)
- **视频编解码**: openh264 0.9
- **音频编解码**: opus 0.4
- **音频 I/O**: cpal 0.18
- **数据目录**: dirs 6.0（Linux `~/.local/share/p2p-comm`，Windows `%APPDATA%\p2p-comm`，macOS `~/Library/Application Support/p2p-comm` 属 #42）
- **平台**: Linux + Windows 10（CI 矩阵验证编译通过）。macOS 增量 spec 见 issue #42 / ADR-0002（未实现；#41 已关）。

## 架构

```
p2p-comm/
├── crates/
│   ├── p2p-comm-core/  # 无头核心，零 GUI 依赖
│   │   ├── node.rs          # Session / 文字 / 文件 / 通话状态机
│   │   ├── frame.rs         # 可靠流帧
│   │   ├── audio.rs         # Opus + 音频数据报
│   │   ├── video.rs         # H.264 + 视频数据报
│   │   ├── audio_io.rs      # 默认麦/扬声器
│   │   ├── video_io.rs      # 默认摄像头
│   │   ├── chatlog.rs / nicknames.rs / roster.rs / inbox.rs
│   └── p2p-comm-gui/   # eframe 前端（单文件 main.rs）
```

## 功能规格

### 1. 启动流程

1. 检查数据目录（`dirs::data_dir()` + `/p2p-comm`），不存在则创建
2. 弹窗要求输入**身份密码**（新用户 = 设置密码，老用户 = 解锁）
3. 从密码派生：
   - 身份：交给 P2PCore `FileKeyStore`（Argon2id + 随机 salt + ChaCha20-Poly1305；错密码不得重建身份）
   - 聊天记录：同一密码，独立随机 32 B salt 存数据目录；Argon2id → master，再 `HKDF-Expand(master, info=peer_id)` → ChaCha20-Poly1305
4. 加载本地昵称表（`nicknames.json`：`{peer_id: nickname}`）
5. 加载历史聊天记录（每个 peer 一个 `.jsonl` 文件，解密后显示）
6. 启动 P2PCore endpoint，进入主界面

### 2. 主界面布局

```
┌─────────────┬──────────────────────────────────┐
│  侧边栏      │  聊天视图                         │
│             │                                  │
│ [Alice]     │  Alice                    [📞][📹]│
│ [Bob]       │  ────────────────────────────────│
│  Charlie    │  2026-09-06 10:23               │
│             │  Alice: 你好                     │
│  + 拨号      │  Me: 在吗？                      │
│             │  [file.zip] 1.2MB ↓             │
│             │  ────────────────────────────────│
│             │  [输入框]              [发送][📎] │
└─────────────┴──────────────────────────────────┘
```

- **侧边栏**: 昵称列表（点击切换聊天视图），底部"+ 拨号"按钮（输入 Peer ID 或昵称 → 发起连接）
- **聊天视图**: 当前选中 peer 的消息历史 + 输入框 + 文件/通话按钮
- **多会话**: 每个 peer 一个独立 Session（P2PCore 支持），切换视图不关闭连接

### 3. 文字消息

**帧格式**（复用 p2p-chat ADR-0001）:
```
[4 字节 LE 长度][JSON payload]
```

消息类型:
```rust
enum Message {
    Text { content: String, timestamp: u64 },
    FileOffer { name: String, size: u64, hash: [u8; 32] },
    FileAccept,
    FileReject,
    FileChunk { offset: u64, data: Vec<u8> },
    CallInvite { media: MediaType },  // Audio | Video | AudioVideo
    CallAccept,
    CallReject,
    CallEnd,
}
```

**聊天记录落盘**:
- 每个 peer 一个 `{peer_id}.jsonl`（加密 JSONL，每行一条消息：随机 nonce + ciphertext+tag）
- 加密: ChaCha20-Poly1305。禁止 `HKDF(password)` 直接当密钥（密码熵不够）。流程：随机 32 B salt 存数据目录 → Argon2id(password, salt) → master → `HKDF-Expand(master, info=peer_id)`
- 启动时解密加载，运行时追加写入

### 4. 文件传输

**流程**:
1. 发送方: 选择文件 → 计算 SHA-256 → 发 `FileOffer`
2. 接收方:
   - Verified peer: 自动接受
   - TOFU peer: 弹窗确认
3. 接受后: 发送方分块发送 `FileChunk`（每块 64KB，JSON 里 `data` 为 base64），走 **同一条** 可靠流（P2PCore Session 目前只暴露一条 bidi）
4. 接收方: 显示进度条，收完验证哈希，保存到 `~/Downloads` 或 `%USERPROFILE%\Downloads`

**进度显示**: GUI 里每个文件传输显示进度条（发送/接收都有）

**已知限制**: FileChunk 与 Text/Call 信令排队。大文件传输期间打字/挂断会等当前 chunk 写完。这是 v1 接受的 HOL，不是 bug。

**不做**: 断点续传（Session 断了让对方重发）；第二条可靠流（等 P2PCore 暴露再考虑）

### 5. 实时语音通话

**信令**（走可靠流，复用文字消息帧格式）:
```
A → B: CallInvite { media: Audio }
B → A: CallAccept
[媒体流开始]
A/B: CallEnd
```

**媒体流**（走数据报）:
```
[1 字节类型 0x01=Audio][8 字节时间戳][Opus payload]
```

**音频参数**:
- 采样率: 48kHz
- 声道: 单声道
- 帧长: 20ms
- 编码器: Opus（`opus` crate）
- I/O: `cpal` 0.18（系统默认设备）。回调线程只拷贝 PCM，不在回调里编码/发网

**流程**:
1. 发起方点"📞" → 发 `CallInvite { media: Audio }`
2. 对方弹窗接受/拒绝 → 发 `CallAccept` / `CallReject`
3. 双方启动音频捕获/播放线程:
   - 捕获线程: `cpal` 读麦克风 → Opus 编码 → `send_datagram`
   - 播放线程: `recv_datagram` → Opus 解码 → `cpal` 写扬声器
4. 任一方点"挂断" → 发 `CallEnd`，停止线程

**不做**: 回声消除（文档写明"请用耳机"）

### 6. 实时视频通话

**信令**: 同语音，`CallInvite { media: AudioVideo }`

**媒体流**（走数据报；DATAGRAM **不能分片**，RFC 9221）:
```
[1 字节类型 0x02=Video][8 字节时间戳][一个 H.264 NAL，长度 ≤ max_datagram_size()-9]
```

**视频参数**:
- 分辨率: 640×480
- 帧率: 15fps
- 编码器: OpenH264（`openh264` crate），`max_slice_len` 设为 `Session::max_datagram_size().unwrap_or(1200) - 9`
- I/O: `nokhwa`（系统默认摄像头；编码始终 640×480，设备不必提供该精确模式）
- 一帧可以发出多个 DATAGRAM（多个 NAL）。超 MTU 的 NAL 丢弃该 slice，不在应用层重组
- 丢包导致花屏/冻结是 v1 可接受行为；不做 FEC/NACK。升级路径：一帧一条 QUIC 单向流、过时 reset（MoQ 风格）—— v1 不做

**流程**:
1. 发起方点"📹" → 发 `CallInvite { media: AudioVideo }`
2. 对方接受 → 双方启动视频+音频线程
3. 视频捕获: `nokhwa` 读帧 → RGB→I420 → OpenH264 编码 → 按 NAL `send_datagram`
4. 视频显示: `recv_datagram` → OpenH264 解码 → 复用同一个 egui `TextureHandle` 每帧 `set`（禁止每帧 `load_texture` 新 ID）

**UI**: 通话中聊天视图顶部显示对方视频画面（640×480 缩放适配窗口宽度），底部工具栏显示"挂断"按钮

### 7. 本地昵称管理

- `nicknames.json`: `{"<peer_id>": "<nickname>"}`
- 侧边栏右键菜单: "设置昵称" / "删除昵称"
- 未设置昵称的 peer 显示 Peer ID 前 8 字符

### 8. 验证策略

- **CI**: `cargo build` + `cargo test` 在 ubuntu-latest + windows-latest 通过（macos-latest 属 #42）
- **真实设备**: 找朋友异机测试音视频（Win10 宿主机 + Ubuntu VM 同机抢设备不可靠）；跟踪票 issue #41（已关）
- **不依赖**: 同机双端测试

## 范围外（v1 不做）

- macOS 客户端实现（增量 spec issue #42 / ADR-0002；#41 已关，尚未写代码）
- 回声消除（文档写"请用耳机"）
- 设备选择（只用系统默认）
- 断点续传
- 多设备"同一个人"聚合
- 移动端
- 第二条 QUIC 可靠流
- 视频 FEC/NACK / 每帧一 stream（升级路径，v1 不做）

## 验收标准

1. ✅ CI 通过（Linux + Windows 编译 + 单元测试）
2. 朋友异机测试：文字消息双向收发
3. 朋友异机测试：文件传输（发送 + 接收 + 进度条）
4. 朋友异机测试：语音通话（延迟 < 500ms，无明显卡顿）
5. 朋友异机测试：视频通话（画面流畅，音视频同步）
6. ✅ 聊天记录加密落盘 + 重启后正确加载
7. ✅ 昵称管理（设置/显示/删除）

## 实现顺序

1. **核心**: `p2p-comm-core` 实现 Session 管理 + 消息收发 + 存储（先不做文件/通话）
2. **GUI**: 侧边栏 + 聊天视图 + 文字消息显示/发送
3. **文件传输**: 核心逻辑 + GUI 进度条
4. ~~**语音通话**: 核心逻辑 + GUI 通话 UI~~ 已合入（PR #17 / issue #7）
5. ~~**视频通话**: 核心逻辑 + GUI 视频显示~~ 已合入（PR #18 / issue #8）
6. **打磨**: 错误处理 + 日志 + 文档

每个阶段都先通过 CI，再找朋友测试。
