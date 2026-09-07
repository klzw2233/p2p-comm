# p2p-comm v1 全项目代码审查报告 (Code Review Report)

- **审查对象**: `p2p-comm` 完整代码库（比较基准 `1c1eaaf` initial scaffold ... `HEAD` / `5ed5d0d`）
- **依据规格**: 
  - [GitHub Issue #1 (Spec: p2p-comm v1 核心功能实现)](https://github.com/klzw2233/p2p-comm/issues/1)
  - 子任务 Issues: [#3 (身份密码)](https://github.com/klzw2233/p2p-comm/issues/3), [#4 (拨号与侧边栏)](https://github.com/klzw2233/p2p-comm/issues/4), [#5 (文字与加密记录)](https://github.com/klzw2233/p2p-comm/issues/5), [#6 (文件传输)](https://github.com/klzw2233/p2p-comm/issues/6), [#7 (语音通话)](https://github.com/klzw2233/p2p-comm/issues/7), [#8 (视频通话)](https://github.com/klzw2233/p2p-comm/issues/8)
  - `docs/spec-v1.md`、`CONTEXT.md`、`CLAUDE.md`
- **审查日期**: 2026-09-07
- **审查模型 (Reviewer)**: `gemini-flash-3.8`

---

## 1. 总体评价 (Executive Summary)

项目在极短时间内完成了从空脚手架到具备端到端加密文字、安全文件传输、实时音频与视频通话的完整 P2P 通信应用。架构遵循了清晰的无头 Core (`p2p-comm-core`) 与轻量 GUI (`p2p-comm-gui`) 分离设计，严格遵守了边界约束（无 GUI 依赖进 Core、纯默认设备采集、公共 relay opt-in、egui 贴图复用、禁止直接用密码做 AEAD 密钥等）。

然而，在审查中也发现了若干需要关注的代码气味、边缘行为缺陷以及已知的 deliberate shortcut（`ponytail:` 标记的妥协点）。

---

## 2. 第一维度：代码规范与设计（Standards Axis）

对照 `CLAUDE.md` 项目规范、全局工程原则以及 Fowler 代码气味基线：

### 2.1 文档规范符合度 (Documented Standards)

| 规范条目 | 状态 | 评估与定位 |
|---|---|---|
| **Core 零 GUI 依赖** | ✅ 符合 | `p2p-comm-core/Cargo.toml` 无 egui/eframe 依赖，完全可无头测试。 |
| **单元与集成测试** | ✅ 符合 | 测试全部挂在 core 层，包含编解码 roundtrip、状态机仿真，未侵入物理硬件。 |
| **设备限制** | ✅ 符合 | 仅采集系统默认设备，未引入未授权的设备选择复杂度。 |
| **Relay 配置** | ✅ 符合 | 正确配置 `RelayConfig::n0_public()` 并提供内置 relay URL hints。 |
| **TextureHandle 复用** | ✅ 符合 | `main.rs:440-446` 复用 `main.video_tex` 并调用 `tex.set(...)`，禁止每帧重新分配。 |
| **视频 NAL MTU 限制** | ✅ 符合 | `video.rs` 与 `node.rs` 正确计算 `max_nal_len = max_datagram_size - 9`。 |
| **挂断释放摄像头与指示灯** | ⚠️ **轻微偏离** | `audio_io.rs:135-138` 在 drop 时直接丢弃了 `JoinHandle`（detached），未同步等待摄像头线程结束。 |

### 2.2 代码气味与架构债务 (Code Smells & Debt)

#### 1. 挂断摄像头线程脱节 (Detached Video Thread)
- **位置**: `crates/p2p-comm-core/src/audio_io.rs:135-138`
- **代码**:
  ```rust
  if let Some(h) = self.video.take() {
      // ponytail: detached join so hangup isn't blocked on camera.frame(); join-with-timeout if LED must go off first.
      let _ = h;
  }
  ```
- **分析**: `nokhwa::Camera::frame()` 是阻塞式读取。当挂断时，将 `stop` 设为 `true` 并不能立即中断正在阻塞读取的系统调用。直接丢弃 `JoinHandle` 导致该线程后台运行至下一帧返回，`camera.stop_stream()` 延迟调用。这不仅导致摄像头物理指示灯未能立即熄灭，且若用户挂断后立刻拨打下一个视频通话，可能因设备仍被占用而打开失败。
- **改进建议**: 在后台线程中采用带超时机制的 join（如 200ms），或者在 `Camera` 上显式触发流中断。

#### 2. 重复代码 (Duplicated Code)
- **位置**: `crates/p2p-comm-core/src/node.rs:673-692` 与 `node.rs:557-577`
- **分析**: 在 `handle_file_offer`（针对 Verified 对端自动接收）与 `accept_file`（针对 TOFU 对端手动接收）中，存在完全一致的代码块：
  ```rust
  self.send_to_live(peer_id_hex, encode_file_accept());
  self.transfers.insert(peer_id_hex.to_owned(), Transfer {
      name, size, transferred: 0, direction: Direction::Incoming,
      status: TransferStatus::Transferring, hash, bytes: Vec::new(), buffer: Vec::new(),
  });
  if size == 0 { self.finish_incoming_transfer(peer_id_hex); }
  ```
- **改进建议**: 提炼为私有方法 `start_incoming_transfer(&mut self, peer_id_hex: &str, name: String, size: u64, hash: [u8; 32])`。

#### 3. 发散式变化 / 巨型模块 (Divergent Change / Large Class)
- **位置**: `crates/p2p-comm-core/src/node.rs` (2189 行)
- **分析**: `node.rs` 承担了过多职责：网络轮询循环（dial/accept/session）、数据报轮询排空、文件分块发送与落盘校验、通话信令状态机、媒体生命周期管理、GUI 快照组装，以及大量集成测试用例。任何协议维度的微调都会修改该文件。
- **改进建议**: 将 `node.rs` 拆分为 `node/session.rs`（传输与会话）、`node/transfer.rs`（文件状态机）、`node/call.rs`（音视频信令与状态）、`node/snapshot.rs`。

#### 4. 基本类型偏执 (Primitive Obsession)
- **位置**: 全局 `node.rs`, `roster.rs`, `inbox.rs`
- **分析**: 大量数据结构直接以 `String`（64位 hex 串）作为 Key 和参数传递，混淆了"已校验的 PeerId"与"普通字符串"。
- **改进建议**: 可定义强类型 `struct HexPeerId(String)` 或在核心状态中统一存储 `p2p_trust::PeerId`。

#### 5. GUI 渲染帧内过度克隆 (GUI Rendering Allocations)
- **位置**: `crates/p2p-comm-core/src/node.rs:1129-1132` 与 `crates/p2p-comm-gui/src/main.rs:96`
- **分析**: 在 GUI 的 `update()` 每次重绘（tick）中均调用 `main.node.snapshot()`，而 `snapshot()` 会将选中 Peer 的所有历史消息深拷贝一遍：`inbox.messages(peer).to_vec()`。在消息量较大时，会导致每秒高达数十次的大量小对象内存分配。
- **改进建议**: `Snapshot` 仅提供消息引用的 slice 或通过版本号/自增 token 决定是否重新生成消息列表。

---

## 3. 第二维度：需求与规格符合度（Spec Axis）

对照 GitHub Issues (#1, #3, #4, #5, #6, #7, #8) 的 Acceptance Criteria 与 User Stories：

### 3.1 规格完成情况总览

| 需求模块 | 对应 Issue | 验收标准覆盖率 | 状态 |
|---|---|---|---|
| **身份设置与解锁** | #3 | 8/8 验收项覆盖（空密码、错密码、数据目录标准路径、测试覆盖） | ✅ 完美符合 |
| **拨号与侧边栏联系人** | #4 | 11/11 验收项覆盖（Hex/昵称拨号、短 ID 回退、多会话共存、测试覆盖） | ✅ 完美符合 |
| **文字消息与加密落盘** | #5 | 10/10 验收项覆盖（Argon2id+HKDF 分离密钥、时间戳、未读计数、测试覆盖） | ✅ 完美符合 |
| **文件传输到 Downloads** | #6 | 11/11 验收项覆盖（64KiB 分块、base64 编码、SHA-256 校验、HOL 限制） | ⚠️ 存在 1 处边缘行为缺陷 |
| **实时语音通话** | #7 | 12/12 验收项覆盖（Opus、48kHz/20ms、无 AEC 说明、PCM 异步拷贝） | ⚠️ 存在 1 处 UI 状态滞留 |
| **实时视频通话** | #8 | 10/10 验收项覆盖（H.264、640×480/15fps、NAL ≤ MTU-9、Texture 复用） | ⚠️ 存在 1 处硬件释放延迟 |

### 3.2 发现的具体规格与实现差异

#### 缺陷 1: 接收同名文件静默覆盖 Downloads 本地文件
- **规格出处**: Issue #1 User Story 39: *"As a 接收方, I want 完整文件落到系统 Downloads, so that 我知道去哪找"*
- **实现代码**: `crates/p2p-comm-core/src/node.rs:758-763`
  ```rust
  let ok = sha256(&xfer.buffer) == xfer.hash
      && std::fs::write(
          self.download_dir.join(safe_filename(&xfer.name)),
          &xfer.buffer,
      )
      .is_ok();
  ```
- **问题分析**: `safe_filename` 成功防御了路径穿越攻击（如 `../`），但直接调用 `std::fs::write` 会**无条件覆写** Downloads 下已存在的同名文件。如果对端传输常见文件名（如 `test.zip`, `notes.txt`），接收方本地原有文件将被直接破坏。
- **建议修复**: 若文件已存在，自动附加自增后缀（如 `file (1).ext`），避免静默覆盖。

#### 缺陷 2: 通话拒绝/超时状态缺乏清除机制 ("Call declined." 永久驻留)
- **规格出处**: Issue #1 User Story 45 & Issue #7 AC: *"对方拒绝或超时后主叫回到聊天视图并看到结果, so that 知道没打通"*
- **实现代码**: `crates/p2p-comm-core/src/node.rs:455-459` 与 `crates/p2p-comm-gui/src/main.rs:489-495`
- **问题分析**: 当呼叫被对端拒绝或 30 秒超时后，`call_result` 被设置为 `Some(CallResult::Rejected)` 或 `TimedOut`。在 UI 界面上会显示红色的 `"Call declined."` 或 `"Call timed out."`。但在用户切换联系人、打字发送新文字、或点击界面时，该状态**从未被清除**，导致红字永久留在聊天栏顶部，直到该会话下一次发起或接收通话。
- **建议修复**: 在切换联系人、发送文字或点击关闭按钮时，将 `call_result` 重置为 `None`。

#### 缺陷 3: 多会话切换时无法在当前视图挂断后台通话
- **规格出处**: Issue #1 User Story 10 & 48: *"切换聊天视图时不断开对应 Session... 点挂断发送 CallEnd 并停采集/播放"*
- **实现代码**: `crates/p2p-comm-gui/src/main.rs:450-466`
  ```rust
  Some(call) if call.peer_id_hex == peer => {
      // 渲染 "Hang up" 按钮
  }
  Some(_) => {
      ui.label("Busy on another call");
  }
  ```
- **问题分析**: 用户在与 Peer A 通话时切换到 Peer B 发送文字，Peer B 的顶部仅显示不可交互的 `"Busy on another call"` 提示，而没有挂断按钮。用户若要挂断通话，必须先在侧边栏找到并切回 Peer A 才能点击挂断。
- **建议修复**: 当存在后台通话时，在顶部提供全局快捷挂断入口或在提示旁增加挂断按钮。

---

## 4. 范围控制与蔓延检查 (Scope Creep Analysis)

项目严格遵守了 `CONTEXT.md` 和 `docs/spec-v1.md` 中的范围控制界限：
- 未引入未授权的移动端或 macOS 平台代码（已通过 ADR-0001 延迟至后续 spec）。
- 未引入回声消除（AEC）复杂实现，文档如实告知「请用耳机」。
- 未实现断点续传（严格按 v1 说明：断开连接即 TransferStatus::Failed）。
- 未私自实现多流支持，忠实遵守单条 QUIC 可靠流造成的 HOL（队头阻塞）约束。
- 零多余抽象，没有无意义的 factory 或 trait 包装。

---

## 5. 审查结论与行动建议

### 结论
项目整体工程质量高、协议实现严谨、测试覆盖完备，符合合并至主线及发布交付的标准。上述发现项属于边角健壮性与用户体验细节问题，不影响核心通信主链路。

### 建议修改优先级
1. **P1 (立即修复 - 数据安全)**: 在 `node.rs` 的 `finish_incoming_transfer` 中加入同名文件冲突重命名逻辑，避免破坏用户 Downloads 目录原有文件。
2. **P2 (次要修复 - 体验优化)**: 在 GUI / Core 中为 `call_result` 添加清除/超时自动隐藏机制；并在切换联系人时允许跨会话挂断当前通话。
3. **P3 (后续重构 - 代码健康)**: 
   - 优化 `LiveMedia::drop` 对摄像头线程的释放逻辑，避免快速重连时设备被占用。
   - 将 2189 行的 `node.rs` 拆解为会话、传输、通话三个独立子模块。
   - 优化 GUI tick 下 `Snapshot::messages` 的全量克隆开销。

---

**报告编写模型 (Review Model)**: `gemini-flash-3.8`  
**时间**: 2026-09-07
