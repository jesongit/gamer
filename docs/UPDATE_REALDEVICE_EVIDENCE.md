# 批次 5 真机验收证据（升级前后 adb / 投屏会话 / 控制 / 截图 / 模板匹配 + 脚本运行与定时任务升级门禁/升级后恢复）

> 用途：批次 5 §17.7「至少一台真实 Android 设备完成升级前后 adb、投屏、控制、截图
> 和模板匹配」「至少一次脚本运行和定时任务升级门禁/升级后恢复验证通过」两项的真实
> 设备端到端证据。仿照 `docs/UPDATE_M2_EVIDENCE.md` 结构；台架方法复用其 E-2/E-4。
> 依据：`docs/AUTO_UPDATE_DEVELOPMENT_PLAN.md` §6.5（业务空闲门禁契约）、§11 测试
> 矩阵、§17.7 checklist；升级链路以 `docs/UPDATE_CONTRACT.md` §6.6 状态机为准。
> 状态：**已实测（2026-08-31～09-01）**——真实小米设备（MIUI/HyperOS）在 0.1.0→0.2.0
> 真实升级前后全部功能等价通过（S1 19 项 / S4 16 项全 PASS），升级门禁真实竞争
> 全程观测并留痕；本轮发现 **1 个阻断级升级链路缺陷 + 1 个中级缺陷**（只记录未改码，
> 见 §R-7）。
> 证据目录：`D:\qa-agentE-tmp\{evidence,logs}\`（临时台架）；本文引用均为实测摘录。

## 实测环境

- 日期：2026-08-31 21:40 ～ 2026-09-01 02:35（本地 UTC+8）；工作树基于 commit
  `6f7792a`（main）+ 未提交修复轮（launcher/src/** 长路径/跨盘修复、QA 台架脚本），
  打包产物从当前工作树全量重建（server/launcher cargo release + web pnpm build +
  四打包脚本实跑，`-Scenario build` 退出码 0）
- 真机：小米 25079RPDCC（HyperOS，turner），`adb devices -l` 同时存在 USB
  （`HIUWUCNJOBEEOZDY`）与 TLS（`adb-HIUWUCNJOBEEOZDY-*. _adb-tls-connect._tcp`）
  双 transport；物理分辨率 1880x3008 / 450dpi
- 台架：安装根 `D:\qa-agentE-tmp\GameBot 真机验收E`（**中文+空格**），端口 **29443**；
  `config.toml` 改写 `port=29443`、`idle_power_secs=0`（关空闲拆会话）、
  `[update] strategy="off"`（关闭协调器自动流程，时间线由脚本显式驱动）
- `GAMER_ADMIN_PASSWORD` 一次性随机 32-hex（进程内转 Argon2id，不落盘）；
  repair 全程死代理（`HTTP(S)_PROXY/ALL_PROXY=http://127.0.0.1:9`），日志仅 seed 命中
- 产物（重建后实测大小/sha256 前 16）：
  - `GameBot-0.1.0-windows-x64-full.zip` 71,603,605 B `0708a7d16c850abb…`
  - `gamer-app-0.1.0-windows-x64.zip` 15,163,157 B `92288382e64d564e…`
  - `gamer-app-0.2.0-windows-x64.zip`（隔离副本构建，主树版本号未动）
    15,160,749 B `b42a7023d1212211…`
  - `0.2.0.json` 7,382 B `fd7986ea21e39b79…`（dev-ed25519-1 签名，validate 通过）

## 与冻结契约的两个边界（先读）

1. **CLI 手动升级语义 = 立即安装，无 §6.5 策略门禁判断。** `launcher upgrade` 的
   `waiting_idle` 只是 journal 状态标记，随后即 drain（批次 3 设计取舍，无策略引擎）。
   「活动 run 阻塞」与「cron 冻结窗口」的实际语义是 **server 侧优雅停机链路**：
   `POST /api/shutdown` → `RunManager::begin_shutdown(10s 宽限)`（拒绝新 run、等
   活动任务结束、超时强停）+ scheduler 触发被 `ShuttingDown` 拒绝。本轮按实测语义
   记录（§R-3），与 §6.5 契约的差异如实对照。
