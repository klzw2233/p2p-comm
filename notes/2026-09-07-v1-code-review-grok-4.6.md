# v1 代码审查

- **日期**: 2026-09-07
- **审查模型**: grok-4.6
- **对照**: GitHub issue #1（权威 spec）及子票 #3–#8；`docs/spec-v1.md` 作副本
- **范围**: 当前 `main`（`5ed5d0d`，PR #18 已合入）。未读既有 v1 review 文件，判断不依赖其结论。
- **未做**: 真机双端、真实麦/摄像头、Windows 实机。CI 绿（Linux + Windows，run `34048157448`）只证明能编、能跑 core 测试。

## 结论

v1 功能票（#3–#8）在 **core 状态机 + 单测** 上基本按 spec 落地：身份解锁、昵称、加密 JSONL、文字帧、文件 TOFU 三态、一路通话、Opus/H.264 roundtrip、egui 复用同一 `TextureHandle`。GUI 也把 spec 里的窗和按钮接上了。

但 **会话生命周期和文件排队与 spec 字面不符**，而且 **GitHub 上已经没有 open issue**。#1 的异机验收（user story 63 / 验收 2–5）仍未做，HANDOFF 还写着，票却全关了。下面按会咬人的程度排。

## GitHub issues

当前 **0 张 open issue**。#1 及子票 #3–#8 全部 `CLOSED` + `ready-for-agent`。

这意味着：

- 下面这些缺陷 **没有票跟踪**。修之前应先开 issue，不要在已关闭的 #1 上追加范围。
- #1 验收 2–5（朋友异机文字/文件/语音/视频）从未被 CI 覆盖，关闭票不等于产品验收完成。HANDOFF「待办 5. 朋友异机验收」仍在。
- 没有 `bug` 标签的 open 项。本次审查若要落地，建议按条开 `bug` / `enhancement`，而不是重开 #1。

## 按票对照

| 票 | 目标 | core | GUI | 缺口 |
|---|---|---|---|---|
| #3 | 身份密码解锁 | 空密码 / 错密码不重建 / 同密码同 ID / 平台数据目录：有测 | 密码窗 → 主界面显示 64 hex | 无钥匙串（范围内不做） |
| #4 | 拨号 + 侧边栏 + 昵称 |  hex/昵称解析、短 ID、增改删落盘：有测 | 拨号、右键昵称、切换视图 | **断线后仍显示 Connected，且无法重拨**（见 F1） |
| #5 | 文字 + 加密记录 | 帧编解码、未知变体忽略、JSONL 往返/隔离/错密钥：有测 | 发送、失败标记、未读计数 | 时间只显示 UTC `HH:MM:SS`，无日期 |
| #6 | 文件 → Downloads | Verified 自动 / TOFU 弹窗 / Unknown 拒 / 64KiB+base64 / SHA-256 成败 / 断开失败 / 零字节：有测 | 进度条、TOFU 窗、rfd 选文件 | **整文件一次入队，不是「当前 chunk」**（F2）；产品里到不了 Verified（F6） |
| #7 | 语音 | invite/accept/reject/end、Untrusted 拒、占线第二路、通话中文字、Opus roundtrip：有测 | Voice 按钮、弹窗、挂断 | 接通后若无设备，状态仍是 Active（静音通话） |
| #8 | 视频 | AudioVideo 信令、slice ≤ MTU-9、合成帧 roundtrip、复用 TextureHandle | 顶栏画面、同意前不开相机 | **挂断不 join 采集线程，指示灯不保证灭**（F3） |

#1 范围外（macOS / AEC / 设备选择 / 续传 / 第二条流 / 自建 relay）代码没偷偷做，符合 ADR-0001。

## 缺陷

### F1 — 断线后 roster 仍是 Connected，无法重拨（正确性）

