# 线上定时任务失败诊断（2026-11-06）

> 本文保留首次只读诊断时的状态；用户随后授权的修复及上线验证见[修复记录](SCHEDULE_FIX_2026_11_06.md)。

本次为用户要求的只读排查。通过现有 SSH 连接 FnOS 192.168.31.10，检查 gamer 容器日志、SQLite 只读查询、既有 Runtime Trace 与通知记录；未改生产配置、任务、脚本或数据，未重新运行账号。容器为 posase/gamer:v1.0.16，宿主 0.5.17 / commit 75891d1，排查时 healthy。本文时间均为北京时间。

## 日常任务：两处脚本识别/页面分支失败

11 月 6 日 04:01 的「云游戏日常合并」按并发 2 正常派发四输入，实际成功 2/4。

| 账号 | 完成时间 | 结果 | 运行 ID |
| --- | --- | --- | --- |
| 188 | 04:03:19 | POWER_OCR_UNCONFIRMED | e7abe7df-9d56-4bcf-9769-e0d4429faf6d |
| 136 | 04:03:22 | CHALLENGE_PAGE_TIMEOUT | e542aba6-c324-45f7-896a-df6b2f616db8 |
| 155 | 04:08:45 | 成功 | 09c95d65-9601-4f9b-93e2-99ab17f3a4cb |
| 173 | 04:10:05 | 成功 | 854ee7d9-168b-4216-bce1-43a222257c2f |

188 已进入历战余响页面，挑战文字模板得分 0.968，体力图标锚点得分 0.988；五轮 read_power_fraction_once，每轮四次 find_text，全部返回 null。保存画面显示体力 249/300、周剩余次数 0/3。脚本体力区域为 97×29 像素，find_text min_confidence=0.8。将保存画面按相同区域、灰度/3 倍双线性缩放/深底反色处理，交给线上相同 Tesseract 5.5.0 离线识别：line/block 均读出 249/300，confidence=34.167755；扩大留白后为 69.684578，均低于 80。该复测使用 Pillow 近似 Rust Triangle 预处理，不能作为生产原次 OCR 原始输出；它与二十次 null 和过滤规则共同支持裁剪/识别置信度不足的原因。

136 点击进入按钮后停在「提前解锁并挑战历战余响可能会影响剧情体验，是否继续？」确认框；04:02:55 与 04:03:06 的保存画面均有该弹窗。脚本直接等待挑战页面文字 20 秒，没有处理此弹窗，因此超时。两输入在失败路径均确认退出云游戏。

建议：补充历战余响提前解锁弹窗的显式导航分支；改进体力 OCR 区域/留白、保留双读校验；对历战余响周剩余 0/3 的画面另行确定转至可重复刷取副本的策略。单纯放宽置信度不构成完整修复。

## 04:22:06 通知来自同一个每月兑换任务

生产 timer_tasks 只有两个任务。每月兑换 task_id=c5b21e7c072d4549a72ea055410cf3e6，今天有两个 scheduled_runs 批次：04:17 与 05:00，两批均四输入成功。04:17 批次分别在 04:19:25（188）、04:19:39（155）、04:21:54（136）、04:22:06（173）完成。

04:22:06 的通知记录标题确为「155 · 每月兑换：成功」，其时间对应全批最后完成，而标题账号取任务默认 device_id=155；它是四输入聚合通知，不能据此认为 155 在 04:22:06 单独执行了第三个定时任务。对应最后运行 e802b7eb-862e-4acd-9273-57688a9d9d8b 的实际设备为 173。

昨晚改时脚本 server/runtime/change_cron.py 只 UPDATE schedule_json，把 cron 从 17 4 6 * * 改为 0 5 6 * *，未重算 next_wakeup。timer_core.rs:1208 优先采用持久游标，故旧 04:17 游标仍被执行；派发前按新表达式推进游标，05:00 又执行一次。排查确认当前下次时间已为 2026-12-06 05:00；日常下次为 2026-11-07 04:01。

## 聚合通知使用旧摘要

实际通知记录还证明：04:10:05 日常标题为「155 · 云游戏日常合并：失败（3/4）」；正文四账号明细为上一轮的 3/4（188 DEFAULT_CAVERN_STAGE_NOT_FOUND，136 成功），底部 error 才含本轮 188 POWER_OCR_UNCONFIRMED 与 136 CHALLENGE_PAGE_TIMEOUT。权威本轮 result_summary_json 为 2/4。

源码原因：MultiFinisher.finalize（server/src/timer_core.rs:468-493）异步更新数据库摘要，却在 emit_summary_notification:527 将原 task.clone() 传入通知钩子，没有赋本轮 result_summary。通知插件 plugins/gamer-notify/host/task.rs:113 读取 task.result_summary，故首批可能没有明细、后续沿用上批；:63、:131-134 用 task.app.device_id 生成 target/device 名，默认标题带 155。05:05:18 的通知 4/4 恰好与前一批相同，不能证明此缺陷不存在。

修复方向：聚合通知传本轮 summary，并使用能表达多输入任务的标题；耗时应取批次实际时间而非固定 0 秒。此次仅定位，未实现或部署。

## 本地证据

- [136 确认弹窗截图](../../server/runtime/diagnose-tasks-20261106/136_12.png)
- [188 体力 OCR 截图](../../server/runtime/diagnose-tasks-20261106/188_11.png)
- [188 OCR 区域元数据](../../server/runtime/diagnose-tasks-20261106/188_11.png.json)
- [线上脚本只读副本](../../server/runtime/diagnose-tasks-20261106/daily-online.yaml)

上述图片/脚本副本保存在忽略的运行目录，未提交到 Git。scheduled_runs 为每批一行，关联 run_id 可由某次提交覆盖；批次 error 与此 run_id 不是逐输入一一对应，逐账号应看 run_records 和 result_summary_json。