2. **server install API 的 IPC 链路止于 prepare_install**，drain→commit 编排入口只有
   CLI（安装锁与 `start` 互斥）——与 `UPDATE_M2_EVIDENCE.md` 边界 2 相同，本轮沿用
   「taskkill launcher（server 孤儿存活）→ CLI 接管」路径。

## R-1 S0 台架装配（解压 → repair → start → ready）

- **结论：PASS**
- 解压 full ZIP 到中文+空格安装根；config 三项改写生效
  （`port = 29443 | idle_power_secs = 0 | strategy = "off"`）
- `repair --manifest manifests\0.1.0.json`（死代理）退出码 0，3 次
  `seed 命中且校验通过`（adb/ffmpeg/app zip），`current.json = 0.1.0/previous=null`
- `launcher start` → `/health/ready` **200**
  `{"checks":{"adb":true,"data_dir":true,"ffmpeg":true,"scrcpy_server":true,"sqlite":true},"ready":true}`
- `POST /api/login` 200（GAMER_ADMIN_PASSWORD 透传链路）
- 台架坑（与 M2 §踩坑一致地复现）：`Start-Process -ArgumentList` 数组项含空格不加
  引号会截断参数（repair 报 Usage）——改 ProcessStartInfo 拼引号 Arguments 解决

## R-2 S1 升级前真机功能（全部经 API 驱动，19 项 PASS / 0 FAIL）

> 逐项输出：`D:\qa-agentE-tmp\logs\s1-probe.log`；截图/锚点：`evidence\pre_*.png`、
> `anchors-pre.json`

1. **adb 入库与连接**：`POST /api/devices/scan` 200（设备入库）；`connect` 200
   `{ok:true,app_started:true}`（scrcpy 会话建立，mirror 模式）。
   环境特性（非缺陷）：MIUI 双 transport 下扫描去重把入库 addr 更新为 TLS serial
   `adb-HIUWUCNJOBEEOZDY-…. _adb-tls-connect._tcp`（kind=wifi），connect/scrcpy 正常
2. **截图（活画面）**：`POST /api/devices/:id/screenshot` 连续两张非空 PNG
   **884,102 / 861,976 字节**；PIL 像素差 bbox=(29,22,1849,3008)，灰度差≥8 的像素
   **76,946 个** → 活画面
3. **控制（REST）**：`{"type":"press","keycode":3}`（HOME）后
   `dumpsys window mCurrentFocus` 由 `com.android.settings/...` 变
   `com.miui.home/...` → 按键注入真实生效
4. **模板匹配（真实 NCC 命中 + 端到端运行）**：
   - 截图裁剪顶部标题栏（564x180）→ `POST /api/templates`（分区
     `data/com.android.settings/tmpl/`）创建 `anchor_top.png`
   - `POST /api/templates/anchor_top.png/test` → `hit=true @ (0,44) 557x178
     score=0.9259`
   - 极简脚本 `probe.yaml`（`find: anchor_top.png`，else `throw`，命中点击模板中心）
     → `POST /api/scripts/:id/run` 202 run_id `a9800631-6e3d-…` → 轮询至 **success**
     （else 未触发即证明 find 真实命中；运行日志含探针/命中行）
5. **定时任务（真实 cron 触发）**：`POST /api/tasks`（`agentE-probe`，
   cron `* * * * *`，psig1 签名门禁保存成功）→ `scheduled_runs` 表出现真实触发行：
   `(1788199680, success)`、`(1788199740, success)`（连续两个分钟刻度真实执行）
6. **业务锚点**：`anchors-pre.json`（devices/templates=1/scripts=1/tasks=1 + system/info）

## R-3 S2 升级门禁真实竞争（waiting_idle 阻塞 + run 未被杀 + cron 冻结拒绝）

> 轨迹：`evidence\s2-journal-trace.json`、`s2-scheduled-runs.json`、
> `s2-server-log-excerpt.txt`、`s2-run-final.json`；方法：taskkill /F launcher
> （不带 /T）→ 孤儿 server ready 200 → 长脚本运行中 CLI 接管。

时序（t0 = 长脚本启动时刻，分钟刻度 +50s）：

