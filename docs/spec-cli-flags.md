# Spec: CLI flags and file logs

GitHub: [issue #48](https://github.com/klzw2233/p2p-comm/issues/48)。grilling 2026-09-12。姐妹票：[`spec-ux-attention.md`](./spec-ux-attention.md) / [#49](https://github.com/klzw2233/p2p-comm/issues/49)。

v1 启动参数只有 `--profile NAME` 和 `-h`。Relay 写死 n0。没有 logger。本票接线 P2PCore 已有的 `RelayConfig::{custom, disabled}`，并默认写 info 日志文件。

默认行为不变：不加新参数 = 仍 n0、不弹 Windows 控制台。

## CLI

手写解析（现有 `parse_data_dir` 扩成总解析）。不加 clap。

| Flag | 作用 |
|---|---|
| `--profile NAME` | 现有。数据目录 `p2p-comm-NAME` |
| `-h` / `--help` | usage 打 stderr，exit 2（现有） |
| `--relay <url>` | 可重复。自建 relay |
| `--no-relay` | `RelayConfig::disabled()` + `DialHints::none()` |
| `--debug` | 把 `p2p_comm` 日志升到 debug |

规则：

- `--relay` 与 `--no-relay` 互斥 → stderr + exit 2。
- 都不写：`RelayConfig::n0_public()` + 现有 `N0_RELAY_URLS` hints。
- 每个 `--relay` URL 必须能过 `RelayConfig::custom`（iroh `RelayUrl`）。非法 / 缺值 / 未知参数 → stderr + exit 2。
- **bind 和 dial hints 必须同一组 URL。** 只改 `Endpoint::bind` 不够（见 `node.rs` 注释：空 hints 的超时会变成 `RelayUnreachable`）。
- 改完要重启。不热切换。不进设置页。
- **不加**：`--data-dir`、`--mute`、`--log-file`、`--listen`、`--info`（默认就是 info）。
- Windows release：**不** `AllocConsole`。

## 日志

第一次在本仓库用 `tracing` + `tracing-subscriber`（lock 里已有传递依赖）。

- 默认过滤器：`p2p_comm=info,p2p_core=info,iroh=warn`。
- `--debug`：只把 `p2p_comm` 改成 `debug`。
- `RUST_LOG` 若设置：整表覆盖。
- 目录：`{data_dir}/logs/`（随 `--profile`）。
- 当天：`logs/YYYY-MM-DD.log`，**本地**日历日，同一天**追加**。
- 本次进程：`logs/latest.log`，每次启动**截断**，与当天文件写同样的行。
- 不轮转、不按体积切、不删旧文件。
- 一行：`2026-09-12 14:03:05 [INFO] Session connected peer=ab12…`
  - 本地时区，精确到秒。
  - 级别：`[ERROR]` `[WARN]` `[INFO]` `[DEBUG]`。
- 打不开日志：stderr 一行警告，GUI 照常起。

### info（默认就有）

解锁成败（不写密码）、bind、relay 模式（`n0` / `custom` 列出 URL / `disabled`）、Session 上/下及失败原因、语音/视频的拨号/接通/结束、来电接/拒/取消/超时/未接、文件 offer/接/拒/完成/失败。

不写文字正文、密码、FileChunk 字节、数据报 payload。

### debug（`--debug`）

另加帧类型、块偏移、数据报大小。仍不写正文和密码。

## 范围外

设置页配 relay、热切换、按体积轮转、Windows 弹控制台、把 iroh 默认打到 debug、聊天 JSONL 当日志。

## 验收

- 无新参数：仍 n0。
- `--relay` 可重复；bind + hints 同源。
- `--no-relay` 禁用 relay。
- 互斥或非法 URL：exit 2。
- `logs/YYYY-MM-DD.log` 追加 + `latest.log` 每次覆盖。
- `--debug` 才有 p2p_comm DEBUG 行；`RUST_LOG` 能覆盖。
- 日志打不开不阻止 GUI。
- `--help` 含新参数。
- CONTEXT / README / HANDOFF / ADR-0003 对齐。
- Linux + Windows CI 绿。
