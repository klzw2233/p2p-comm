# Stack & architecture review (2026-09-06)

对照：[GitHub issue #1](https://github.com/klzw2233/p2p-comm/issues/1)（`ready-for-agent` spec）、本地 workspace、crates.io / 上游 README / RFC。抓取日期：2026-09-06。

## Verdict

两 crate 切分、P2PCore Session、可靠流文字、Opus 48 kHz/20 ms、ChaCha20-Poly1305 落盘、测试只挂 core —— 对私人 Linux+Win10 v1 是对的。真正会咬人的不是 GUI 框架，是三件事：**视频 NAL 塞进不可分片的 QUIC DATAGRAM**、**文件和信令共用一条可靠流（HOL）**、**密码只用 HKDF、没有慢哈希**。egui 0.30 / 媒体 crate 可以先用；不要为了「新」去升 egui 0.36（MSRV 1.95）。n0 公共 relay 当 hobby 可以，当生产不行。

## What is sound

- `p2p-comm-core` 无头 + `p2p-comm-gui` 只画：CI 能测协议，GUI 不进测试。符合 #1。
- P2PCore git `main`（`Cargo.lock` 里 `p2p-core`/`p2p-trust` 0.1.0，`iroh` 1.1.0）。数据报已在 main，不必再跟 `feat/datagram-api`。
- 文字走长度前缀 JSON 可靠流；信令复用同一帧格式。
- Opus 48 kHz / 单声道 / 20 ms：VoIP 默认帧。
- 信任三态驱动文件/通话：Verified 自动、TOFU 弹窗、Untrusted 拒。
- 全进程一路通话：默认麦/摄像头不会抢。
- 不做 AEC / 设备选择 / 断点续传 / 群聊：范围对。
- CI 已装 `libasound2-dev` + `libv4l-dev`；cpal/nokhwa **编译**不需要真设备。

## What works but watch

- **egui 0.30 贴 640×480@15**：`Context::load_texture` + `TextureHandle`；每帧 `ColorImage` 再 `set` 全量上传。15 fps 约 1.2 MB/s 上传，私人工具够用。不要每帧 `load_texture` 新 ID。
- **nokhwa 0.10**：`input-v4l` + `input-msmf` 等于官方 `input-native` 去掉 Mac。README 示例用 `CameraIndex::Index(0)` + `RgbFormat`。openh264 要 I420/YUV；RGB→YUV 在编码前做一次。
- **cpal 0.18 在 core**：回调线程里不要做 Opus/网络。环形缓冲 → 编码线程。CI 无声卡时 **打开设备** 会失败，编解码 roundtrip 不要碰 cpal。
- **workspace `unsafe_code = forbid`**：nokhwa/openh264/cpal 内部有 unsafe，消费方 API 是 safe 的。openh264 的 `nal_unit` 实现里有 unsafe，调用方不必写。
- **公共 relay**：n0 明文说 hobby/dev、无 SLA、有限速、能看见 IP/时长/流量。两人试用可以；别当生产。
- **clippy pedantic + `-D warnings`**：第一批真实模块会吵。先在自己的模块 `allow` 具体 lint，不要关 CI。

## What should change before coding

1. **视频不要假设「一个 NAL = 一个 DATAGRAM」。** RFC 9221：DATAGRAM **不能分片**，实际上限是 path MTU（常见 ~1200 B 量级），不是 `max_datagram_frame_size` 广告值。openh264 默认一帧多 NAL，I 帧轻易超过 MTU。v1 最小补丁：编码时 `max_slice_len` 卡在 path MTU 减去头（类型+时间戳），仍然可能丢片导致花屏。更好（仍可不换 crate）：一帧一条 QUIC **单向流**，过时就 reset —— 这是 iroh 自己对实时媒体的建议，也是 MoQ/`callme` 的路。#1 写死数据报的话，至少在 spec 里写上 slice 限制 + 丢包花屏可接受。

2. **FileChunk 走同一条可靠流，和「传文件还能打字」打架。** 64 KiB 二进制 → JSON+base64 ≈ 87 KiB，和 Text/Call 排队。QUIC 多流本来就是为这个存在的。v1 最小：文件另开一条可靠流（或第二个 bidi），信令/文字留在控制流。不换 JSON 也行，先把 HOL 切开。

3. **磁盘加密：HKDF(password) 太快。** RFC 5869 的 HKDF 假定 IKM 已经有熵；登录密码没有。应用层应先 Argon2id（或至少 scrypt）再 HKDF-Expand。salt 用随机 32 B 存在数据目录，不要字面量 `"chat-history"`。`info = peer_id` 做域分离可以留。空密码拒绝保留。

4. **文档漂移（本分支已清）:** `CONTEXT.md` / `HANDOFF.md` / `docs/spec-v1.md` / issue #1 已对齐 `main`、cpal 0.18、Argon2id、视频 MTU、文件 HOL。HANDOFF 待办三条已标完成。

5. **H.264 专利：HANDOFF「私人工具无风险」说重了。** Cisco 只对 **它自己分发的预编译二进制模块** 覆盖 MPEG LA 池；`openh264` crate 默认 `source` 是 **编译捆绑源码**，费用在使用者。私人、不发布、非商用实际风险低，但不要写成法律结论。

其余（egui 0.30、cpal 0.18、opus 0.4、dirs 6、tokio 1.x）**先别升**，见下表。

## Per-crate version check

| Crate | Cargo.toml | Cargo.lock | crates.io latest (2026-09-06) | 结论 |
|---|---|---|---|---|
| egui / eframe | 0.30 | 0.30.0 | **0.36.1** (2026-08-07) | **钉 0.30。** 0.36.1 声明 rust 1.95；workspace MSRV 1.80。升 0.36 等于先升 toolchain。 |
| nokhwa | 0.10 | 0.10.11 | 0.10.11 (2026-05-15) | 已是 0.10 最新。git 主分支 Cargo.toml 写 0.11.0 未发 crates.io。feature：`input-v4l`+`input-msmf` 正确。 |
| openh264 | 0.9 | **0.9.8** | 0.9.8 (2026-08-08) | 0.9 线当前。0.9.0 曾标 rust 1.88，0.9.8 标 1.85。changelog：0.9 改 edition 2024。 |
| opus | 0.4 | 0.4.0 | 0.4.0 (2026-08-23) | 最新。SpaceManiac/opus-rs，safe 绑 libopus，构建要 cmake+C 编译器。 |
| cpal | **0.18** | 0.18.2 | 0.18.2 (2026-08-16) | 已是最新。#1/旧 spec 写 0.16 是错的。MSRV 1.85（ALSA/WASAPI）。 |
| dirs | 6.0 | 6.0.0 | **7.0.0** (2026-09-05，今天) | **先留 6。** 7 刚发，XDG/Known Folder 行为 6 已够。 |
| tokio | 1.42 | **1.53.1** | 1.53.1 | semver 已解析到最新 1.x。toml 写 1.42 无害。 |
| serde | 1.0 | 1.0.229 | 1.0.229 | 最新。 |
| bytes | 1.9 | 1.12.1 | 1.12.1 | 最新 1.x。 |
| anyhow | 1.0 | (lock) | 1.0.104 | 最新 1.x。 |
| chacha20poly1305 | （未入 toml） | — | 0.11.0 | #1 指定，实现时加。 |
| hkdf | （未入 toml） | — | 0.13.0 | 实现时加。 |
| argon2 | （未入 toml） | — | 0.6.0 | **建议加**，见加密。 |

## Deep dives

### 2. egui 视频贴图

egui README：纯 Rust 立即模式 GUI；eframe 官方原生框架（Web/Linux/Mac/Windows）。源码 `Context::load_texture(name, ImageData, TextureOptions) -> TextureHandle`，示例是 **加载一次** 再 `ui.image`。`epaint::TextureManager::set` 支持整张或局部更新。

640×480 RGBA ≈ 1.23 MB/帧 × 15 ≈ 18 MB/s 进 GPU，对桌面 wgpu 很轻。坑：每帧新建 `TextureHandle` 会泄漏/打爆纹理表；应用层持有一个 handle，每帧 `set`。Windows 走 eframe 默认 wgpu，#1 分辨率无需自写 glow。

### 3. nokhwa 0.10

README 功能表：`input-native` = V4L2 Linux + MSMF Windows + AVFoundation Mac。默认 feature **什么后端都不开**。本仓库只开 v4l+msmf，避开 Mac，对。示例：`CameraIndex::Index(0)` 当「第一个/默认」。输出可 `decode_image::<RgbFormat>()`。openh264 输入是 YUV 4:2:0（Cisco：planar YUV）。中间要做 RGB→I420，crate 自带转换。

nokhwa 声明只保证 latest stable rustc。0.10.11 仍在 crates.io 维护（2026-05）。git 上的 0.11 不要跟。

### 4. openh264 0.9 / 数据报大小

[openh264-rs](https://github.com/ralfbiedert/openh264-rs) 绑 Cisco OpenH264，默认 `source` 用 `cc` 编进包。编解码 Linux/Windows x86_64 官方测过。`Encoder::encode` 返回 `EncodedBitStream`：一帧 **多个 NAL**（先 SPS/PPS，再 I/P）。`Layer::nal_unit(i)` 取出单 NAL。`EncoderConfig::max_slice_len` 可限 slice。Cisco 输出 Annex B。

RFC 9221 §5：DATAGRAM **cannot be fragmented**；上限再被 `max_udp_payload_size` 和 path MTU 砍。I 帧一个 NAL 经常 >1200 B。v1 若不改运输，必须 `max_slice_len` + 接受花屏。

640×480@15 对软件编码器很轻松（上游 bench：512² 编码约 2–4 ms 量级，有 nasm 更快）。

### 5. opus 0.4

Safe libopus 绑定。48 kHz / 20 ms / mono 是 Opus 标准 VoIP 帧（RFC 6716）。构建依赖 cmake + C 编译器。实时注意：不要在 cpal 回调里 `encode`；回调只拷贝 PCM。

### 6. cpal 0.18

RustAudio/cpal。Linux 默认 ALSA（本 CI 已 `libasound2-dev`），Windows 默认 WASAPI，最低 Win10。API：按角色取默认输入/输出、开 stream。README 自陈是 **low-level**。无设备时 `default_input_device()` 返回 None / 建流失败 —— 运行时，不是编译时。所以 core 依赖 cpal **不破坏 CI 编译**；测试必须把设备 I/O 和编解码切开。

### 7. 媒体放 core vs GUI

#1 把 nokhwa/cpal/openh264/opus 放 core 是对的：信令状态机和「合成 PCM roundtrip」必须无窗口可测。GUI 只拿解码后的 RGB 贴纹理、把按键变成 `CallInvite`。

`unsafe_code = forbid` 禁的是 **本 crate 源码** 里的 `unsafe`，不禁依赖。openh264 的 NAL 切片在 **依赖内部** unsafe。CI 无摄像头时 nokhwa 打开失败，不要在 `cargo test` 里打开。

### 8. QUIC 数据报 + n0 relay

iroh：拨公钥、打洞，失败走 relay；底层 QUIC（noq），带 stream 和 datagram。Relay 只转发 **已加密** 流量，能看见元数据。

n0 公共 relay（[Public Relays](https://docs.iroh.computer/iroh-services/relays/public.md)）：免费、无认证、无 SLA、只跟最新稳定 iroh、限速、明确 **development and hobby**。限速是 per-connection token bucket，打满会背压不是丢包。直连后限速消失。#1「暂用 n0」符合 hobby v1；文档应写「直连失败时视频会被限速/变卡」。

iroh《Using QUIC》原文：实时协议去用 DATAGRAM **多数情况下是错的** —— DATAGRAM 仍走拥塞控制且会 ACK；传统 RTP-on-datagram 在 QUIC 上往往不如设计预期。他们推荐 **每帧一条 stream，过时 reset**（MoQ）。n0 自己的音频 demo 是 [callme](https://github.com/n0-computer/callme) + `iroh-roq`（RTP over QUIC），视频直播走 [iroh-live](https://github.com/n0-computer/iroh-live) / MoQ。v1 用原始 DATAGRAM 能出声出画，但和运输层作者的建议相反。

RFC 9221 动机就是可靠流 + 不可靠实时共用一条 QUIC 连接。音频 Opus 20 ms 帧通常 <200 B，**音频走数据报合理**。视频才是问题。

### 9. FileChunk JSON+base64 + 单流 HOL

同一条双向可靠流上：Text、Call*、FileOffer/Chunk。64 KiB chunk × 4/3 base64 ≈ 87 KiB JSON。发送方写循环会堵住 CallEnd/Text，直到当前 chunk 写完（还要等流控）。#1 user story 41「传文件还能发文字」在单流下只在「chunk 之间」成立，大文件体感就是聊天卡。

YAGNI 平衡：**不要**上第二个协议；**要**第二条 QUIC 流给文件。P2PCore Session 若只暴露一条可靠流，这是对 P2PCore 的缺口，应在实现前确认能否 `open_bi` 第二条。确认不了就把文件也当「会卡住信令」写进 spec，删掉 story 41。

### 10. Crypto

#1：`HKDF-SHA256(password, salt="chat-history", info=peer_id)` → ChaCha20-Poly1305。ChaCha20-Poly1305（RFC 8439）做 AEAD 没问题。HKDF（RFC 5869）§3.1：salt 最好是 HashLen 的随机非保密值；固定字符串当 salt 合法但弱。更关键：HKDF-Extract 不是密码哈希。离线撞密码对「登录口令」几乎免费。

最小正确：

- 随机 32 B salt 存在数据目录（身份一份就够）
- `master = Argon2id(password, salt)`
- `chat_key = HKDF-Expand(master, info=peer_id)`（域分离留下）
- 每条 JSONL 行：随机 nonce + ciphertext+tag

身份密钥封装应走 P2PCore 已有 API；没有就同一套 Argon2id master 再 wrap，不要第二条密码。

### 11. core / gui 运行时

eframe `App::update` 在 GUI 线程。tokio 多线程 runtime 放在 gui 启动时（或 core 持有、gui 拿 handle）是常见做法。cpal 回调线程、nokhwa 采集线程、tokio、egui **四套线程**。规则：回调/采集只入队；编解码在 worker；GUI 只 poll 最新帧。不要在 `update` 里 `block_on` 网络。

egui 0.30 和 tokio 1.53 无已知强制冲突。n0 的 `callme-egui` 已经是 egui+音频+iroh 的先例。

### 12. clippy pedantic

workspace：`clippy pedantic = warn` + CI `-D warnings`。pedantic 会抓 `must_use_candidate`、文档句号、太大的 fn。依赖不 clippy；本仓库模块会。openh264 上游自己都 `allow(clippy::must_use_candidate)`。起步允许对 gui 占位和测试关几条具体 lint，别关 `pedantic` 整组。

## Doc drift（相对当时的 #1；本分支已清）

下表是 2026-09-06 评估时的漂移。`docs/v1-spec-align` 已把 CONTEXT / HANDOFF / spec-v1 / issue #1 / README / CLAUDE 对齐。

| 文件 | 当时过时内容 | 现在 |
|---|---|---|
| `CONTEXT.md` | P2PCore `feat/datagram-api` | git `main` |
| `HANDOFF.md` | 等 PR #22、搭 CI、开 spec | 三项完成；待办改为实现 #1 |
| `docs/spec-v1.md` | feat 分支、cpal 0.16、AES-256-GCM | 与 #1 一致 |
| issue #1 | HKDF(password)+固定 salt；单流当无 HOL；NAL=DATAGRAM | Argon2id；HOL 写明；slice≤MTU |

ADR 目录仍空。密码 KDF、文件是否第二条流、视频 DATAGRAM vs 每帧一 stream —— 这三件值得各写一篇 ADR，别埋在 issue 评论里。

## Sources

1. https://crates.io/api/v1/crates/egui — 0.36.1 (2026-08-07)
2. https://crates.io/api/v1/crates/eframe — 0.36.1；0.30.0 rust 1.80 (2024-12-16)
3. https://crates.io/api/v1/crates/nokhwa — 0.10.11 (2026-05-15)
4. https://crates.io/api/v1/crates/openh264 — 0.9.8 (2026-08-08)
5. https://crates.io/api/v1/crates/opus — 0.4.0 (2026-08-23)
6. https://crates.io/api/v1/crates/cpal — 0.18.2 (2026-08-16)
7. https://crates.io/api/v1/crates/dirs — 7.0.0 (2026-09-05)；6.0.0 (2025-01-12)
8. https://crates.io/api/v1/crates/tokio — 1.53.1
9. https://crates.io/api/v1/crates/serde — 1.0.229
10. https://crates.io/api/v1/crates/chacha20poly1305 — 0.11.0
11. https://crates.io/api/v1/crates/hkdf — 0.13.0
12. https://crates.io/api/v1/crates/argon2 — 0.6.0
13. https://github.com/emilk/egui/blob/master/README.md
14. https://github.com/emilk/egui/blob/master/crates/egui/src/context.rs — `load_texture`
15. https://github.com/emilk/egui/blob/master/crates/epaint/src/textures.rs — `TextureManager::set`
16. https://github.com/l1npengtul/nokhwa/blob/master/README.md
17. https://github.com/l1npengtul/nokhwa/blob/master/Cargo.toml — features
18. https://github.com/ralfbiedert/openh264-rs/blob/master/README.md
19. https://github.com/ralfbiedert/openh264-rs/blob/master/openh264/src/encoder.rs
20. https://github.com/cisco/openh264/blob/master/README.md
21. https://www.openh264.org/faq.html
22. https://www.openh264.org/BINARY_LICENSE.txt
23. https://github.com/SpaceManiac/opus-rs/blob/master/README.md
24. https://github.com/RustAudio/cpal/blob/master/README.md
25. https://github.com/n0-computer/iroh/blob/main/README.md
26. https://github.com/n0-computer/iroh/blob/main/iroh/src/lib.rs — RelayMode::Default = number 0
27. https://github.com/n0-computer/iroh/blob/main/iroh-relay/README.md
28. https://docs.iroh.computer/llms.txt
29. https://docs.iroh.computer/concepts/relays.md
30. https://docs.iroh.computer/iroh-services/relays/public.md
31. https://docs.iroh.computer/relays/rate-limiting.md
32. https://docs.iroh.computer/protocols/using-quic.md — DATAGRAM「misguided」；MoQ 每帧一 stream
33. https://docs.iroh.computer/protocols/streaming.md — callme / iroh-live
34. https://docs.iroh.computer/add-a-relay.md
35. https://www.rfc-editor.org/rfc/rfc9221.txt — DATAGRAM 不分片、不重传
36. https://www.rfc-editor.org/rfc/rfc5869.txt — HKDF salt
37. https://www.rfc-editor.org/rfc/rfc8439.txt — ChaCha20-Poly1305
38. https://github.com/klzw2233/p2p-comm/issues/1
39. 本仓库 `Cargo.toml` / `Cargo.lock` / `CONTEXT.md` / `HANDOFF.md` / `docs/spec-v1.md` / `.github/workflows/ci.yml`
