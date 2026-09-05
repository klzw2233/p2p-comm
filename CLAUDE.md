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

## 测试要求

- 单元测试: 核心逻辑必须有测试覆盖
- 集成测试: 挂在 `p2p-comm-core`（无 GUI 依赖）
- 真实设备测试: 通过朋友异机验证（同机 Win10 + Ubuntu VM 抢设备不可靠）
- CI: 保证 `cargo build` / `cargo test` 在 Linux 和 Windows 上编译通过

## CI 配置

GitHub Actions 矩阵:
- `ubuntu-latest`
- `windows-latest`

需要配置 secret: `P2PCORE_TOKEN`（用于拉取私有 P2PCore 依赖）

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
- **Relay**: 暂用 `n0.computer` 公共 relay
