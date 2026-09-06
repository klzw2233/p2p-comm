# HANDOFF.md

## 项目状态

**当前阶段**: issue #4 拨号 + 侧边栏认人已实现。解锁后可拨 64 hex Peer ID / 本地昵称，入站 Peer 进侧边栏（无昵称显示短 ID），昵称落盘 `nicknames.json`。尚未收发文字。

栈评估（2026-09-06）: [notes/2026-09-06-stack-architecture-review.md](./notes/2026-09-06-stack-architecture-review.md)

## 快速上手

```bash
# 克隆仓库
git clone https://github.com/klzw2233/p2p-comm.git
cd p2p-comm

# 编译（需要配置 P2PCORE_TOKEN 环境变量以拉取私有 P2PCore 依赖）
cargo build

# 运行（启动后输入身份密码进入主界面）
cargo run -p p2p-comm-gui
```

## 架构要点

- **workspace 两 crate**: `p2p-comm-core`（无头核心）+ `p2p-comm-gui`（eframe 0.30 前端）
- **P2PCore 依赖**: git 依赖 `main`（数据报 API 已合入；`RelayConfig::n0_public()` 显式 opt-in）
- **平台**: Linux + Windows 10，CI 矩阵覆盖两平台编译
- **数据目录**: 自动使用平台标准位置（`dirs` crate 6.x）
- **身份密码**: 启动弹窗输入，v1 不做钥匙串。封装走 P2PCore `FileKeyStore`（Argon2id + ChaCha20-Poly1305）
- **多会话**: 侧边栏昵称列表，可同时和多个 Peer 聊天

## 已锁定决定（ADR）

暂无独立 ADR 文件。协议真相在 issue #1。实现前若再改运输/KDF，再补 `docs/adr/`。

## 范围边界

**包含**:
- 文字 + 文件 + 语音 + 视频通话
- 本地昵称表
- 加密聊天记录落盘

**不做**:
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
4. 按 issue #1 实现（顺序：~~身份解锁~~ → ~~拨号/侧边栏~~ → 存储/文字 → GUI → 文件 → 语音 → 视频）
5. 朋友异机验收

## 已知约束

- **同机测试不可靠**: Win10 宿主 + Ubuntu VM 同时跑会抢摄像头/麦克风，验证以朋友异机测试为准
- **Relay**: v1 用 `RelayConfig::n0_public()`。n0 公共 relay 是 hobby：无 SLA、有限速、能看见连接元数据（IP/时长/流量）。直连失败时视频会卡。生产需自建或付费 relay
- **视频数据报**: DATAGRAM 不能分片。编码必须把 slice 卡在 `max_datagram_size() - 9` 以内；丢包花屏是 v1 可接受行为
- **文件 HOL**: 控制流和 FileChunk 共用一条可靠流
- **H.264**: `openh264` crate 默认编译 Cisco 源码。Cisco 的 MPEG LA 覆盖只针对 **它分发的预编译二进制模块**，不自动覆盖自编译。本仓库是私人、不发布、非商用工具，实际风险低，但不是法律结论

## 术语

见 [CONTEXT.md](./CONTEXT.md)