`handle_disconnect` 清了 `live` / `dgram_tx` / 通话，**不改** `Roster`。`Roster` 没有 disconnected 路径；`connect_failed` 对已 Connected 的 Peer 直接 return。

```771:786:crates/p2p-comm-core/src/node.rs
    fn handle_disconnect(&mut self, peer_id_hex: &str) {
        // ...
        self.live.remove(peer_id_hex);
        self.dgram_tx.remove(peer_id_hex);
        // 没有 roster 状态更新
    }
```

```468:475:crates/p2p-comm-core/src/node.rs
        match self.roster.status(&hex) {
            Some(PeerStatus::Connected | PeerStatus::Connecting) => {
                self.roster.select(&hex);
                return Ok(());
            }
```

**失败场景**: 已连接 Peer 掉线 → 聊天区仍写 "Connected" → 用户再 Dial 同一 ID，`dial()` 当已连上，只切视图，**不发** `Command::Dial`。必须重启进程才能再连。#4 AC「连接失败…可重试」只覆盖首次失败；运行中掉线是同一种用户动作，现在走不通。

### F2 — 文件把全部 chunk 一次塞进无界队列，违反「chunk 之间可插文字」（正确性 / spec）

#6 / #1：HOL 接受的是「等 **当前** 64KiB chunk 写完」；user story 41 按 **chunk 之间** 可以插文字理解。

`send_chunks` 把整文件所有 `FileChunk` 同步 `unbounded_channel.send`，然后才返回。`session_loop` 再逐帧 `session.send().await`。之后才 `send_text` / `hangup` 的帧排在 **整文件** 后面，不是当前块后面。

```1380:1400:crates/p2p-comm-core/src/node.rs
    for chunk in bytes.chunks(CHUNK) {
        let frame = encode_file_chunk(offset, chunk);
        if tx.send(frame).is_err() { /* ... */ return; }
        offset += chunk.len() as u64;
    }
```

大文件期间挂断/打字会等整个文件出站，内存里同时堆着全部 base64 JSON（64KiB → ~87KiB/块）。core 测试只断言「两块都出现在 channel」，没有断言块与块之间能插 `Text`/`CallEnd`。

### F3 — 挂断不 join 视频线程，相机指示灯不保证灭（#8 AC）

#8：「挂断后画面消失、相机释放（指示灯灭）」。画面：GUI 在 `call.is_none()` 时丢 `video_tex`，这一半有。相机：

```127:138:crates/p2p-comm-core/src/audio_io.rs
        if let Some(h) = self.video.take() {
            // ponytail: detached join so hangup isn't blocked on camera.frame()
            let _ = h;
        }
```

`capture_loop` 里 `camera.frame()` 阻塞。`stop` 是 `Relaxed`，只在两次 `frame()` 之间和 `Err` 时看。线程被 detach 后，设备可能一直占到下一次 `frame()` 返回——同机验收本来就不可靠，这条会让「指示灯灭」在真机上变成运气。音频 encode 线程有 `join`，视频没有。

### F4 — 已连接时 GUI 每帧 `request_repaint`（性能）

```104:112:crates/p2p-comm-gui/src/main.rs
                if changed
                    || transferring
                    || in_call
                    || snap.pending_offer.is_some()
                    || snap.pending_invite.is_some()
                    || snap.selected_status == Some(PeerStatus::Connecting)
                    || snap.selected_status == Some(PeerStatus::Connected)
                {
                    ctx.request_repaint();
```

选中一个 **Connected** Peer 就全速重绘。文字聊天不需要；视频 15fps 也不需要。叠加 F1 后，断线仍是 Connected，空转不会停。`Connecting` / 传输中 / 通话中才值得立刻刷。

### F5 — CI / Win10 GUI 的 `P2PCORE_TOKEN` 没有真正用于 git fetch（工程）

