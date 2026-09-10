# p2p-comm 全项目 Code Review

**日期**: 2026-09-10  
**审查者**: Claude Sonnet 5 (claude-sonnet-5)  
**方法**: 两轴并行审查 (Standards + Spec)  
**范围**: 初始提交 (1c1eaaf) 到 HEAD (5fc31da)，共 20 commits

---

## 执行摘要

对 p2p-comm 全项目进行了双轴 code review：

- **Standards 轴**: 检查代码是否符合项目文档标准 (CLAUDE.md, CONTEXT.md) + Fowler baseline smells
- **Spec 轴**: 检查实现是否符合 issue #1 及其所有子 issue (#3–#8, #19–#25) 的规格要求

**结论**: 
- ✅ 项目无文档标准硬性违规
- ✅ 功能实现基本符合 spec，v1 功能完整
- ⚠️ 发现 3 处测试覆盖缺失 + 1 处信任状态映射待确认 → **issue #33**
- ⚠️ 发现 4 处代码气味 (Fowler baseline，判断调用) → **issue #34**

---

## Standards 轴

### 审查方法

对照：
1. **CLAUDE.md**: 术语一致性、Git 工作流、测试要求、CI 配置、文档更新要求、范围控制
2. **CONTEXT.md**: 术语表、架构约束、范围边界
3. **Fowler baseline smells**: 12 种代码气味 (Refactoring, ch.3)

### Hard Violations (文档标准)

**✅ 无违规发现**

