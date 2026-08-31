# CLEAN_BASELINE 波次 2 功能回归验收证据（2026-09-01）

验收范围：此前因浏览器 `ERR_BLOCKED_BY_CLIENT` 受阻未验收的功能项——DataChannel 控制、
viewer 接管互斥、watchdog/idle 生命周期、本机状态页冒烟。结论先行：**浏览器层受阻结论被推翻**，
MCP 托管的全新 Chrome 实例下 WebRTC 视频、DataChannel、接管互斥全部实测通过。

## 环境与准备

- 服务端：工作树当前构建（`cargo build` 13.5s 完成），进程命令行：
  `GB_CONFIG=D:/qa-agentD-tmp/config.toml GB_LOG=D:/qa-agentD-tmp/gamer-server.log
  GAMER_ADMIN_PASSWORD=<一次性随机值> ./target/debug/gamer-server.exe`（cwd=server/，端口 **8443**）
- scratch 配置 `D:\qa-agentD-tmp\config.toml`：基于 `server/config.toml`，`data_dir` 指向
  `D:/qa-agentD-tmp/data`（全新空库，启动自动建 schema v1 并扫描入库设备），`idle_power_secs = 60`
- 生效配置日志行：`effective config: port=8443 data_dir=D:/qa-agentD-tmp/data ... idle_power_secs=60s ...`
- 设备：小米 25079RPDCC（id `HIUWUCNJOBEEOZDY`，USB），`/health/ready` 200，五项检查全 ok
- 登录：`POST /api/login {username:"admin", password:<GAMER_ADMIN_PASSWORD>}` → 200
  （开发口令仅来自环境变量；无默认账号/密码的约束保持）
- 证据截图目录：`D:\qa-agentD-tmp\shots\`；服务端日志：`D:\qa-agentD-tmp\gamer-server.log.2026-08-31`（按天滚动命名）

## 步骤 2：本机状态页冒烟 — PASS（本机部分）

- 登录页无预填凭据；登录后 Console 正常渲染（`shots/02-login-console.png`）
- 设置页（#/settings）显示**真实服务端数据**，与 `GET /api/system/info` 逐项一致：
  - 版本 `GameBot 0.1.0`（来自服务端响应，非前端硬编码）、channel `dev`、`commit 6f7792a0`、
    target `x86_64-pc-windows-gnu`、`boot 27964cbb`（与 boot_id 一致）
  - readiness「就绪」徽标；数据库 schema v1 / 文件布局 schema v1 / 自动回滚下限 v1
  - 依赖表：adb `1.0.41`（外部）、ffmpeg `9.0.1-full_build-www.gyan.dev`（外部）、scrcpy `3.3.3`（随应用分发），状态均「正常」
  - 更新能力 check/download/install/rollback 均禁用，明确提示 `update_not_managed`
  - 证据：`shots/02b-settings-real-info.png`
- **发现（缺陷，轻微）**：G 项「页面明确显示任务使用的服务端时区」与实际不符——
  全前端（web/src）无任何「时区」文案/时区标注（`grep -rn "时区" web/src` 仅命中
  RunConflictModal.vue 注释）；定时任务页「下次执行」用服务端 Local 时间渲染
  （scheduler.rs `next_run` 基于 `Local::now()`），页面不标注这是哪个时区。
  见 `shots/02c-tasks-no-tz-label.png`。

## 步骤 3：设备连接与投屏 — PASS

- 打开 `#/console` 后前端**自动建链**（无需手点连接）：页面显示「WebRTC 连接建立」，
  视频实时画面为设备桌面（1880x3008、fps/延迟/码率徽标实时刷新），非黑屏非占位
  （`shots/03-webrtc-video-homescreen.png`，画面含时钟/图标可辨）
- `GET /api/devices` → `status:"online"`（mirror 模式）
- 服务端日志：`control data channel opened: control`、`pusher live: frame_no=... key=...`
- 补充：Chrome webrtc-internals 统计经页面日志可见 `framesDecoded` 持续增长（62→64 fps 解码），无积压

## 步骤 4：DataChannel 控制 — PASS（含 REST 降级 PASS）

- **键控**：页面「🔇」按钮（`toggleAudio` → `sendControl({type:"audio",on:...})`）产生服务端日志
  `audio forwarding toggled by viewer device=25079RPDCC on=true`（16:57:19）。
  REST `parse_ctl` **没有** audio 动作（api/devices.rs `api_control`），该日志只能来自 DataChannel
  → DC 链路专属铁证