私有 git 依赖。`release.yml` 做了 `url.insteadOf` + `CARGO_NET_GIT_FETCH_WITH_CLI`。`ci.yml` 和 `win10-gui.yml` 只把 token 放进 **env**，cargo/libgit2 **不会**读 `P2PCORE_TOKEN`。当前 CI 绿，很大概率是 `~/.cargo/git` cache 命中（workflow 缓存了 git db）。`Cargo.lock` 换 rev 或 cache miss 时，Linux/Windows CI 和手动 Win10 产物可能突然拉不下 P2PCore。#1 AC「Linux + Windows CI `cargo test` 通过」建立在一条未写进 workflow 的隐式条件上。

### F6 — GUI 没有把 Peer 标成 Verified，#6 的自动接收在产品里走不到（产品 / spec）

P2PCore `TrustState::{Unknown, Tofu, Verified}`。入站未知 → TOFU。`Endpoint::mark_verified` 存在，`Node` 不暴露，GUI 无 SAS/验证入口。core 测 Verified 是 `set_trust` 注入的。真实使用只有 TOFU 弹窗和 Unknown 拒。#6 AC「Verified：入站文件自动接受」实现了，用户到不了那个状态。不是运行时崩溃，是验收口径空了。

### F7 — `SendFailed` 误伤最后一条聊天（正确性，边角）

`session.send` 失败（含 FileChunk）会 `mark_last_failed` 再断线。`mark_last_failed` 不看 `direction` / 是否刚发出的那条。先收到一条 Incoming，再发文件失败 → 对方那条被标 `[failed]`。#5 要的是「发出失败」状态。

## 测试与验收缺口

有、且对得上票的：身份、昵称、帧、JSONL、文件三态、通话信令、编解码 roundtrip。缺的：

- 无进程内双 `Endpoint` 测试（#1 允许假 Session，byte sink 算数；真实 `attach_session` / `session_loop` / datagram drain 仍是手工路径）。
- 无「chunk 之间插入 Text/CallEnd」测试 → F2 能合入。
- 无「Session 断开后 roster / 可重拨」测试 → F1 能合入。
- 无相机 drop 后 `stop_stream` 被调用的测试（真设备不进 CI，至少应对 join/stop 标志做可测 seam）。
- 异机验收未做，#1 却关了。

## 做得对的地方（不展开）

- core 零 GUI 依赖；测试挂 core。
- 聊天密钥：随机 32B salt + Argon2id → HKDF-Expand(`peer_id`) → ChaCha20-Poly1305；身份走 `FileKeyStore`。错密码不解 JSONL。
- 未知 JSON 变体 `Decoded::Ignored`，不崩。
- FileChunk `data` 是标准 base64，不是数字数组。
- `safe_filename` 去掉 `../` 和盘符。
- 视频 `max_slice_len = max_datagram_size()-9`，接入时快照；超限 NAL 丢弃。
- 全进程一路通话；Untrusted/Unknown 拒文件和通话；同意前不开麦/摄像头。
- GUI `apply_video_frame` 首次 `load_texture`，之后 `set`。
- README 写了耳机、n0 hobby relay、HOL、花屏。

## 建议（最短）

1. 开 issue 跟踪 F1–F5（至少 F1/F2/F3），不要重开 #1。
2. F1：`handle_disconnect` 把 roster 打成 Failed；`dial` 对 Failed 重发。加一个 core 测。
3. F2：`send_chunks` 改成等 `session_loop` 写完一块再入下一块，或有界 channel(1)。加「两块之间能插 Text」测试。
4. F3：hangup 时 join 视频线程（可带超时）；`frame()` 失败也要看 `stop`。
5. F4：Connected 走 `request_repaint_after(200ms)`；通话/传输再立刻刷。
6. F5：CI 和 Win10 GUI 复用 release.yml 的 git URL rewrite。
7. 异机验收单独留一张 open issue，关 #1 不等于 v1 验收完。

**Skipped**: 不在本报告里改代码。需要修哪条，指定 issue 号即可。
