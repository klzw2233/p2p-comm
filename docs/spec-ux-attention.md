# Spec: attention sounds, settings, dial/copy, call timeout

GitHub: [issue #49](https://github.com/klzw2233/p2p-comm/issues/49)。grilling 2026-09-12。姐妹票：[`spec-cli-flags.md`](./spec-cli-flags.md) / [#48](https://github.com/klzw2233/p2p-comm/issues/48)。

窗口在后台时来电/文件/文字/入站 Session 既不响也不闪。入站邀请无超时。侧边栏断线 Peer 只能抄 ID 回 Dial 框。没有 Copy 自己的 Peer ID。没有设置。

## 注意力

| 事件 | 声 | 闪 |
|---|---|---|
| 来电（含已聚焦） | `ringtone.wav` 循环到接/拒/取消/90s | Critical，点到窗口停；铃不因聚焦停 |
| TOFU 文件弹窗（含已聚焦） | `file.wav` 一声 | Critical |
| 新文字，仅未聚焦/最小化 | `message.wav` 一声 | Informational |
| 入站 Session（对方拨入），仅未聚焦；自己拨通的不提示 | 复用 `message.wav` | Informational |
| Verified 自动收、通话结束、文件传完、Untrusted 静默拒、出站拨通 | 无 | 无 |

- 来电铃响时不插播短音。
- Active 通话：短音全关；TOFU 文件仍闪 + 弹窗。
- 短音未播完又来：切到新的，不排队。
- Windows Critical = 闪到点窗口；Informational = 闪几下。Linux 能闪就闪（X11 urgency）；Wayland 经常没反应。无托盘、无气泡、无 Dock 徽章。
- `ViewportCommand::RequestUserAttention`（库已有，app 从未调用）。

## 声音

三套：来电循环 `ringtone.wav`（语音/视频同一套）；文字/入站 Session `message.wav`；TOFU 文件 `file.wav`。不要搬微信/QQ 音。

查找顺序：

1. 编译期 `include_bytes`：仓库 `sounds/` 里有对应 wav 就编进二进制（`build.rs`：没有文件也能编）。
2. 可执行文件旁 `sounds/`。
3. `{data_dir}/sounds/`。

都没有 = 静音，闪还在。独立于通话 cpal（通话输出只在 Accept 后才存在）。循环铃中间留间隔；不因窗口聚焦而停。

## 设置

侧边栏 "Chats" 旁齿轮 → **整页**设置（顶栏返回）。再点齿轮或返回回到聊天。来电/文件弹窗仍盖在设置上。

只两项，写入数据目录（`settings.json`），重启仍在：

- 提示音总开关（默认开）
- 任务栏闪烁总开关（默认开）

无标签、无滑条、无音量、不按事件拆开关、不放 relay、不放改密。

## Copy / 双击拨号

- `Your Peer ID` 旁文字按钮 `Copy` → 复制 64 位 hex → 约 1 秒显示 `Copied`。不要 emoji 图标。
- 侧边栏：单击仍只选中；**双击**未连接项 = 拨号；已连接忽略。不把 ID 填进 Dial 框。现有 Retry 保留。

## 通话超时与文案

`INVITE_TIMEOUT` 出站+入站都改成 **90 秒**。硬编码。超时发 `CallEnd`，不是 `CallReject`。TOFU 文件弹窗仍不超时。被叫弹窗 Hang up = Reject。

都不进 JSONL。被叫**选中该 Peer** 才出聊天红字，不切过去。info 日志一行见 CLI 票。未接不加 sidebar unread。接通后再挂：聊天不补系统行。

| 怎么结束 | 主叫聊天 | 被叫聊天 | 被叫算未接 |
|---|---|---|---|
| 被叫 Reject | `"Call declined."` | 无 | 否 |
| 被叫弹窗 Hang up | `"Call declined."` | 无 | 否 |
| 主叫振铃时挂断 | `"Call canceled."` | `"Call canceled."` | 是 |
| 90s 超时 | `"Call timed out."` | `"Call timed out."` | 是 |
| 振铃时 Session 掉线 | `"Connection lost."` | `"Call canceled."` | 是 |
| 已经接通再挂 | 无新字 | 无 | 否 |

入站 Session **不**加同意门。第一次连上仍是 TOFU。Verified GUI 仍走不到。

## 范围外

托盘/气泡/徽章、音量滑条、按事件开关、联系人页、信任验证/SAS、改密、设置页配 relay、第二个 native 窗口、Mac 通知中心（#42 范围外）。

## 以后另票（已记，本票不做）

- 联系人页：搜索、按名称、按昵称加入时间倒序。`nicknames.json` 现在没有时间戳。
- 信任验证：把 Verified 做进 GUI（P2PCore 已有 `sas` / `mark_verified` / `introduce`）。入站连接同意门。
- 改密：`FileKeyStore::change_password` 在；`ChatKeys` 必须一起迁，否则 JSONL 解不开。

## 验收

- 事件表的声/闪行为。
- 缺 wav 能编、能闪、静音。
- Active 通话时短音关。
- 设置两项落盘，重启仍在；弹窗盖设置页。
- 双击未连接才拨；Copy → Copied。
- 90s 超时；canceled / timed out / declined 文案表。
- 未接红字不进 JSONL、不切会话、不加 unread。
- Linux + Windows CI 绿。