```
t0+0.0   POST /api/scripts/:id/run（gate.yaml：10×(log+wait 1.2s)≈13s）→ 202
t0+5.0   run state=running 确认
t0+5.2   启动 launcher upgrade --manifest 0.2.0.json（cache/artifacts 种子命中）
t0+5.196 journal downloading|downloading
t0+5.348 journal waiting_idle|waiting_idle          ← 进入等待空闲
……      journal 停在 waiting_idle 不前进（run 继续跑）
t0+12.1  gate run 自然终态 success（18:21:02.085，未被杀；server 日志 run finished state=Success）
         —— 但 server 仍不退出（见缺陷 #1），升级无法继续
t0+60s   cron 触发（18:21:04）：server 日志 WARN `scheduled trigger dropped: server draining`
         → scheduled_runs 落 (1788200460, skipped,「服务正在关闭」)
t0+66s   cron 再触发（18:22:04）：同样 skipped「服务正在关闭」
t0+101.6 journal staged|staged + error update_busy
         「旧版本未在时限内退出，升级已取消: 端口 29443 在 90s 内仍未关闭」→ exit 1
```

- **门禁断言逐项对照**：
  - 「升级停在 waiting_idle 不前进（journal 状态+时间戳）」：**成立**——停留
    t0+5.348 → t0+101.6（96 秒），期间无任何前向状态推进
  - 「活动 run 未被杀」：**成立**——run 自然跑到 success（`run finished … state=Success`）
  - 「等脚本自然结束后升级自动继续」：**未成立**——run 结束后 server 端 drain 仍
    停滞（缺陷 #1），升级在 90s 超时后取消（journal 回 staged，旧版本未动，
    `current.json` 保持 0.1.0，无半写状态）。升级的继续由 §R-4 干净路径完成
  - 「cron 冻结窗口」：**实际语义 = drain 期间触发被冻结拒绝**——两个分钟触发点
    `skipped/「服务正在关闭」`（RunManager `ShuttingDown` 拒绝路径），非 §6.5 的
    「距下次触发 > 冻结窗口才允许安装」前置判断（CLI 手动语义无该判断，边界 1）
- 对照说明：升级取消为**安全取消**——数据未动、旧版本健康、journal 明确
  `update_busy`，符合 §6.6「draining：准确 PID 未退出则默认取消升级」分支

## R-4 S3 升级 committed（干净路径，3.7s）

> `evidence\s3-journal-trace.json`、`s3-upgrade-stdout.log`、`s3-snapshot-manifest.json`、
> `s3-system-info.json`

拆 scrcpy 会话（`POST /api/devices/:id/disconnect`，无运行守卫阻碍）使 drain 空载后
重跑同一升级命令：

```
journal：0.192 downloading → 0.493 waiting_idle → 2.302 snapshotting
        → 2.452 migrating → 2.604 candidate_starting →（committed/cleaning 亚秒快边）
exit 0，耗时 3.7s
```

- `current.json = 0.2.0 / previous = 0.1.0`；journal 终态 `idle/idle`、error=null、
  from/to = 0.1.0/0.2.0
- `/health/ready` 200（五项 checks 全 true）
- 受管重启后 `POST /api/login` 200、`system/info`：**`app.version=0.2.0`**、
  `capabilities check/download/install/rollback 全 true`、deployment `launcher/managed`
- **快照含升级前数据**：`backups/upd-1788200934575-335b/manifest.json` 5 files /
  63,812 B，含 `data/gamer.db`、`config/config.toml` 与升级前业务分区
  `data/com.android.settings/`（probe.yaml、gate.yaml、anchor_top.png）
- 观察项：并发读 journal 偶发一次 Windows 共享冲突（rename 原子替换窗口，
  trace 0.343s `Permission denied`，重试即恢复，无数据影响）
- 过程偏差（如实记录）：首次 S3 断言登录 FAIL——CLI 接管拉起的候选进程未携带
  GAMER_ADMIN_PASSWORD（本台架 CLI 由无该环境变量的脚本启动），认证 fail closed；
  以 `launcher start`（带该 env）受管重启后补齐全部断言 → 收录为缺陷 #2

