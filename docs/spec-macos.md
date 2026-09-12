# Spec: macOS client (incremental, behavior = #1)

GitHub: [issue #42](https://github.com/klzw2233/p2p-comm/issues/42)。ADR-0002。grilling 2026-09-11。

v1 平台是 Linux + Windows 10。架构已经是无头 core + eframe native，应用代码没有 `cfg(target_os)` 设备分支。Mac 不是换栈，是矩阵：nokhwa `input-avfoundation`、`rfd` 的 unix/xdg-portal 会把 Mac 编进 Linux 文件选择器、CI/`release.yml`、未签名 `.app` + TCC。

**行为 = [issue #1](https://github.com/klzw2233/p2p-comm/issues/1) / [`spec-v1.md`](./spec-v1.md)。本文件只写平台差。**

实现曾被 #41（v1 朋友异机验收）挡住。#41 已于 2026-09-12 关闭。本 spec 合入文档时仍未写产品代码。

## 用户故事

1. Apple Silicon 用户从 GitHub 下载 zip 里的 `p2p-comm-gui.app`，不必自己装 Rust。
2. 首次打开摄像头/麦克风时看到系统权限弹窗，TCC 能把权限记到这个 app。
3. 首次连 P2P 时看到本地网络权限弹窗（若系统要），LAN 发现/直连不被静默掐掉。
4. 拒绝权限后通话静默降级（与 v1 没设备时相同），程序不崩。
5. 身份/昵称/聊天记录落在 `~/Library/Application Support/p2p-comm`。
6. 选文件发送时弹出原生文件对话框，不走 Linux xdg-portal。
7. 文字/文件/语音/视频行为与 #1 相同。
8. 最低 macOS 13 Ventura + Apple Silicon 的包能打开。
9. README 写明 Gatekeeper 右键打开或去掉 quarantine。
10. CI `macos-latest` 上 `cargo build` / `cargo test` / clippy 过。
11. 手动 workflow 能打出 Mac `.app` zip。
12. `v*` Release 挂同一份 arm64 zip + sha256。
13. Linux/Windows 行为不变。

## 实现决定

**功能**

- 仍只抓系统默认设备。无回声消除、无设备选择、无钥匙串、无 Mac 专属 UI（菜单栏/通知/跳系统设置）。
- 权限拒绝：沿用 `LiveMedia` 现有 best-effort 静默降级。README 加「系统设置 → 隐私 → 摄像头/麦克风」。不改 GUI。

**采集 / 依赖**

- `nokhwa` 0.10：在 `cfg(target_os = "macos")` 打开 `input-avfoundation`。不要在 Linux/Windows 统一加这个 feature。
- `cpal` 继续 `default_host()`，不写 CoreAudio 专用分支。
- `p2p-comm-gui` 的 `rfd`：拆掉 `[target.'cfg(unix)']` + `xdg-portal`。Linux 保持 xdg-portal；macOS 走 rfd 默认/Cocoa；Windows 不变。
- 数据目录：已是 `dirs::data_dir().join("p2p-comm")`。Mac 上即 `~/Library/Application Support/p2p-comm`。只补文档，不改 API。

**打包**

- 只出 `aarch64-apple-darwin`。Intel、universal fat binary：范围外。
- 最低系统：macOS 13 Ventura。
- 产物：未签名 `p2p-comm-gui.app` 打进 `.zip` + sha256（不要 `.tar.gz` 拆坏包结构；不要 `.dmg`）。
- Bundle ID：`com.github.klzw2233.p2p-comm`。显示名：`p2p-comm`。无图标。
- Info.plist **英文**：
  - `NSCameraUsageDescription`
  - `NSMicrophoneUsageDescription`
  - `NSLocalNetworkUsageDescription`
- 裸 Mach-O 可以附带给 `cargo` 用户，但不能当作通话验收包（TCC 要 `.app`）。
- 不做 ad-hoc `codesign` 当 Gatekeeper 解决方案。README 写：右键打开，或 `xattr -d com.apple.quarantine`。
- 无 Developer ID、无公证、无钥匙串、无 Homebrew cask。

**CI / 发布**

- `ci.yml` 矩阵加 `macos-latest`（不钉 14/15）。只保证能编能测，不当 Mac 功能验收。
- 新增 `workflow_dispatch`（对标 `win10-gui.yml`）打 arm64 `.app` zip artifact。
- `release.yml` 在 `v*` 时加 `aarch64-apple-darwin` 同样的 zip + sha256。

## 测试

- CI `macos-latest`：`cargo build --all-targets` / `cargo test --all-targets` / clippy。不打开真摄像头/麦克风。
- 不在 core 里为 AVFoundation 写设备测试。
- Mac 功能验收：朋友的 Apple Silicon Mac（Ventura+）装 zip 里的 `.app`，与 Linux/Windows 再跑文字/文件/语音/视频。同机 VM 仍不算。

## 范围外

- Intel (`x86_64-apple-darwin`) 与 universal binary
- macOS 12 及以下
- Developer ID 签名、公证、staple
- `.dmg`、Homebrew cask、Sparkle/自动更新
- 钥匙串 / Touch ID / 系统密码管理器
- 设备选择、回声消除
- 菜单栏、通知中心、Dock 徽章、跳转系统设置
- 权限失败的 GUI 提示行（要做另开票）
- 把 #1 的 user story 再写一遍
- 移动端

注意力声音 / 任务栏闪烁是 [`spec-ux-attention.md`](./spec-ux-attention.md)，不在本票。Mac 通知中心仍范围外。

## 验收

- [x] #41 已关闭（实现前提）
- [ ] nokhwa 在 macOS target 打开 `input-avfoundation`，Linux/Windows feature 集不变
- [ ] rfd：Linux 仍 xdg-portal，macOS 不是 xdg-portal，Windows 不变
- [ ] `p2p-comm-gui.app` 含英文相机/麦克风/本地网络用法说明
- [ ] 仅 `aarch64-apple-darwin`；zip + sha256
- [ ] `workflow_dispatch` 能打 Mac artifact；`v*` Release 挂同一产物
- [ ] CI 矩阵含 `macos-latest` 且 build/test/clippy 过
- [ ] Linux + Windows CI 不回归
- [ ] README 写数据目录、Gatekeeper、耳机、默认设备
- [ ] 朋友 Apple Silicon Mac 与现有平台异机：文字/文件/语音/视频通过

关本票：Mac CI 绿 + 手动/Release zip 能下 + 朋友 Apple Silicon 异机四项通过 + 文档平台表已含 macOS。
