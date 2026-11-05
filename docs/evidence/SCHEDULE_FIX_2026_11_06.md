# 线上定时任务修复（2026-11-06）

用户在诊断后明确要求：136 提前解锁弹窗增加障碍模板点击确认，其余已定位问题一并修复。本轮包括源码、本地回归、线上脚本资源更新和宿主部署。未重新执行真实账号任务或发送人工测试通知；真实运行效果等待后续调度验收。

## 资源修复

线上 hsr/云游戏日常_退出收尾.yaml 从 cloud_game_daily_routine_v1_42 更新为 v1_43，源码 SHA256：`c15100c3daf5e8ae04e7e7882cc4babb92ec45387d21920d2861ede1bcc93b3f`，资源版本 `c15100c3daf5`。通过资源 API expected_version 对旧版本 `8e59a5fb0923` 条件更新；原脚本、资源元数据和两任务完整配置保存在生产卷 `/data/_fix_backup_20261106`。

- 挑战页等待新增 `obstacles: [daily_echo_unlock_confirm_20261106#540_570_700_680.png]`。模板由本次失败运行的既有确认弹窗帧裁剪，为 98×43 灰度 PNG；区域覆盖确认按钮并留足模板滑动余量，复用现有障碍识别/点击能力。
- 历战余响体力 OCR 区域四周增加约 3px 留白，识别置信度由该布局的 0.8 调整为 0.65；其他布局仍用 0.8。保留两次独立观察、首次/复读数值和上限一致校验，并新增上限必须为 300、当前体力在 0..上限的检查。
- 沿用现有历战余响周次数检查和默认侵蚀隧洞切换，不新增游戏策略。

离线验证：本地服务端资源钩子接受修复后脚本；同一模板在 136 两张弹窗帧得分 0.9999936 / 0.9999950、命中中心位于确认按钮，在 188 普通挑战页不命中。保存的 188 帧在扩边 3px、3 倍缩放后，Tesseract line/block 均读出 `249/300`，置信度约 0.697；原区域约 0.342。OCR 复测使用 Pillow 近似宿主预处理，不将其宣称为真实账号重新执行通过。

## 宿主修复

主仓提交 `ed3509b5822ff2030e38a5779d151f9926bf394c`；插件仓提交 `c3af2ef556ca1b6485c47df55c68019b7c9f6f7c`。

- MultiFinisher 向通知钩子传递本轮 summary，不依赖异步数据库写入，也不沿用上批 Task 快照中的摘要。
- 聚合默认标题去掉默认账号，示例「每月兑换：成功（4/4）」；正文列本批全部输入。自定义模板的 target/device.name 为输入名称列表，无单一目标时 id 为空；单输入标题和变量保持原语义。
- 批次耗时从派发到全部终态，包含并发排队；事件键由任务和 scheduled_at 冻结，立即运行使用独立 UUID，继续复用通知记录去重。
- 受支持的 REST/MCP 保存本已通过 Task::new 清除旧 next_wakeup，无需改动调度解释器或数据库结构。移除旧维护脚本直接修改 schedule_json 的路径，新增 `tools/gamer_change_cron.py`：服务端时区预览、完整快照 CAS、显式 --apply、保留多输入/并发/参数/通知配置，不自动重试未知写结果。
- 修复 MCP task_save 重建任务时丢失 inputs/concurrency/result_summary 的缺口；输入逐项复用对象范围授权，缺省保留，显式值校验和完整快照 CAS 保持。

## 验证

- 完整后端：`cargo test --bin gamer-server -- --test-threads=4`，**1229 passed / 0 failed / 55 ignored**。
- 通知专项：聚合 Core 2 项、通知模板 4 项通过；通知整个模块 17 passed / 0 failed / 1 ignored（真实外发 smoke 按原约定忽略）。HTTP mock 覆盖本批 2/4 替代旧批 3/4、四行明细、125 秒整批耗时、稳定事件键与重复事件仅一条发送记录。
- 改时专项：REST/MCP 更新清旧游标且保留多输入、服务端时区每月 6 日 05:00、stale CAS 拒绝、非法/越权输入拒绝，3 项通过。
- Python 改时维护工具 5 项 mock 通过；格式与 diff 检查通过。

## 部署

部署完成：北京时间 2026-11-06 06:12，新容器 healthy。线上系统信息确认版本 0.5.17、源码提交 ed3509b5822ff2030e38a5779d151f9926bf394c、Linux x86_64、schema 8。资源 SHA256 和配置复核全部通过。

镜像从干净 Git 归档构建，复用已核对摘要的 v1.0.16 基础镜像，只替换宿主二进制并加入改时维护工具；应用版本仍为 0.5.17，以源码提交标识本次修复。

- 镜像：`gamer:taskfix-20261106-ed3509b`；manifest list `sha256:c292bad771b889256585b991c7b3f8716a2b9c85b413591ca538bad54569275b`；运行镜像 config ID `sha256:706a5efd48110d43c45526089f127a960f044cc468bcea0c410deb0dce42f983`。
- 镜像归档 583,581,614 字节，传输两端 SHA256 一致：`d0a3419ff268135ca6dc491eed3df5ce0d448567d47e00af6711252ced1cf034`。
- 全卷冷备份：`/tmp/gamer-taskfix-20261106/data-before-taskfix.tar.gz`，1,923,281,037 字节，gzip -t 通过，SHA256 `42811d2e81bbc67f2098d7a81e11b9feac47dd5abbfd438a37c87a9ee45c0e2a`；原容器保留为 `gamer-before-taskfix-20261106`，restart=no。
- 实际模板存储 SHA256（服务端灰度归一化后的字节）：`02938e1a69d176ce920c14b17b8dfc17b3a057d3a61e1abb1e1e165bc381bcad`。
- 验证两任务全部关键字段与修改前快照相同：name/app/runner/entrypoint/payload/schedule/enabled/extensions/inputs/concurrency/preset/next_wakeup；两个任务均四输入、并发 2。日常下次 **2026-11-07 04:01**，每月兑换下次 **2026-12-06 05:00**；全部输入无活动运行。
- 端口、数据卷、主机名、环境变量键值、共享内存、安全策略、日志配置、资源上限和设备/权限相关参数均复核等效。沿用主机名，未删除浏览器资料锁文件或修改真实账号资料。
- 新增维护工具已在容器 `/app/tools/gamer_change_cron.py` 可用；宿主旧 `/tmp/change_cron.py` 也去除直接 SQL，改为调用此 MCP/CAS 工具。

部署曾两次自动回滚：第一次 CLI 把 inspect 返回的 seccomp 正文当文件路径；第二次新容器已健康，但 Env 数组排列顺序导致过严比较。将原安全策略写文件、按环境键值和最终挂载语义复核后，启用同一个已验证候选成功；全过程未降低安全策略。回执分别保存在宿主同一目录的 deploy-receipt.json、switch-retry-receipt.json、activate-receipt.json；最终资源与任务核验保存在生产卷 `/data/_fix_backup_20261106/verification.json`。

本地构建、验证与临时资源证据保存在忽略目录 `server/runtime/diagnose-tasks-20261106`；完整后端日志在 `server/target/taskfix-20261106-all-tests.log`。
