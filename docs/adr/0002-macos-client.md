# 0002 macOS client (incremental spec)

- Status: accepted
- Date: 2026-09-11
- Supersedes: [ADR-0001](0001-defer-macos.md)

v1 功能在 Linux + Windows 10 上已经实现。macOS 不再是「不做」，而是一张独立增量 spec：[issue #42](https://github.com/klzw2233/p2p-comm/issues/42)。行为 = [issue #1](https://github.com/klzw2233/p2p-comm/issues/1)；#42 只写平台差。

实现曾被 [issue #41](https://github.com/klzw2233/p2p-comm/issues/41)（v1 朋友异机验收）挡住。#41 已于 2026-09-12 关闭（Win10 四项通）。本 ADR 合入时仍只改文档；Cargo / CI / `.app` 属 #42 实现。

Mac 仍是矩阵问题，不是换栈：无头 core + eframe native，应用代码继续不要 `cfg(target_os)` 设备分支。平台差放 target 依赖和打包。

锁定（grilling 2026-09-11）：

- 只出 `aarch64-apple-darwin`；最低 macOS 13 Ventura
- 未签名 `p2p-comm-gui.app` 打进 zip + sha256；手动 `workflow_dispatch` + `v*` Release
- Info.plist 英文：相机 / 麦克风 / 本地网络。Bundle ID `com.github.klzw2233.p2p-comm`
- 权限拒绝：沿用 v1 静默降级
- Intel、universal、`.dmg`、签名公证、钥匙串、设备选择、回声消除、Mac 专属 UI：范围外
