# HANDOFF.md

## 项目状态

**当前阶段**: 脚手架搭建完成，待开第一张 spec ticket

## 快速上手

```bash
# 克隆仓库
git clone https://github.com/klzw2233/p2p-comm.git
cd p2p-comm

# 编译（需要配置 P2PCORE_TOKEN 环境变量以拉取私有 P2PCore 依赖）
cargo build

# 运行（GUI 占位界面）
cargo run -p p2p-comm-gui
```

## 架构要点

- **workspace 两 crate**: `p2p-comm-core`（无头核心）+ `p2p-comm-gui`（eframe 前端）
- **P2PCore 依赖**: git 依赖指向 `feat/datagram-api` 分支（PR #22），该分支提供数据报 API
- **平台**: Linux + Windows 10，CI 矩阵覆盖两平台编译
- **数据目录**: 自动使用平台标准位置（`dirs` crate）
- **身份密码**: 启动弹窗输入，v1 不做钥匙串缓存
- **多会话**: 侧边栏昵称列表，可同时和多个 Peer 聊天

## 已锁定决定（ADR）

暂无（脚手架阶段）

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

## 待办

1. 等 P2PCore PR #22 合入 main，切换 git 依赖到 main 分支
2. 开第一张 spec issue，拆分实现 tickets
3. 搭建 CI（参考 p2p-chat 的 `P2PCORE_TOKEN` 做法）

## 已知约束

- **同机测试不可靠**: Win10 宿主 + Ubuntu VM 同时跑会抢摄像头/麦克风，验证以朋友异机测试为准
- **Relay**: v1 暂用 `n0.computer` 公共 relay，生产环境需自建
- **专利**: H.264 用于私人工具不构成实际风险（不发布、不商用）

## 术语

见 [CONTEXT.md](./CONTEXT.md)