## R-5 S4 升级后真机功能（同一模板/脚本/任务不改，16 项 PASS / 0 FAIL）

> `D:\qa-agentE-tmp\logs\s4-probe.log`、`evidence\post_*.png`、`anchors-post.json`

1. **数据保留**：设备记录原 id 保留且 `pkg=com.android.settings` 保留；
   `anchors-post.json` templates=1 / scripts=2 / tasks=1 与升级前一致；
   `scheduled_runs` 累计 23 行（升级前历史完整保留）
2. **adb/连接**：scan 200、connect 200（scrcpy 会话重建）
3. **截图**：934,846 / 917,732 字节非空 PNG；像素差 bbox=(31,22,1856,3008)、
   76,472 像素 → 活画面
4. **控制**：HOME 键焦点 `com.android.settings` → `com.miui.home` 变化
5. **模板匹配**：同一 `anchor_top.png`（升级前创建，未重建）test
   `hit=true @ (0,44) 557x178 score=0.9430`；同一 `probe.yaml` run
   `d9ec44d1-6afd-…` → **success**（find 命中点击、运行日志含命中行）
6. **定时任务**：升级后新 server 上 cron 真实触发——`scheduled_runs`
   (1788200940, success)、(1788201000, success)（18:29:07 / 18:30:00 两个分钟刻度）；
   server 日志 `run finished … source=Scheduled state=Success`
7. **结论**：升级前后真机功能等价（S1/S4 逐项一致），升级前业务数据全部存活

## R-6 补充观测：/api/shutdown 对活动 run 的真实语义（0.2.0 server）

> `evidence\s2b-shutdown-semantics.json`、`logs\s2b-semantics.log`

带登录会话、客户端读超时 60s（**不断开**）对 20s 活动 run 调用 `POST /api/shutdown`：

```
18:31:10.274 run accepted（gate20.yaml，20s）
18:31:12.277 shutdown coordinator: draining
18:31:22.311 WARN shutdown timeout: force-cancelling active runs forced=1   ← 10s 宽限到点
18:31:22.350 run finished state=Cancelled
18:31:23.919 shutdown coordinator: finished → graceful → 端口关闭
POST /api/shutdown → 200 {"ok":true}，耗时 11.64s
```

- 证明：server 停机链路本身**功能正常**（宽限 10s → 强停 → 完成 → 退出）；
  对照 §R-3，升级失败的唯一差异是 **launcher 的 HTTP 读超时（5s）小于 drain 真实
  耗时（11.6s）** → 缺陷 #1 根因坐实

## R-7 发现缺陷（只记录，未改任何产品代码）

| # | 级别 | 缺陷（本轮真机实测现象） | 根因 | 对照既有证据 |
|---|---|---|---|---|
| 1 | **阻断（升级链路，真机/有会话场景）** | 活动脚本运行中触发 `launcher upgrade`：journal 正确停在 `waiting_idle`、run 未被杀，但 run 结束后 server 端 drain 停滞永不完成（`shutdown coordinator: finished` 不出现、scrcpy 会话音频流持续、scheduler 持续 tick）→ engine 90s 端口超时 → 升级取消（journal `staged`+`update_busy`「旧版本未在时限内退出」，exit 1） | launcher `drain_old_server` 对 `POST /api/shutdown` 的读超时为 `probe.per_attempt_timeout.max(5s)`≈**5s**，而 server `api_shutdown` handler **同步 await 完整 drain**（真实耗时 11.6s+：活动 run 10s 宽限 + 拆 scrcpy 会话）；读超时后 launcher 断开连接 → hyper 取消 handler future → `ShutdownCoordinator::request()` 在 `(self.drain)().await` 处被 drop，drain 半途停滞且无自恢复 | M2 E2E（无真机：无 run、无 viewer、无 scrcpy 会话）drain 秒级 <5s，响应来得及返回，故未暴露；本轮 §R-6 证明客户端不断开时 drain 11.64s 正常完成。修复方向（供主控决策）：server 侧 handler 内 spawn drain 立即返回（状态查询走 `/health/shutdown`），或 launcher 侧 drain 读超时对齐 shutdown_timeout(90s) |
| 2 | 中（CLI 接管路径） | `launcher upgrade` 接管后拉起的候选 server 无管理口令：`POST /api/login` 401、`system/info` 不可读（认证 fail closed），升级后系统可用性降级直至用户以 `GAMER_ADMIN_PASSWORD` 重启 | `supervisor` 注入 `GAMER_ADMIN_PASSWORD` 来自 launcher 进程环境；CLI 由用户 shell 启动时该 env 通常缺失，engine 未从 `state/`（admin-token 同级）持久化/回注口令 | M2 E2E 未暴露：其 CLI 从 E2E 脚本环境继承了该 env。与 M2 §E-6 #5（候选身份探针观测缺失）同族——持久化凭据的候选侧注入缺口 |
| 3 | 观察（低，无行动项） | 并发轮询读 `state/update-journal.json` 偶发一次 `Permission denied`（Windows rename 原子替换窗口）；轮询重试即恢复 | journal 原子写 = 临时文件 + rename，观察方在窗口内以读共享模式打开失败 | 15ms 级瞬时；不影响 journal 完整性 |

