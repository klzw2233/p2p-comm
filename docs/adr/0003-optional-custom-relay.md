# Optional custom relay via CLI

- Status: accepted
- Date: 2026-09-12

v1 把自建 relay 划在范围外，endpoint 写死 `RelayConfig::n0_public()`。P2PCore 已经提供 `custom` / `disabled`；p2p-comm 没接。用户要可选自建，默认仍走 n0。规格：[issue #48](https://github.com/klzw2233/p2p-comm/issues/48) / [spec-cli-flags](../spec-cli-flags.md)。

决定：用启动参数接线，不进设置页。`--relay <url>` 可重复；`--no-relay` 关掉；都不写 = n0。`Endpoint::bind` 和 `DialHints` 必须同一组 URL，只改 bind 会把超时映射成 `RelayUnreachable`。改了要重启——绑定发生在 `Node::start`，热切换是另一件事。

不把 relay 放进设置页：改了看起来生效、进程其实还用旧 endpoint，比 CLI 更假。