检查项全部通过：
- ✅ **测试要求**: 集成测试挂在 p2p-comm-core，无 GUI 依赖，使用内联 `#[tokio::test]` + 假 Session
- ✅ **术语一致性**: 代码中使用 `peer_id_hex`, `Session`, `MediaType` 等术语与 CONTEXT.md 一致，无漂移
- ✅ **文档更新**: ADR-0001 (macOS 推迟)、CONTEXT.md、HANDOFF.md、README 均与功能同步更新
- ✅ **Git 工作流**: commit 消息显示 `feat:` / `docs:` 前缀，PR 合并记录 (#25, #24, #30 等) 表明使用了 feature 分支
- ✅ **范围控制**: 所有已添加功能 (视频、语音、文件传输) 均在 CONTEXT.md "范围内" 列表；ADR-0001 明确记录 macOS 推迟

### Baseline Smells (Fowler，判断调用)

#### 1. Primitive Obsession (原始类型偏执)

**位置**: `node.rs`, `main.rs`, `frame.rs`  
**现象**: `peer_id_hex: String` 出现 80+ 次

- 散布在多个枚举和结构体: `Event`, `IoEvent`, `CallState`, `Snapshot`, `Node`
- 71 处 `.clone()` 调用暗示字符串作为 ID 的摩擦
- 64 字符十六进制 Peer ID 是域概念，应有自己的类型

**影响**: 
- 编译器无法防止 Peer ID 与普通字符串混淆
- 高克隆成本
- 类型不表达意图

**修复建议**: 引入 `PeerIdHex(String)` newtype，提供 `FromStr` / `Display` / `Serialize` / `Deserialize`

#### 2. Data Clumps (数据泥团)

**位置**: `node.rs:260-283` (Node 结构体)  
**现象**: 18 个字段中，5 个 `HashMap<String, _>` 总是一起旅行

```rust
struct Node {
    live: HashMap<String, LivePeer>,      // peer_id_hex → Session + 状态
    transfers: HashMap<String, Transfer>,  // peer_id_hex → 传输状态
    dgram_tx: HashMap<String, ...>,        // peer_id_hex → 数据报发送
    max_dgram: HashMap<String, usize>,     // peer_id_hex → MTU
    trust: HashMap<String, TrustState>,    // peer_id_hex → 信任状态
    // ...
}
```

**影响**:
- 添加 per-peer 数据需要散弹式修改多个 HashMap
- 五个 HashMap 的键同步容易出错

**修复建议**: 提取 `PeerState` struct，合并为 `HashMap<PeerIdHex, PeerState>`

#### 3. Shotgun Surgery (散弹手术)

**位置**: 文件传输状态管理  
**现象**: 添加文件排队功能 (issue #22) 触碰了多处

- `Transfer` 结构体字段 (`next_offset`, `status`, `buffer`)
- `enqueue_next_chunk` 函数
- `handle_send_progress` 函数
- `poll` 循环
- 多个 IoEvent handlers

**影响**: 一个逻辑变更需要修改多个分散的位置

**修复建议**: 将传输逻辑封装为 `Transfer::try_enqueue_chunk(&mut self, session: &Session) -> Result<bool>`

#### 4. Message Chains (消息链)

**位置**: `node.rs` 测试代码 (line 2597-2598)  
**现象**: 
```rust
node.snapshot().call.expect("active").peer_id_hex
```

**影响**: 
- 链式导航 `Option → CallView → String`
- 若 `CallView` 结构变化则调用点脆弱

**修复建议**: 提供测试 helper `node.call_peer() -> Option<&str>` 隐藏导航

### 未观察到的 Smells

- ✅ **Speculative Generality**: ADR-0001 明确推迟 macOS，无过早跨平台抽象
- ✅ **Middle Man**: `node.rs` 的方法 (如 `send_text`, `invite_audio`) 虽有委托但添加了验证和帧编码，不是纯转发

---

## Spec 轴

### 审查方法

对照：
- **主 spec**: GitHub issue #1 (63 user stories)
- **子 issue**: #3 (身份解锁), #4 (拨号), #5 (文字), #6 (文件), #7 (语音), #8 (视频), #19 (审查修补), #20–#25 (修补子票)

### (a) Missing/Partial Requirements (缺失/部分实现)

#### Finding 1: 未知 JSON 变体不崩溃测试缺失

**Spec**: Issue #5  
> "未知 JSON 变体不崩 (Unknown JSON variants must not crash)"

**现状**:
- ✅ `frame.rs:37` 已有 `#[serde(other)] Unknown` variant
- ✅ 现有测试显示基本解码功能正常
- ❌ **缺失**: 没有测试验证解码一个*真正未知的未来变体*会返回 `Decoded::Ignored` 而不是 panic

**示例**:
```rust
// 应有但缺失的测试
let future_json = r#"{"type": "FutureFeature", "magic": 42}"#;
assert!(matches!(decode_frame(frame), Ok(Decoded::Ignored)));
```

**优先级**: 中 (协议演进时的健壮性)

#### Finding 2: FileChunk 与 Text/CallEnd 交织测试缺失

**Spec**: Issue #22  
> "块之间能打字或挂断 (text/hangup can interleave between chunks)"

**现状**:
- ✅ `node.rs:726–742` 正确实现了块排队 (下一块不在上一块写完前占队列)
- ✅ 测试验证了排队行为 (line 1698: "second chunk must not occupy the queue before the first is written")
- ❌ **缺失**: 没有测试验证 **Text 或 CallEnd 帧真的能插在两个 FileChunk 之间**

**应有但缺失的测试**:
1. 发送 128KiB 文件 (两块)
2. 第一块写出后，调用 `send_text("插队文字")`
3. 验证 sink 上帧顺序: `FileChunk(offset=0)` → `Text` → `FileChunk(offset=65536)`
4. 同样测试 `hangup()` 插在两块之间

**优先级**: 高 (spec 明确要求，当前只测了排队不测交织)

### (b) Scope Creep (范围蔓延)

#### Finding 3: CI Node 24 bump

**Commit**: 7c4e781  
**现象**: 升级 GitHub Actions 到 Node 24 运行时

**评估**: ✅ 基础设施维护，不是功能范围蔓延

#### Finding 4: Code review 文档

**位置**: `notes/2026-09-*` (多份外部 AI 审查记录)  
**评估**: ✅ post-v1 审计产物，不在原始 spec 范围但不影响功能

### (c) Wrong Implementation (错误实现)

#### Finding 5: 信任状态映射待确认

**Spec**: Issue #6  
> "Untrusted 本端直接 FileReject"

**实现**: `node.rs:701`
```rust
if matches!(trust_state, TrustState::Unknown) {
    session.send_reliable(&encode_file_reject())?;
}
```

**问题**: 
- Spec 说 "Untrusted"
- 实现检查 `TrustState::Unknown`
- 需确认 P2PCore 是否将 `Untrusted → Unknown` 映射

**可能性**:
1. P2PCore `TrustState` 只有 `Verified` / `Unknown` 两态，`Unknown` 即 "Untrusted" → ✅ 实现正确
2. P2PCore 有单独的 `Untrusted` variant → ❌ 需改为检查 `TrustState::Untrusted`

**优先级**: 中 (需确认 P2PCore API，可能只是术语映射问题)

---

## 统计数据

### Commits 审查范围

```
5fc31da feat: idle Connected peer uses low-frequency repaint (#25) (#32)
e0e2464 feat: hangup stops camera, CallResult clears, any-view hangup (#24) (#31)
9cdeff2 Merge pull request #30 from klzw2233/feat/downloads-no-overwrite-23
df3573a fmt modified files
ee4c2ef feat: write colliding Downloads as name (n).ext (#23)
047a6d3 Merge pull request #29 from klzw2233/feat/file-chunk-pacing-22
3dcf024 feat: pace outbound FileChunk so text and hangup can interleave (#22)
7c4e781 ci: bump Actions to Node 24 runtimes (#28)
9a91e9c feat: mark disconnect as Failed so the same Peer can redial (#27)
5a1876e Merge pull request #26 from klzw2233/docs/v1-code-review
8a13741 docs: P2PCore is public, drop P2PCORE_TOKEN
375cbde docs: add grok-4.6 v1 code review
691068b review with gemini-3.8-flash
5ed5d0d Merge pull request #18 from klzw2233/feat/video-calls
098fd0b docs: mark video calls done after CI green
a2b83c1 feat: video calls (H.264 datagrams)
5f2abe0 feat: voice calls (Opus datagrams) (#17)
cff0203 Merge pull request #16 from klzw2233/docs/handoff-file-transfer
0453213 docs: mark file transfer done in HANDOFF
1bbe630 Merge pull request #15 from klzw2233/feat/file-transfer
```

共 20 commits，6673 insertions, 47 deletions, 29 files changed。

### Findings 汇总

| 轴 | 类型 | 数量 |
|---|---|---|
| Standards | Hard Violations | 0 |
| Standards | Baseline Smells | 4 (判断调用) |
| Spec | Missing/Partial | 2 |
| Spec | Wrong Implementation | 1 (待确认) |
| Spec | Scope Creep | 2 (无害) |
| **总计** | **需修复** | **3 (测试) + 1 (确认)** |

---

## 修复建议

### 立即修复 (issue #33)

**Spec 轴发现，影响健壮性和 spec 完整性**:

1. ✅ 补充测试: 未知 JSON 变体 roundtrip
2. ✅ 补充测试: FileChunk 与 Text 交织
3. ✅ 补充测试: FileChunk 与 CallEnd 交织
4. ✅ 确认信任状态映射: `Unknown` vs `Untrusted`

**预计工作量**: 1-2 天  
**风险**: 低 (只补充测试，不改实现)

### 可选重构 (issue #34)

**Standards 轴发现，影响可维护性**:

1. 引入 `PeerIdHex` newtype (消除 Primitive Obsession)
2. 提取 `PeerState` struct (消除 Data Clumps)
3. Transfer 方法封装 (消除 Shotgun Surgery)
4. 测试 helper `node.call_peer()` (消除 Message Chains)

**预计工作量**: 3-5 天 (分 3 个 PR)  
**风险**: 中 (改内部结构，需保证回归测试通过)  
**优先级**: 低于 #33，可在 v1 异机验收后进行

---

## 已生成产物

### 文档

1. **docs/architecture.md** (架构文档)
   - 系统架构总览 (GUI → Core → P2PCore)
   - 核心组件详解 (Node, Frame, Audio, Video, Storage)
   - 数据流图、并发模型、安全模型、已知约束
   - 未来演进路线图

2. **docs/module-design.md** (模块设计文档)
   - 12 个模块的职责、接口、数据结构
   - 关键状态机 (Session、文件传输、通话)
   - 实现细节 (帧格式、编解码参数、密钥派生)
   - 测试策略、性能考量、错误处理

3. **HANDOFF.md** 更新
   - 记录 review 完成状态 (2026-09-10)
   - 链接新文档
   - 更新待办事项 (issue #33, #34)

### GitHub Issues

- **#33**: Spec 修复：补充测试覆盖与映射确认 (label: `ready-for-agent`)
- **#34**: 架构改进：消除 Primitive Obsession 与 Data Clumps (label: `ready-for-agent`)

---

## 结论

**项目质量**: ✅ 良好

- v1 功能完整，符合 issue #1 规格
- 代码无严重架构问题或硬性违规
- 测试覆盖基本充分 (仅缺 3 条边缘用例)
- 文档与实现同步

**主要优点**:
1. ✅ 架构清晰: GUI/Core 分离，测试 seam 明确
2. ✅ 范围约束有效: 无功能蔓延，ADR 记录推迟决策
3. ✅ 术语一致: CONTEXT.md 术语在代码中贯彻
4. ✅ CI 覆盖: Linux + Windows 矩阵

**改进空间**:
1. 补充 3 条测试 (issue #22 交织、issue #5 未知变体)
2. 确认信任状态映射 (可能只是术语问题)
3. (可选) 重构消除 4 处代码气味

**下一步**:
1. 修复 issue #33 (Spec 轴)
2. (可选) 修复 issue #34 (Standards 轴)
3. 朋友异机验收 (v1 最后一步)

---

**审查完成时间**: 2026-09-10  
**审查耗时**: ~8 分钟 (两轴并行，各约 5 分钟)  
**Token 消耗**: 约 137k (Spec) + 136k (Standards) = 273k subagent tokens