- **触控**：在页面 video 元素派发 mousedown/mouseup（clientX/Y 对应设备坐标 799,2336，
  经前端 `toDeviceCoord` contain 映射）→ DataChannel `{"type":"touch","action":"down"/"up"}`
  → 设备 `mCurrentFocusedWindow` 从 `com.miui.home` 变为 `com.android.settings`（触控到达设备）
- **REST 降级**：`POST /api/devices/:id/control`：
  - `{"type":"press","keycode":3}` → 200 `{"ok":true}`，焦点 设置→桌面
  - `{"type":"tap","x":799,"y":2336}` → 200，焦点 桌面→设置
  降级路径功能完整（前端 `sendControl` 在 DC 未开时自动 fallback REST 并 toast 提示）
- 排除项（非缺陷）：VOL_UP（keycode 24）在本机不改变 `dumpsys audio` 音量——adb 直注
  `input keyevent 24` 行为一致（该设备音频走 remote_submix，音量键特性），REST 与 adb 对照一致

## 步骤 5：viewer 接管互斥 — PASS

时间线（服务端日志为证）：

1. 页面 2 打开 `#/console`，自动连接 offer（无 force）→
   `16:59:28 ws offer rejected: another viewer active (conflict)`；页面 2 停止自动重连并持久显示
   「设备已在其他页面连接，本页已停止重连」（设计：自动重连遇 conflict 直接放弃）
2. 页面 2 点「连接」→ 弹确认框：「设备…正在其他页面投屏。确认接管连接？对方页面将断开且不会自动重连。」
3. 确认后带 force 重发 → `17:00:02 kicked previous viewer (takeover) ... old_viewer_id=1a36... new_viewer_id=281d...`
4. 断言：
   - 页面 1（被顶页）断开、**未自动重连**（此后无任何新 offer/conflict 日志，UI 保持未连接态）
   - 页面 2 画面正常（`shots/05-takeover-page2-streaming.png`，流媒体统计 2fps/8ms 正常刷新）
   - 设备 scrcpy 会话未拆：`GET /api/devices` 全程 `status:"online"`，日志无会话重建、无重连风暴

## 步骤 6：脚本运行与可视化 — PASS

- 分区准备：`PUT /api/devices/:id` 设 `pkg=com.android.settings`（仅改 pkg，投屏会话未拆、画面不中断——
  与「仅投屏相关参数变更才拆会话」一致）
- 创建脚本：`POST /api/scripts {"pkg":"com.android.settings","name":"qa-smoke","content":"steps:\n  - log: QA smoke start\n  - tap: [0.425, 0.7766]\n  - log: QA smoke tap done\n"}`
  → 200 `{"id":"com.android.settings/qa-smoke.yaml","version":"ddb928b085ac"}`
- Console 运行（页面 2「▶ 运行」点击）：日志 `run accepted run_id=d472d300... source=Manual` →
  `run finished ... state=Success elapsed_ms=63`；运行日志三条（`GET /api/logs`）：
  `QA smoke start` / `QA smoke tap done` / `脚本执行完成(level=success)`
- API 复跑：`POST /api/scripts/com.android.settings%2Fqa-smoke.yaml/run` → **202**
  `{"resolved_args":{},"run_id":"fb47fb73-8504-4e75-b496-cf072a9122fa","state":"starting"}`；
  `GET /api/runs/fb47fb73...` → 200 `state:"success"`（API 实际取值为 `success`，非 "succeeded"）
- 设备侧效果：两次运行后 `mCurrentFocusedWindow` 均变 `com.android.settings`（脚本 tap 生效）
- **se 事件反推链路**：页面 2 预装 MutationObserver，API 触发运行后 DOM 出现 `.alt-tap` 覆盖层
  （`style="left:765.8px; top:677.9px..."`，即引擎 `{"type":"se",...}` 经 DataChannel 反推 →
  页面 tap 可视化渲染）——命中/未命中框（`.hit-box`）同机制

## 步骤 7：watchdog/idle 生命周期 — PASS（watchdog 仅非破坏观测）

- 关闭页面 2（无 viewer、无脚本运行），基线 `mWakefulness=Awake`、设备 `status:"online"`
- **idle（镜像模式路径）**：60s 后 ——
  `17:05:04 idle: turn off mirror screen (session kept) device=25079RPDCC idle_secs=60`
  设备 `mWakefulness=Asleep`；会话保留（`status` 仍 `online`，与设计一致：虚拟屏模式才拆会话）
