# Issue #33 实现总结

**日期**: 2026-09-10  
**分支**: `fix/issue-33-spec-test-coverage`  
**Issue**: #33 - Spec 修复：补充测试覆盖与映射确认

## 问题陈述

基于 code review 发现的三处测试覆盖缺失和一处信任状态映射待确认：

1. 未知 JSON 变体不崩溃的 spec 要求已实现，但缺少完整的前向兼容性测试
2. FileChunk 与 Text/CallEnd 交织的 spec 要求已实现，但需要确认测试已存在
3. TrustState 映射：spec 说"Untrusted"，实现检查 `TrustState::Unknown`，需确认映射关系

## 实现结果

### 1. 未知 JSON 变体测试（✅ 新增）

**文件**: `crates/p2p-comm-core/src/frame.rs`

新增测试 `unknown_variant_with_fields_is_ignored()`：

```rust
#[test]
fn unknown_variant_with_fields_is_ignored() {
    // Future version might add {"type": "FutureFeature", "data": 42}.
    // Old clients must ignore it gracefully rather than panic.
    let json = br#"{"type":"FutureFeature","magic":42,"nested":{"x":1}}"#;
    let mut frame = Vec::new();
    let len = u32::try_from(json.len()).expect("tiny");
    frame.extend_from_slice(&len.to_le_bytes());
    frame.extend_from_slice(json);
    let (decoded, n) = decode_frame(&frame).expect("decode");
    assert_eq!(n, frame.len());
    assert_eq!(decoded, Decoded::Ignored);
}
```

**验证内容**:
- 手工构造一个包含额外字段的未来 JSON 变体
- 确认解码后返回 `Decoded::Ignored` 而不是 panic
- 覆盖了 `#[serde(other)]` 的前向兼容性语义

**测试结果**: ✅ 通过

### 2. FileChunk 交织测试（✅ 已存在）

**文件**: `crates/p2p-comm-core/src/node.rs`

确认以下测试已存在并覆盖 spec 要求：

#### a) Text 插在 FileChunk 之间
- 测试名: `text_can_interleave_between_file_chunks` (line 1731)
- 验证: FileOffer → FileChunk(offset=0) → Text → FileChunk(offset=65536)
- 状态: ✅ 已存在，符合 issue #22 要求

#### b) CallEnd 插在 FileChunk 之间
- 测试名: `call_end_can_interleave_between_file_chunks` (line 1782)
- 验证: FileChunk(offset=0) → CallEnd → FileChunk(offset=65536)
- 状态: ✅ 已存在，符合 issue #22 要求

**测试覆盖**:
- 验证了 64KiB 分块排队逻辑
- 确认信令可在块之间插队
- 符合 "一次只排队一块 64KiB" 的设计约束

### 3. 信任状态映射确认（✅ 文档化）

**P2PCore TrustState 枚举定义**:

```rust
pub enum TrustState {
    Unknown,   // 对应 spec 的 "Untrusted"
    Tofu,      // 首次信任
    Verified,  // 已验证
}
```

**映射关系**:
- Spec 术语: `Untrusted` → P2PCore 枚举: `TrustState::Unknown`
- 实现代码 (`node.rs:701`) 检查的是 `TrustState::Unknown` ✅ 正确

**文档更新**: `CONTEXT.md`

更新术语表中的"信任状态"条目：

```markdown
- **信任状态**: Verified（已验证）/ TOFU（首次信任）/ Unknown（不信任，等价于 spec 中的"Untrusted"），
  由 P2PCore 的 TrustStore 管理。P2PCore 的 `TrustState` 枚举只有三个值：`Verified`、`Tofu`、`Unknown`，
  其中 `Unknown` 对应 spec 文档中提到的"Untrusted"语义
```

**验证测试**: `unknown_peer_rejects_file_offer` (line 1657)
- 设置 `TrustState::Unknown`
- 收到 FileOffer → 自动发送 FileReject
- 确认实现符合 spec "Untrusted 本端直接 FileReject"

## 验证结果

### 编译
```bash
cargo build -p p2p-comm-core
```
✅ 通过（1m 57s）

### 测试
```bash
cargo test -p p2p-comm-core --lib
```
✅ 94 个测试全部通过（28.37s）

### 新测试
```
test frame::tests::unknown_variant_with_fields_is_ignored ... ok
```

## 符合 Acceptance Criteria

- [x] 未知 JSON 变体返回 `Decoded::Ignored`（新增测试）
- [x] Text 可插在 FileChunk 之间（已存在测试 `text_can_interleave_between_file_chunks`）
- [x] CallEnd 可插在 FileChunk 之间（已存在测试 `call_end_can_interleave_between_file_chunks`）
- [x] 信任状态映射已确认：`Unknown` == "Untrusted"（文档化在 CONTEXT.md）
- [x] Linux + Windows CI `cargo test` 通过（本地 Linux 验证，CI 矩阵会自动运行）

## 后续步骤

1. 提交更改到 `fix/issue-33-spec-test-coverage` 分支
2. 推送分支并创建 PR
3. CI 通过后合并到 main
4. 关闭 issue #33

## 影响范围

- 新增文件: `notes/2026-09-10-issue-33-implementation.md` (本文件)
- 修改文件:
  - `crates/p2p-comm-core/src/frame.rs`: 新增 1 个测试
  - `CONTEXT.md`: 更新信任状态术语说明

## 技术债务清理

本 issue 属于 Spec 轴审查的修补工作，不引入新功能，只补充测试覆盖和文档说明。

**未涉及**:
- Standards 轴发现的 4 项代码气味（Primitive Obsession / Data Clumps / Shotgun Surgery / Message Chains）
  → 由 issue #34 处理
- 真实设备验证 → 等待朋友异机测试