环境特性记录（非缺陷，建议主控收口 PITFALLS）：

- MIUI/HyperOS USB+TLS 双 transport：`adb devices -l` 两行同设备；server 扫描去重
  （serial 子串匹配）会把入库 addr 更新为 TLS serial（kind=wifi）。`-s` 指定任一
  serial 均可操作；connect/scrcpy/截图/控制全链路在 TLS serial 上实测正常
- 本轮真机 screencap 与投屏（视频）分辨率一致（1880x3008），未复现「比例不一致」
  特性；截图 API 走帧缓存解码，坐标天然与视频分辨率对齐

## R-8 未能完成 / 剩余缺口

1. **「活动 run 阻塞后升级自动继续」未能完整闭环**：受缺陷 #1 阻断，S2 的升级在
   waiting_idle 停留后安全取消；committed 由干净路径（无 run、无会话）完成（§R-4）。
   门禁的「阻塞与不杀 run」语义已实证，「自动继续」需缺陷 #1 修复后复验
2. 浏览器侧 WebRTC 投屏（viewer 接管/互斥/初始帧重放）不在本轮范围：本轮验证到
   scrcpy 会话建立（connect 200、app_started）、截图/控制/模板匹配/脚本运行；
   多页面互斥与 DataChannel 可视化事件仍以既有 Windows 证据为准
3. 候选失败回滚场景未在真机台架重跑（M2 §E-5 已有全量证据，本轮复用 0.2.0 正常
   候选）；0.1.0 之前版本的升级路径不适用
4. cron 冻结窗口的 §6.5 契约语义（「距下次触发 > 冻结窗口」前置判断）当前产品无
   策略引擎实现，本轮记录的是 drain 拒绝语义；契约级实现属后续批次

## R-9 复现方法

```powershell
# 产物构建（幂等；-Scenario build 只构建打包，不跑场景）
powershell -NoProfile -ExecutionPolicy Bypass -File release\packaging\test-upgrade-launcher-e2e.ps1 `
  -Scenario build -WorkDir D:\qa-agentE-tmp\rig

# S0 台架（解压/改 config/repair/start/ready）
powershell -NoProfile -ExecutionPolicy Bypass -File D:\qa-agentE-tmp\rig-s0.ps1

# S1 / S4 真机功能探针（pre=升级前含资源创建；post=升级后只重放断言）
python D:\qa-agentE-tmp\probe.py --phase pre  --port 29443 --serial HIUWUCNJOBEEOZDY --root "D:\qa-agentE-tmp\GameBot 真机验收E"
python D:\qa-agentE-tmp\probe.py --phase post --port 29443 --serial HIUWUCNJOBEEOZDY --root "D:\qa-agentE-tmp\GameBot 真机验收E"

