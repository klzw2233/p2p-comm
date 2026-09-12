# HANDOFF.md

## 项目状态

**当前阶段**: v1 功能已完成。#41 Win10 异机：拨号/文字/文件/语音/视频已通。中文方框：egui 默认无 CJK，启动时加载系统字体（微软雅黑 / Noto CJK）。

**最新动态** (2026-09-11):
- issue #34 Step 1+2 已合入：`PeerIdHex` (#36) + `PeerState` (#37)
- issue #34 仍 OPEN：可选 Step 3 `Transfer::try_enqueue_chunk` 未做
- 全项目 code review 完成（Standards + Spec 两轴，2026-09-10）
- 架构文档: [docs/architecture.md](./docs/architecture.md) / [docs/module-design.md](./docs/module-design.md)

栈评估（2026-09-06）: [notes/2026-09-06-stack-architecture-review.md](./notes/2026-09-06-stack-architecture-review.md)

## 快速上手

```bash
# 克隆仓库
git clone https://github.com/klzw2233/p2p-comm.git
cd p2p-comm

# 编译
cargo build

# 运行（启动后输入身份密码进入主界面）
cargo run -p p2p-comm-gui

# 同机两个身份（测拨号）
cargo run -p p2p-comm-gui -- --profile alice
cargo run -p p2p-comm-gui -- --profile bob
```

Win10 GUI：Actions → Win10 GUI → Run workflow，下载 artifact。

发布：`git tag vX.Y.Z && git push origin vX.Y.Z`，Actions 编 Linux/Windows 并挂到 GitHub Release。

## 架构要点

详见 [docs/architecture.md](./docs/architecture.md) 和 [docs/module-design.md](./docs/module-design.md)。

**核心原则**:
- **workspace 两 crate**: `p2p-comm-core`（无头核心）+ `p2p-comm-gui`（eframe 0.30 前端）
- **P2PCore 依赖**: git 依赖 `main`（数据报 API 已合入；`RelayConfig::n0_public()` 显式 opt-in）
- **平台**: Linux + Windows 10，CI 矩阵覆盖两平台编译
- **数据目录**: 自动使用平台标准位置（`dirs` crate 6.x）
- **身份密码**: 启动弹窗输入，v1 不做钥匙串。封装走 P2PCore `FileKeyStore`（Argon2id + ChaCha20-Poly1305）
- **多会话**: 侧边栏昵称列表，可同时和多个 Peer 聊天

**测试 seam**: Node 公开 API + 假 Session（test_node + 字节 sink）

## 已锁定决定（ADR）

- [ADR-0001](docs/adr/0001-defer-macos.md): macOS 不并进 issue #1；#1 做完文件/语音/视频后再开独立 spec。协议真相仍在 issue #1。

## 范围边界

**包含**:
- 文字 + 文件 + 语音 + 视频通话
- 本地昵称表
- 加密聊天记录落盘

**不做**:
- macOS（#1 之后另开 spec，见 ADR-0001）
- 移动端
- 回声消除（文档写明用耳机）
- 设备选择（v1 只抓默认设备）
- 断点续传
- 多设备聚合展示
- 线上交换 profile
- 第二条可靠流（P2PCore Session 只暴露一条；传文件会堵住文字/信令）

## 待办

1. ~~等 P2PCore PR #22 合入 main~~ 已切到 `main`
2. ~~开第一张 spec issue~~ issue #1
3. ~~搭建 CI~~ `.github/workflows/ci.yml`
4. ~~按 issue #1 实现（顺序：身份解锁 → 拨号/侧边栏 → 存储/文字 → 文件 → 语音 → 视频）~~
5. ~~v1 审查修补~~ issue #19 (已关：#20–#25)
6. ~~全项目 code review~~ 完成 (2026-09-10)
7. **Spec 修复**: issue #33 (补充测试覆盖与映射确认)
8. **(可选) 架构改进**: issue #34 Step 1+2 已合入；剩可选 Step 3 `Transfer::try_enqueue_chunk`
9. 朋友异机验收
10. macOS：#1 完成后再开独立 spec（ADR-0001）

## 已知约束

- **同机测试不可靠**: Win10 宿主 + Ubuntu VM 同时跑会抢摄像头/麦克风，验证以朋友异机测试为准
- **Relay**: v1 用 `RelayConfig::n0_public()`。n0 公共 relay 是 hobby：无 SLA、有限速、能看见连接元数据（IP/时长/流量）。直连失败时视频会卡。生产需自建或付费 relay
- **视频数据报**: DATAGRAM 不能分片。编码必须把 slice 卡在 `max_datagram_size() - 9` 以内；丢包花屏是 v1 可接受行为。`max_datagram_size` 在 Session 接入时快照，不在每帧重读
- **文件 HOL**: 控制流和 FileChunk 共用一条可靠流。出站一次只排队一块 64KiB；文字/挂断等当前块写完，不是等整份文件
- **H.264**: `openh264` crate 默认编译 Cisco 源码。Cisco 的 MPEG LA 覆盖只针对 **它分发的预编译二进制模块**，不自动覆盖自编译。本仓库是私人、不发布、非商用工具，实际风险低，但不是法律结论

## 术语

见 [CONTEXT.md](./CONTEXT.md)