- **唤醒恢复**：`POST /api/devices/:id/connect` 为幂等 no-op（**不**唤醒，符合「消费者=viewer 注册/run_begin」设计）；
  API 触发脚本运行（run_begin → notify_activity）→
  `17:06:02 idle screen woke up (viewer/script active)`，设备 `mWakefulness=Awake`，会话未重建
- 附：viewer 在看期间每 ~30s `idle screen woke up`（镜像 30s 补醒，设计行为）
- **watchdog**：按约束未人为制造死链路，无强拆事件发生（日志无 watchdog 强拆行）；
  会话中视频自愈链路多次工作：`chain broken without keyframe, requesting IDR via reset_video`（×7）、
  `reset_video requested by viewer (decoder desync)`（×1），全部会话存活、无破坏性动作

## 步骤 8：清理 — 完成

- `DELETE /api/scripts/com.android.settings%2Fqa-smoke.yaml` → 200；列表回空
- `POST /api/devices/:id/disconnect` → 200；`GET /api/devices` → `status:"offline"`
- 停止服务端进程；8443 无 LISTEN（仅遗留客户端 TIME_WAIT）、无 gamer-server/scrcpy 进程残留
- scratch 数据/日志/截图均在 `D:\qa-agentD-tmp\`（仓库零残留；`.qa-tmp-shots/` 空目录可删）

## 对 CLEAN_BASELINE checklist 的建议勾项

| 条目 | 建议 | 证据 |
|---|---|---|
| 10.4 G「本机与 Docker 状态页冒烟测试通过」 | **本机部分建议改判 PASS**（可改措辞为「本机状态页冒烟通过（Docker 部分另计）」或保持未勾但注明本机已过）；Docker 状态页仍未测（daemon 不可用） | 步骤 2 全项 + `shots/02b-settings-real-info.png` |
| 10.4 G「页面明确显示任务使用的服务端时区」（已勾[x]） | **建议复核**：全前端无时区显示，勾选与实际不符（产品缺口，见步骤 2 发现） | `shots/02c-tasks-no-tz-label.png` + grep 证据 |
| 10.5「DataChannel 控制正常，REST 可靠性降级仍可用」 | **建议勾选 [x]** | 步骤 4：DC audio 日志（REST 无此动作）、DC 触控焦点迁移、REST press/tap 均生效 |
| 10.5「viewer 接管、重连、watchdog 和 idle 生命周期未回归」 | **建议勾选 [x]**（watchdog 死链路强拆路径未人为触发——按验收约束仅作非破坏观测；其余子项全过） | 步骤 5/7 日志时间线 |
| 10.5「Docker readiness、时区和 WebRTC UDP 验证通过」 | 维持未勾（Docker daemon 不可用；时区显示缺失为实际问题） | — |
| 10.5「登录、设备扫描、连接和投屏的 HTTP/session 冒烟成功（浏览器视频轨道未验证）」 | **浏览器视频轨道部分建议补注为已验证** | 步骤 3：`shots/03-webrtc-video-homescreen.png` |

## 发现的产品问题与风险汇总

1. **（缺陷·轻微）前端无服务端时区显示**：G 项已勾但实际缺失；Docker 部署（TZ 可配）下用户无法从 UI 判断 cron 时刻基准。
2. **（UI·轻微）Console 设备下拉状态标签不随连接刷新**：建链后仍显示「· 离线」，点「刷新」或重进页面才更新。
3. **（UI·轻微）被接管页面缺少持久原因提示**：taken_over 仅 toast「连接已被其他页面接管」转瞬即逝，此后停在通用「未连接设备」；对比冲突放弃路径有持久文案，信息不对称（功能行为本身正确：不自动重连）。
4. **（行为确认·非缺陷）`POST /api/devices/:id/connect` 幂等 no-op 不唤醒已关屏**：唤醒只由 viewer 注册 / run_begin 触发；外部系统若只调 connect 期望“唤醒设备”不会生效。
5. **（环境·非缺陷）**小米设备 `screencap` 返回 1200x2000 与显示 1880x3008 比例不一致（MIUI 截图缩放），外部坐标校准须以 `wm size` 为准；设备 mDNS 广播导致 adb 出现 USB+TLS 双 transport，裸 `adb shell` 需 `-s`。VOL 键在本机无 dumpsys 可见效果（remote_submix 路由特性）。
6. **（风险·遗留）**watchdog「会话确死→force 拆会话→重连」路径与 Docker readiness/UDP 仍未实测（前者受验收约束、后者受 Docker daemon 限制），这两点在本轮未获得证据。