# S2 门禁竞争 / S3 干净升级 / 补充观测
python D:\qa-agentE-tmp\s2_gate.py
python D:\qa-agentE-tmp\s3_upgrade.py
python D:\qa-agentE-tmp\s2b_shutdown_semantics.py
```

## S5 清理实录（2026-09-01）

- 台架任务/脚本/模板均在安装根分区与 DB 内（无台架外残留），随安装根整删；
  删除前 `logs/`（launcher.log、gamer-server.log.*）备份至 `D:\qa-agentE-tmp\logs\rig-server-logs\`
- 进程确认：`gamer-launcher`/`gamer-server` 均为 0；本台架 runtime adb（按
  ExecutablePath 精确匹配）无存活进程（另一 QA 目录的 adb daemon 不属本台架，未动）
- 端口：29443 无 LISTENING（仅客户端 TIME_WAIT 残留）；真机 `adb devices` 仍在线
  （USB + TLS 双 transport 原样）

## R-10 缺陷修复与真机复跑（2026-09-01，**缺陷 #1 已修复，升级自动继续实测通过**）

> 承接 §R-7 缺陷 #1（阻断）的修复与端到端复跑证明；修复 2（next_run 时区偏移）
> 顺带交付。台架方法复用 §R-1～R-5（脚本与 §R-9 同构，路径改 `D:\qa-agentI1-tmp\`）。
> 证据目录：`D:\qa-agentI1-tmp\evidence\`（journal 轨迹/run 终态/锚点/日志摘录）；
> server 日志全量备份 `D:\qa-agentI1-tmp\logs\rig-server-logs\`。

### R-10.1 修复内容

1. **缺陷 #1（阻断）**：`launcher/src/upgrade/engine.rs` `drain_old_server` 对
   `POST /api/shutdown` 的 HTTP 读超时由 `per_attempt_timeout.max(5s)` 改为
   **`shutdown_timeout + 5s`**（server handler 同步 await 完整 drain，读超时必须
   覆盖之，否则 hyper drop handler future → drain 半途停滞）；端口关闭的
   cancel deadline 改为**从 drain 起算**（总等待上界仍恰为 `shutdown_timeout`，
   取消语义不变）。附单测双向验证：`drain_survives_slow_graceful_shutdown_beyond_
   legacy_read_timeout`（mock shutdown 延迟 6s 才回 200；暂存回旧超时实测 FAIL、
   恰好复现「5s 断开 → 端口不关 → shutdown_timeout 后取消」的生产症状，修复态
   7.1s PASS）+ `drain_fast_shutdown_path_stays_fast_under_longer_read_timeout`
   （快路径 <5s 不退化）。
   **附带核实（只读，不改 server）**：server 侧最长 drain =
   `begin_shutdown(10s 宽限 + 0.5s settle)` + 踢 viewer（亚秒）+ `shutdown_all`
   （每设备 adb 操作 5～8s 超时封顶 + 1.5s 退场窗口）——本轮实测 8.34s、E 轮
   §R-6 实测 11.64s，均远小于引擎 `shutdown_timeout` 90s；90s 作为 drain 总
   deadline 对真实多设备机群仍有充分余量。
2. **修复 2（小）**：`server/src/api/tasks.rs` `next_run` 序列化改为带时区偏移
   （`format_next_run` = `%Y-%m-%d %H:%M:%S%:z` → `2026-09-01 03:16:00+08:00`，
   保持空格分隔仅追加偏移；`web/src/task-tz.js` 的 `±HH:MM` 解析原生兼容，
   TaskScheduler.vue 原样展示无解析破坏）。新增 server 测试
   `task_next_run_serialized_with_timezone_offset`（列表+详情双端点、偏移可回读、
   必须等于服务端本地偏移）。前端时区标签「服务端时区 UTC+08:00」由此点亮
   （标签分支已有 web 单测覆盖，本轮 web 全绿未改前端）。

### R-10.2 复跑环境与产物

- 日期：2026-09-01 02:40 ～ 03:30（本地 UTC+8，server 日志为 UTC Z 串）；
  工作树基于 `6f7792a` + 未提交改动（含本轮两修复），`test-upgrade-launcher-e2e.ps1
  -Scenario build` 退出码 0 全量重建
- 台架：安装根 `D:\qa-agentI1-tmp\GameBot 真机验收I`（中文+空格），端口 **29543**，
  config 改写同 §R-1（port / idle_power_secs=0 / strategy="off"）
- 真机：同一小米 25079RPDCC（HIUWUCNJOBEEOZDY，USB+TLS 双 transport）
- 产物（重建后大小 / sha256 前 16）：
  - `GameBot-0.1.0-windows-x64-full.zip` 71,596,060 B `a0cae55ed560a77d…`
  - `gamer-app-0.1.0-windows-x64.zip` 15,154,675 B `5c65d6a06195c699…`
  - `gamer-app-0.2.0-windows-x64.zip` 15,155,477 B `fe24d2361d8566c5…`
  - `0.2.0.json` 7,382 B `7f3f5d38cfdfb509…`（dev-ed25519-1，校验通过）

### R-10.3 S0 / S1（升级前台架与真机功能）

- S0：repair 3 次 `seed 命中且校验通过`（adb/ffmpeg/app），`current.json =
  0.1.0/previous=null`，`/health/ready` 200（五 checks 全 true），登录 200
- S1：**19 项 PASS / 0 FAIL**（方法与 §R-2 一致）：scan 入库（TLS serial 去重，
  kind=wifi，环境特性同 §R-7 记录）、connect 200 `{ok:true,app_started:true}`、
  两张截图 944,423/898,355 B（差≥8 像素 473,221 → 活画面）、HOME 焦点
  `com.android.settings→com.miui.home`、模板 `anchor_top.png` test
  `hit=true @ (0,44) 557x178 score=0.9153`、probe.yaml run `643a33ad-…` success、
  cron 任务 `agentI-probe`（`* * * * *`，psig1 门禁保存成功）真实触发 success

### R-10.4 S2 升级门禁真实竞争（修复证明核心，2026-09-01 03:17–03:18 本地）

方法同 §R-3：taskkill launcher（孤儿 server ready 200）→ gate.yaml 长脚本
（10×(log+wait 1.2s)）运行中启动 `launcher upgrade --manifest 0.2.0.json`
（cache 种子命中）。t0 = run 开始（03:17:50.00 本地）：

```
t0+0.0    run 37cc657b-cfc5-43d1-94c7-af1b4f0c46c5 → 202，gate.yaml
t0+5.10   launcher upgrade：check 完成（03:17:55.110）
t0+5.20   cache 命中且校验通过（03:17:55.203）
t0+5.35   journal waiting_idle｜waiting_idle（03:17:55.349）
          同时 server 日志 `shutdown coordinator: draining`（03:17:55.353）
          —— drain 开始等待活动 run，launcher 持续等待 /api/shutdown 响应
