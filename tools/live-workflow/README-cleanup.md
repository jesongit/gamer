# 直播日常与独立收尾

先重建本体，再应用配套插件与脚本（不启动直播、设备或日常运行）：

```powershell
.\gamer.ps1 rebuild
node tools/live-workflow/apply-live-cleanup.mjs --apply
```

执行前停止自动直播和输出，处理完当前／等待项，等待目标上其他任务结束。工具不会代为停止它们，也不会重启服务。它验证本体 `live.status.features.workflow_cleanup`，防止旧本体先收到删掉收尾的主脚本。

默认不带参数仅准备本地文件、读取服务状态。`--stage` 创建／保存私有脚本草稿，使用真实作者服务验证语法、函数及模板引用，不发布正式脚本。候选及备份位于 `server/data/live-workflow-20261015/cleanup-20261015/`；校验脚本：

```powershell
node tools/live-workflow/apply-live-cleanup.mjs --stage
node tools/live-workflow/check-live-cleanup-scope.mjs
```

`--apply` 仅安装已生成的 0.2.13 配套包，发布收尾、配置对应直播规则，再发布主脚本。遇到正式脚本、用户草稿、日常资源或不同收尾配置冲突即停止，不覆盖。源脚本按版本发布；中途中断可重试，已经应用的相同内容跳过。

指定历史队列第 9 项的业务结果通过新 API 按日志证据纠正为成功，原 Core 失败记录保留，不重跑任务。纠正操作对同一审计身份幂等。

`--apply` 的新版服务完整写入验收与真实云游戏回归，须在本体重建后进行；源码测试和无账号 Chrome 测试不等同于实机验收。详细证据见 `docs/evidence/LIVE_CLEANUP_2026_10_15.md`。
