# p2p-comm v1 核心功能实现

## 目标

基于 P2PCore（feat/datagram-api 分支）构建跨平台（Linux + Windows 10）P2P 通讯工具，支持：

- 文字消息
- 文件传输
- 实时语音通话
- 实时视频通话
- 加密聊天记录落盘
- 本地昵称管理
- 多会话并行

## 技术栈（已锁定）

- **核心**: P2PCore (git dep, feat/datagram-api)
- **GUI**: eframe 0.30 / egui 0.30
- **摄像头**: nokhwa 0.10 (Linux V4L2 + Windows MSMF)
- **视频编解码**: openh264 0.9
- **音频编解码**: opus 0.4
- **音频 I/O**: cpal 0.16
- **数据目录**: dirs 6.0（Linux `~/.local/share/p2p-comm`，Windows `%APPDATA%\p2p-comm`）
- **平台**: Linux + Windows 10（CI 矩阵验证编译通过）

## 架构

```
p2p-comm/
├── crates/
│   ├── p2p-comm-core/  # 无头核心，零 GUI 依赖
│   │   ├── session.rs       # Session 生命周期管理
│   │   ├── messaging.rs     # 文字消息收发
│   │   ├── file_transfer.rs # 文件传输（分块 + 进度）
│   │   ├── call.rs          # 通话信令 + 媒体流
│   │   ├── storage.rs       # 加密聊天记录 + 昵称表
│   │   └── crypto.rs        # 密钥派生（从身份密码）
│   └── p2p-comm-gui/   # eframe 前端
│       ├── main.rs          # 窗口 + 路由
│       ├── sidebar.rs       # 昵称列表
│       ├── chat_view.rs     # 消息 + 文件 + 通话 UI
│       └── settings.rs      # 身份密码输入
```

## 功能规格

### 1. 启动流程

1. 检查数据目录（`dirs::data_dir()` + `/p2p-comm`），不存在则创建
2. 弹窗要求输入**身份密码**（新用户 = 设置密码，老用户 = 解锁）
3. 从密码派生：
   - P2PCore `SecretKey`（身份）
   - 聊天记录加密密钥（AES-256-GCM，`chacha20poly1305` crate）
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
- 每个 peer 一个 `{peer_id}.jsonl`（加密 JSONL，每行一条消息）
- 加密: ChaCha20-Poly1305，密钥从身份密码派生（`HKDF-SHA256(password, salt="chat-history", info=peer_id)`）
- 启动时解密加载，运行时追加写入

### 4. 文件传输

**流程**:
1. 发送方: 选择文件 → 计算 SHA-256 → 发 `FileOffer`
2. 接收方:
   - Verified peer: 自动接受
   - TOFU peer: 弹窗确认
3. 接受后: 发送方分块发送 `FileChunk`（每块 64KB），走可靠流
4. 接收方: 显示进度条，收完验证哈希，保存到 `~/Downloads` 或 `%USERPROFILE%\Downloads`

**进度显示**: GUI 里每个文件传输显示进度条（发送/接收都有）

**不做**: 断点续传（Session 断了让对方重发）

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
- I/O: `cpal`（系统默认设备）

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

**媒体流**（走数据报）:
```
[1 字节类型 0x02=Video][8 字节时间戳][H.264 NAL unit]
```

**视频参数**:
- 分辨率: 640×480
- 帧率: 15fps
- 编码器: OpenH264（`openh264` crate）
- I/O: `nokhwa`（系统默认摄像头）

**流程**:
1. 发起方点"📹" → 发 `CallInvite { media: AudioVideo }`
2. 对方接受 → 双方启动视频+音频线程
3. 视频捕获: `nokhwa` 读帧 → OpenH264 编码 → `send_datagram`
4. 视频显示: `recv_datagram` → OpenH264 解码 → egui texture 显示在聊天视图上方

**UI**: 通话中聊天视图顶部显示对方视频画面（640×480 缩放适配窗口宽度），底部工具栏显示"挂断"按钮

### 7. 本地昵称管理

- `nicknames.json`: `{"<peer_id>": "<nickname>"}`
- 侧边栏右键菜单: "设置昵称" / "删除昵称"
- 未设置昵称的 peer 显示 Peer ID 前 8 字符

### 8. 验证策略

- **CI**: `cargo build` + `cargo test` 在 ubuntu-latest + windows-latest 通过
- **真实设备**: 找朋友异机测试音视频（Win10 宿主机 + Ubuntu VM 同机抢设备不可靠）
- **不依赖**: 同机双端测试

## 范围外（v1 不做）

- 回声消除（文档写"请用耳机"）
- 设备选择（只用系统默认）
- 断点续传
- 多设备"同一个人"聚合
- 移动端

## 验收标准

1. ✅ CI 通过（Linux + Windows 编译 + 单元测试）
2. ✅ 朋友异机测试：文字消息双向收发
3. ✅ 朋友异机测试：文件传输（发送 + 接收 + 进度条）
4. ✅ 朋友异机测试：语音通话（延迟 < 500ms，无明显卡顿）
5. ✅ 朋友异机测试：视频通话（画面流畅，音视频同步）
6. ✅ 聊天记录加密落盘 + 重启后正确加载
7. ✅ 昵称管理（设置/显示/删除）

## 实现顺序

1. **核心**: `p2p-comm-core` 实现 Session 管理 + 消息收发 + 存储（先不做文件/通话）
2. **GUI**: 侧边栏 + 聊天视图 + 文字消息显示/发送
3. **文件传输**: 核心逻辑 + GUI 进度条
4. **语音通话**: 核心逻辑 + GUI 通话 UI
5. **视频通话**: 核心逻辑 + GUI 视频显示
6. **打磨**: 错误处理 + 日志 + 文档

每个阶段都先通过 CI，再找朋友测试。
