# 直播日常结果与收尾隔离（2026-10-15）

## 现场证据

队列第 9 项 `898757dd-f292-40ea-a1e7-6298d14cc659`，主运行 `93ed9736-bd4b-4469-9e16-94b4b919c09b`，目标 `browser-multiview-blank`。以该 run 的 journal 和 Runtime Trace 定位，不使用全局最近一条记录推断。

- 21:43:37.550 子脚本 `hsr/云游戏日常.yaml` 的直接 `call_end` 为 `ok:true`（事件 96600）。
- 21:43:37.551–.561 父脚本 `run[4] reset_input` 返回成功（96603–96607）。重置没有移到开头，也没有被跳过。
- 下一步等待主页“进入游戏”时，在 21:43:57.570 报 `等待页面就绪超时`（96615），未执行到退出账号。
- 最后可用图像来自 21:43:36.923；异常截图也因等待页面就绪超时未取得。不能以旧截图证明页面没刷新。

用户明确业务规则：日常完成即成功；成功与失败后都尝试重置；收尾异常只记录警告，继续后续弹幕。

## 实现

沿用 gamer-live 原有 FIFO。规则可选 `cleanup`（同包入口、固定参数、默认 180 秒超时），入队时校验并冻结。主脚本和收尾有各自 run_id、状态、错误及时间。主脚本正常成功或失败并释放运行槽后才提交收尾；收尾结束并释放运行槽后才推进视频恢复／下一项。设备被其他运行占用时等待，不取消其他来源的任务。

主脚本自然结束决定业务结果；收尾不覆盖它。显式停止／取消不启动新收尾，已启动的只取消对应收尾 run。服务重启遗留收尾进入核对，不自动重放。日志可查看主运行和独立收尾记录，观众网页仍仅暴露原有公开字段。

两份 HSR 脚本暂存于 `server/data/live-workflow-20261015/cleanup-20261015/`：主脚本只保留登录与日常，收尾第一步 `reset_input`，之后原样等待主页、更多、退出、确认。现有 `云游戏日常.yaml` 11.0 未改。真实服务 `authoring.save` 对两份草稿的语法、函数绑定和模板引用诊断均为空；草稿 3.1／0.1 尚未发布。

新增 `queue.reconcile_success`：只有已结束并释放槽的失败主运行，且真实日志存在指定同包直接子脚本成功完成证据，才允许纠正队列业务结果。原 Core 失败记录保留，附纠正审计。用于本次已完成日常的历史项，不重跑日常或发送输入。

## 页面刷新

本机无账号 Chrome 合成页面复现：有用户激活与 `beforeunload` 的页面在 `Page.reload` ACK 后才发离开确认；不处理会使后续 readyState 请求阻塞。本次真实日志无对话框事件，不能断言现场就是同一原因。

修复仅对通过原控制屏障并实际发送的显式刷新开放一次 `beforeunload` 确认；ACK 后作用域持续至主框架导航提交或有界超时。普通 alert／confirm／prompt、其他会话、手动导航不自动确认。保留租约、帧身份、页面就绪门禁和 ACK 即返回的语义。

证据：`server/target/live-cleanup-20261015/reload-repro/result.json`（基线阻塞 20010ms；处理后 53ms 获取新文档，passed:true）。

## 验证与应用状态

已通过：

- `cargo test --locked --no-default-features extensions::live -- --test-threads=1`：77 项。覆盖主成功／失败与收尾成功／失败组合、冻结参数、两次租约释放、自动超时、仅取消自身清理运行、普通核对恢复、重启清理核对、写盘失败和精确幂等历史纠正。
- 浏览器刷新对话框回归：10 项；另外显式运行 1 项无账号 Chrome 刷新／旧帧拒绝测试和 1 项既有 reset_input 慢加载 ACK 测试，均通过。
- 架构边界 7 项通过；插件 UI 45 项及 Vite 构建通过；固定 SDK 一致性检查通过。
- 当前真实作者服务校验两份草稿通过；11 项 AST／哈希范围断言通过，确认旧日常及原登录／退出操作未被改写。
- 再次通过真实 API 分两页读取本次历史日志，严格条件命中事件 96600；证据在 `completed-daily-proof.json`。

