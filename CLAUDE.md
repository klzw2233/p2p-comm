# CLAUDE.md

接手前读 [HANDOFF.md](./HANDOFF.md) 和 [CONTEXT.md](./CONTEXT.md)。术语不要漂。

## Git 工作流

不直接 push main。

开发流程:
1. 从 main 拉 `feat/*` / `fix/*` / `docs/*` 分支
2. 实现 + 测试
3. 提交 + 推送分支
4. 创建 PR
5. CI 通过后合并
6. 删除远程和本地的开发分支
7. 同步拉取到本地

## 测试要求

- 单元测试: 核心逻辑必须有测试覆盖
- 集成测试: 挂在 `p2p-comm-core`（无 GUI 依赖）
- 真实设备测试: 通过朋友异机验证（同机 Win10 + Ubuntu VM 抢设备不可靠）；跟踪票 issue #41
- CI: 保证 `cargo build` / `cargo test` 在 Linux 和 Windows 上编译通过（`macos-latest` 属 issue #42，未实现）

## CI 配置

GitHub Actions 矩阵（`.github/workflows/ci.yml`）:
- `ubuntu-latest`
- `windows-latest`

（`macos-latest` 属 issue #42，未实现。）

手动编 Win10 GUI（`.github/workflows/win10-gui.yml`）: Actions → Win10 GUI → Run workflow，产物 `p2p-comm-gui.exe`。

推送 `v*` tag 触发 `.github/workflows/release.yml`：Linux + Windows `--release` 编 GUI，打包 tar.gz + sha256，创建 GitHub Release。Mac `.app` zip 属 issue #42。

## 文档要求

影响以下内容时需同步更新文档:
- 外部接口 → 更新 README.md
- 架构决策 → 写 ADR（`docs/adr/*.md`）
- 术语 → 更新 CONTEXT.md
- 交接信息 → 更新 HANDOFF.md

## 范围控制

严格遵守 CONTEXT.md 中的"范围"章节。

超出范围的特性需:
1. 在 issue 中说明理由
2. 更新 CONTEXT.md 范围声明
3. 写 ADR 记录决策

## 已知约束

- **同机测试**: Win10 宿主 + Ubuntu VM 不可靠，只做 CI 编译验证 + 朋友异机测试
- **设备**: v1 只抓系统默认设备，不做选择界面
- **回声消除**: 不做，README 写明"请用耳机"
- **Relay**: 默认 P2PCore `RelayConfig::n0_public()`。可选 `--relay` / `--no-relay`（spec-cli-flags，未实现；ADR-0003）。n0 公共 relay 是 hobby（无 SLA、有限速）
- **视频数据报**: NAL 必须 ≤ `Session::max_datagram_size() - 9`；丢包花屏可接受
- **文件 HOL**: FileChunk 与文字/信令共用 Session 唯一可靠流。出站一次只排队一块 64KiB

## Agent skills

### Issue tracker

Issues live in GitHub Issues (`klzw2233/p2p-comm`) via the `gh` CLI. See `docs/agents/issue-tracker.md`.

### Triage labels

Canonical roles, same strings: `needs-triage`, `needs-info`, `ready-for-agent`, `ready-for-human`, `wontfix`. See `docs/agents/triage-labels.md`.

### Domain docs

Single-context: root `CONTEXT.md` + `docs/adr/`. See `docs/agents/domain.md`.