t0+12.08  gate run finished state=Success elapsed_ms=12077（03:18:02.081，未被杀）
t0+13.69  `shutdown coordinator: finished`（03:18:03.695）→ graceful http 收尾
          —— drain 全程 8.34s：旧 5s 读超时必然中途断开（E 轮即停滞 96s 后取消），
             修复后 launcher 等到 200，升级自动继续
t0+14.27  候选 0.2.0 启动（03:18:04.270，受管子进程）
t0+14.81  activate 已受理（激活闸内先行，03:18:04.809）
t0+15.40  `升级 committed 并清理完成 from=0.1.0 to=0.2.0`（03:18:05.402）
          —— launcher upgrade 退出码 0，CLI 全程仅 10.4s（E 轮同场景 96s 后 exit 1）
```

- journal 轨迹（`evidence/s2-journal-trace.json`）：downloading(5.197s) →
  waiting_idle(5.349s) → candidate_starting(14.879s) → idle(16.488s)；终态
  **idle/idle、error=null**（`evidence/s2-journal-final.json`），from=0.1.0、
  to=0.2.0，快照 `upd-1788203875058-3162`（5 files / 63,446 B，含
  data/gamer.db、config.toml 与升级前业务分区 3 文件——probe/gate 脚本+模板）
- `current.json = 0.2.0 / previous = 0.1.0`；`/health/ready` 200
- 门禁断言逐项对照（对照 §R-3 修复前结果）：
  - 「活动 run 未被杀」：**成立**（自然终态 Success，elapsed_ms=12077）
  - 「等脚本自然结束后升级自动继续」：**成立（修复前不成立）**——run 结束后
    1.6s 内 drain 完成、3.3s 内 committed，升级退出码 0
  - 「cron 冻结窗口」：本轮**实际语义 = 修复后 drain 仅 8.3s，旧 server 的 10s
    调度 tick 相位未落入 drain 窗口，无触发被拒；03:18:00 触发点由 0.2.0 新
    server 启动后按「窗口内最近一个触发点」补跑并 success**（run `04792304-…`，
    03:18:06.015 `run finished … source=Scheduled state=Success`）——触发点
    零丢失（对照修复前 waiting_idle 停留 96s 期间两个分钟触发点连续
    skipped「服务正在关闭」）；§6.5 契约的「冻结窗口前置判断」仍无策略引擎（同 §R-8）
  - 采样说明：journal 前向状态按 150ms 轮询，candidate_ready/activating/
    committed/cleaning 亚秒快边未被采样命中（E 轮同现象），committed 由
    current.json、CLI 逐阶段日志与 journal 终态三重证明；`s2-checks.json` 中
    唯一 FAIL 项即该采样遗漏，非链路故障
- 复跑方法：`python D:\qa-agentI1-tmp\s2_gate_upgrade.py`（S1/S4 用
  `s1_probe.py --phase pre|post`、受管重启用 `s4_restart.py`，均同 §R-9 形态）

### R-10.5 S4 升级后（0.2.0 server）

- 受管重启（缺陷 #2 仍在：CLI 拉起的候选无 GAMER_ADMIN_PASSWORD，登录 fail
  closed）——taskkill 候选 + `launcher start`（带 env）后 **7 项全 PASS**：
  ready 200、登录 200、`app.version=0.2.0`、capabilities 四项全 true、boot_id
  新实例（`evidence/s4-system-info.json`）
- 功能探针 **16 项 PASS / 0 FAIL**（方法同 §R-5）：设备原 id
  `d1afa904bee04a1891b1d740511a15bd` 与 pkg 保留；connect 200；截图
  970,775/949,013 B 活画面；HOME 焦点变化；同一 `anchor_top.png`（升级前创建）
  test `hit=true @ (11,44) score=0.8710`；同一 probe.yaml run `0af20ab8-…`
  success；cron 03:21:00 触发 success（scheduled_runs 8 行累计，升级前历史完整）
- **修复 2 端到端**：升级前后 `/api/tasks` 的 `next_run` 均带偏移——
  升级前（0.1.0）`2026-09-01 03:16:00+08:00`（`anchors-pre.json`）、升级后
  （0.2.0）`2026-09-01 03:22:00+08:00`（`anchors-post.json`/`s4-tasks.json`），
  前端 `serverTzLabelFromTasks` 据此点亮「服务端时区 UTC+08:00」标签

### R-10.6 门禁测试数字

- launcher：`cargo fmt --check` 干净；`cargo clippy --all-targets --all-features
  -- -D warnings` 干净；`cargo test` **184 通过 / 0 失败**（lib 107 + 集成 77，
  含本轮 2 个 drain 回归测试）
- server：改动模块 `api::tests::…::tasks_tests` **2 通过 / 0 失败**（含新增
  next_run 偏移测试）；`cargo fmt --check` 干净。注：`api::tests` 模块并发运行时
  `auth_tests::authentication_logs_rejection_metadata_without_secrets` 偶发
  FAIL（隔离运行稳定通过，且暂存本轮格式改动复测仍 FAIL）——既有 flake
  （tracing 日志捕获的跨测试竞争），与两修复无关，未改
- web：`pnpm test:run` **42 文件 / 579 用例全绿**（task-tz/task-scheduler 标签
  分支覆盖在列，前端零改动）

### R-10.7 清理实录（2026-09-01）

- 台架任务/脚本/模板均在安装根内，随 `D:\qa-agentI1-tmp\GameBot 真机验收I`
  整删；删除前 server 日志 + journal 备份至 `D:\qa-agentI1-tmp\logs\rig-server-logs\`
- 进程：`gamer-launcher`/`gamer-server` 均 0；端口 29543 无 LISTENING；
  真机 `adb devices` 双 transport 仍在线
