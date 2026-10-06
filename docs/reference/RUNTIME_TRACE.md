# Runtime Trace 图片证据

Runtime Trace 为现有运行记录增加图片证据，不创建新的执行器，也不需要打开投屏页面。普通运行日志仍按现有 `log_retain_days`（默认 14 天）保存。

## 采集与真实性

- 图像 `image_id` 为独立 UUID，不能与解释器调用栈 `frame_id` 混用
- 识别完成后提交该次真正消费的帧，保留未命中、模板版本、原始匹配区域、阈值和结果；模板图片来自实际消费的冻结字节
- Android 记录所解码 GOP 快照最后一帧的接收时间，静止画面不会被标成刚刚捕获。CDP/adb 按需截图记录截图完成时间，`capture_time_basis` 明确区分二者
- `trace(false)` 只关闭自动图片记录，普通日志、步骤和错误继续保存；开关两侧各尝试保存一张边界图
- 失败记录分别使用 `error_last_consumed` 和 `error_fresh`。前者保留原始识别的时间与步骤，根错误上下文放在 `error_context`；后者只有重新抓图成功时才存在。断连等失败形成 gap，不伪造当前画面
- CDP 没有被假定为连续解码帧源，没有启动无限后台截图循环。补充观察有频率与数量限制，不能保证看见任意两次观察之间的变化
- 保存原始宽高、无损 PNG。当前 image 依赖仅启用 PNG/JPEG；WebP 需要增加 codec 特性与依赖，首版选择现有无损 PNG 以减少新增依赖。无损保存不能恢复 H.264/原始录制已损失的细节

## 队列与容量

哈希、去重、PNG 编码和文件写入在独立有界 worker 完成，输入线程只提交共享帧引用。队列满、磁盘失败、截图失败、模板过大、资源快照过大与容量停止都作为 gap 返回。相同图片在同一运行内只写一份像素载荷，多次识别仍保留各自图像关联记录。

`config.toml` 可加入：

```toml
[trace]
retain_hours = 24
global_max_bytes = 536870912
run_max_bytes = 67108864
run_max_images = 1000
queue_capacity = 4
min_observation_ms = 1000
retained_max_bytes = 268435456
retained_max_runs = 10
```

成功和失败运行图片均从完成时起保留 24 小时，后台每 30 秒及有新工作时检查。全局超限先清理最旧已结束运行；只剩活动运行时停止后续采集并显示缺口。基本日志不随图片清理。短索引随基本运行历史到期清理。

“保留记录”仅接受已完成且有图片的运行，将图片、实际模板、脚本快照、图片诊断和运行摘要复制到独立的持久目录，并受独立总容量和数量限制。普通日志的 14 天保留期不因此变更。素材包、正式模板、脚本历史与持久保留目录不会被临时 Trace 清理扫描或删除。

## 管理员 API

以下接口复用现有认证中间件，返回的截图、模板与 Trace JSON 使用 `Cache-Control: private, no-store`：

- `GET /api/runs/:run_id/trace?after=0&limit=100`：最多 100 条图片元数据，`next`/`has_more` 游标，`status`（available/expired/unavailable/retained）、`enabled`、`stopped`、`gaps`、冻结的 `snapshot`
- `GET /api/runs/:run_id/trace/images/:image_id`：原尺寸 PNG；加 `?template=true` 读取该识别的冻结模板。错误运行的 image_id 不可跨运行读取；未知 404、到期 410
- `POST /api/runs/:run_id/trace/retain`：复制已完成证据，已保留时幂等；活动、空证据或独立配额不足返回 409

相关运行选择应先按具体设备和自动化 entrypoint 筛选，并核对快照中的配置包与脚本版本。禁止将全局最近一次记录当成当前配置包的故障证据。

## 本地验证与剩余实测

单元/路由测试覆盖无损像素、去重、原始捕获时刻、错误最后帧、开关、24 小时边界、活动/完成配额、队列背压、磁盘故障、重启、并发运行隔离、目录/文件符号链接、身份路径、认证和缓存策略。合成 1280×720 RGB 编码/磁盘基准输出 `TRACE_BENCH`；只衡量本地合成数据，不代表真实设备截图或输入延迟。

仍须在有条件后测 Android/CDP 长时采集成本、真实战斗动态变化、原始目标输入延迟与真实模型故障诊断效果。
