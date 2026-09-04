# Phase 1：Frame 热路径与基础设施优化

## 目标

先解决当前真正的热路径与基础设施问题，在完全不依赖未来 WASM 的情况下获得性能和内存收益。

---

## 1. VideoFrame payload 共享化

### 当前问题

如果视频帧仍以 `Vec<u8>` 深复制：

```text
scrcpy reader
→ FrameCache
→ snapshot
→ screenshot
→ WebRTC
```

在大 GOP 下可能出现明显的内存复制和峰值。

### 改造

优先：

```rust
bytes::Bytes
```

备选：

```rust
Arc<[u8]>
```

目标：

- Frame clone 只增加引用计数
- GOP snapshot 不复制完整 payload
- WebRTC fanout 尽量共享 payload

---

## 2. FrameCache 增量维护大小

不要每次遍历整个 GOP 求和。

维护：

```rust
gop_bytes: usize
```

新增 frame：

```text
gop_bytes += frame.len
```

淘汰 frame：

```text
gop_bytes -= removed.len
```

---

## 3. ffmpeg 输入流式写

避免：

```text
config + GOP
→ 拼接新 Vec
→ ffmpeg stdin
```

改成：

```text
write(config)
write(frame1)
write(frame2)
...
```

减少一次大块 buffer 申请与复制。

---

## 4. DB worker 调用侧异步化

保留当前：

- 单 DB worker
- WAL
- 有界队列
- 日志批量刷盘

不要为了“异步”换 ORM。

只把 async 调用侧的同步 reply：

```text
std recv
```

逐步替换为：

```rust
tokio::sync::oneshot
```

目标：避免 Tokio worker 被同步等待阻塞。

---

## 5. 通用文件能力从 scripts 模块移出

抽出：

```text
core/fs/
├── atomic_write
├── safe_name
├── content_version
└── archive_validation
```

后续供：

- Keymap
- AppPackage
- Extension installer
- User Override
- Registry downloader

复用。

---

## 6. 不在本阶段做的事情

暂不：

- 引入持久 ffmpeg decoder
- 引入 libavcodec binding
- 引入 GPU
- 重写 matcher
- 引入 OpenCV

这些都必须以 benchmark 为依据。

---

## 验收标准

- 现有功能行为完全兼容
- Frame/GOP 深复制明显减少
- screenshot/find P95 不回退
- 内存峰值下降或至少不升
- DB async handler 不再同步阻塞等待 DB worker
- Core 通用 fs 能力不再由 scripts 模块提供

## 回滚点

这一阶段所有优化应可以独立 revert，不影响后续架构阶段。