Rust 输出位于 `server/target/live-cleanup-20261015/{live-tests,browser-tests,browser-chrome,browser-slow-load,architecture-tests}.log`。插件包 `plugins/gamer-live-0.2.13.gplugin` 的 SHA-256 为 `f1e9aec2b1c7ea99003337cf5627e849269d4573ceb50ded05a2cc9a27a7d252`。

第一次集成测试发现的三个失败已修复并重跑：两处日志测试缺少外键父运行记录，另一处普通主任务人工核对被误收紧到清理的严格恢复门禁。保留外键与清理终态／租约要求，普通人工核对恢复原有语义。

完整默认 feature 的独立命名后端候选已编译成功（`cargo rustc --locked --bin gamer-server -- -o ...`，1m55s，SHA-256 `c5c8ca98546da346ac708f780ba7dab88a0b3fd1f0d70574780b6261e7a052d3`）。构建日志见 `candidate-build.log`，没有覆盖正在运行的正式 exe。

没有连接真实账号／设备、执行实际日常或重启正式服务。最后一次预检 `preflight.json` 确认仍需后端重建，两份已校验草稿均尚未发布；第 9 项也尚未纠正，待应用工具调用新 API。正式服务仍需重建并应用脚本，不能称为已部署。

应用工具 `tools/live-workflow/apply-live-cleanup.mjs` 默认仅准备本地文件、读取状态；`--stage` 只暂存私有草稿；`--apply` 要求新版 host 的 `features.workflow_cleanup`，已有直播／等待队列／设备运行均为空闲，检查并更新配套 0.2.13 插件，再按收尾→规则→主脚本发布，最后依据日志纠正指定历史项。旧 host 门禁已实测拒绝，正式脚本和规则未因该测试变化。新版服务端完整应用仍待重建后验证。

服务重建后由用户执行：

```powershell
.\gamer.ps1 rebuild
node tools/live-workflow/apply-live-cleanup.mjs --apply
```

之前自动审批拒绝了服务进程启动／切换，因此本次不尝试以其他方式重启正式服务。不能把源码、草稿和本地合成验证称为正式部署或真实云游戏验收。

## 2026-10-16 重建后应用与回读

用户已重建并明确授权应用。运行 `node tools/live-workflow/apply-live-cleanup.mjs --apply` 成功，随后独立通过真实 API 回读校验：

- `gamer-live` 0.2.13 为 Running，`last_error:null`，后端 `features.workflow_cleanup:true`。
- 主脚本正式 4.0／资源 `bc3ac1c52e57`，收尾脚本正式 1.0／资源 `b361a9ae9391`，两者正式内容均与已校验候选逐字节一致。
- `hsr-live-daily` 和测试规则均已配置独立收尾；正式规则仍启用，测试规则仍停用。原日常 11.0／`5355aff7fed5` 保持不变。
- 指定第 9 项 `898757dd-f292-40ea-a1e7-6298d14cc659` 的队列状态和主结果均为 `success`，对应原主运行 `93ed9736-bd4b-4469-9e16-94b4b919c09b`。审计依据为 `hsr/云游戏日常.yaml` 完成，原页面就绪超时作为收尾警告保留。
- 本次校正 API 返回 `already_reconciled:true`，表明该项在此次调用前已纠正；本次幂等确认，没有重写已有审计或重跑日常。当前无活动队列项，等待 0 项。

应用报告 `server/data/live-workflow-20261015/cleanup-20261015/applied.json`；最终回读证据 `verified-20261016.json`。本次只应用／核对资源和业务状态，没有启动脚本、输入设备、启动直播或切换服务进程；真实云游戏全流程仍需实际触发验收。

随后用户明确领奖核验才是成功边界，已发布日常 12.0／直播 5.0：直播在实际 500 金勾核验后返回，不再等待日常的关窗及主界面检查；独立收尾保持不变。详见 [领奖边界证据](LIVE_REWARD_BOUNDARY_2026_10_16.md)。
