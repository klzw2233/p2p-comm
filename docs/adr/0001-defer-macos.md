# 0001 Defer macOS until after spec #1

v1 平台保持 Linux + Windows 10。macOS 客户端不并进 issue #1。

架构已经是无头 core + eframe native，应用代码没有 `cfg(target_os)` 设备分支。Mac 是矩阵问题（nokhwa `input-avfoundation`、CI/`release.yml`、文档、分发时的 `.app` + TCC），不是换栈。把 Mac 塞进 #1 会扩大验收（还要一台 Mac）并改已经在执行的 63 条 user story。

#1 做完文件 / 语音 / 视频后再开 issue #2 做 Mac。实现 #1 媒体时继续走 cpal/nokhwa 默认设备，不要写 Linux/Windows 专用采集。CI 加 `macos-latest` 只保证能编、不当 #1 验收，也不算开 Mac spec。
