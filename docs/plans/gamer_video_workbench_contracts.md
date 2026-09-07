# 视频工作台 V1 实施合同（并行开发协调文档）

> 父计划：`docs/plans/gamer_video_workbench_development_plan.md`（下称"计划"）
> 本文是并行实施的**唯一协调点**：模块所有权、REST/JSON/TS 形态、集成点。
> 与计划冲突时以本文为准；实施中确需改合同 → 在最终报告中申报，由集成者统一落。
> 骨架已提交：`server/src/media/mod.rs`、`server/src/recording/mod.rs`、
> `server/src/api/{media,recording}.rs`（函数体 `todo!()`，签名即合同）。

## 0. 模块所有权（写权限矩阵，违者视为冲突）

| 模块 | 可写（独占） | 只读 |
| --- | --- | --- |
| A 媒体服务 | `server/src/media/**`、`api/media.rs`、`api/vision.rs`、`capabilities/frame.rs`、`capabilities/vision.rs`、`capabilities/adapters/{vision,frame,mod}.rs`、`extensions/host_api.rs`、`extensions/permissions.rs` | 其余全部 |
| B 录制服务 | `server/src/recording/**`、`api/recording.rs`、`device/{scrcpy,mod,frames}.rs`、`api/devices.rs`、`capabilities/{input,touch}.rs`、`capabilities/adapters/{input,touch,run}.rs`（`adapters/mod.rs` 归 A；B 若必须改 → 不改，报告集成者） | 其余全部 |
| C 舞台前端 | `web/src/components/console/{ConsoleVideoStage,TemplateCropModal,TemplateCapture}.vue`、`web/src/components/console/useConsoleTemplates.js`、Console 舞台相关新组合式 `web/src/components/console/useConsoleStage*.js`、`web/src/views/Console.vue`、`web/src/api.js` | 其余全部 |
| D1 视频扩展+YAML 草稿 | `server/src/extensions/video/**`、`extensions/mod.rs`、`extensions/service.rs`、`extensions/gamer_yaml/**`、`tools/plugins/gamer.video/manifest.toml` | 其余全部 |
| D2 视频面板前端 | `web/src/components/video/**`、`web/src/workspace/core-component-registry.ts`、相关新测试 | 其余全部 |
| 集成者 | `api/mod.rs`、`main.rs`、`api/common.rs`（已定稿）、guard/boundary 测试、AGENTS.md、PITFALLS、跨模块修复 | - |

硬规则：
- **禁止** `git add -A` / `git add .`；只 add 上表自己的文件。工作树里别人的新文件一律无视。
- 禁改：`AGENTS.md`、`docs/PITFALLS.md`、`docs/plans/**`、`api/mod.rs`、`main.rs`、`api/common.rs`（集成者所有）。踩坑写进最终报告，集成者汇总。
- D1 在 `extensions/mod.rs` 加 `pub(crate) mod video;`（文件归 D1）。
- 提交：Conventional Commits 中文一句话；先 `cargo check`（Rust）/`pnpm vitest run`（web）通过再提交；**不跑全量 `cargo test`**（集成者统一跑）；测试进程可能与他人排队属正常。
- Windows 环境：无 grep/head，用 `git grep` / PowerShell；cargo 在 `server/`，前端 `pnpm` 在 `web/`。

## 1. 媒体 REST（A 实施于 `api/media.rs`，服务体在 `crate::media`）

| 端点 | 形态 | 响应 |
| --- | --- | --- |
| `POST /api/media/import?name=<urlencoded 文件名>` | raw 字节 body（组限额 1GiB） | 201 `MediaMetadata` JSON |
| `GET /api/media` | - | `{"media":[MediaMetadata]}`（创建时间倒序） |
| `GET /api/media/:id` | - | `MediaMetadata`；不存在 404 `{"error":"media_not_found"}` |
| `DELETE /api/media/:id` | - | 204；被引用 409 `{"error":"media_referenced"}` |
| `GET /api/media/:id/file` | 可选 `Range` 头 | 原文件流（206/200，`Accept-Ranges: bytes`）；仅供 `<video>` 播放 |
| `GET /api/media/:id/frame?pts_us=|index=&max_width=` | - | PNG 字节（同一请求逐字节可重复）；400 非法参数 |
| `POST /api/media/:id/refs` | `{"refs":[{package_id,plugin_id,kind}]}` | 200；V1 可 501 |

`MediaMetadata` 字段 = `crate::media::MediaMetadata`（骨架已钉）。原文件不可变；
导入 = 临时文件 → ffprobe（`ffmpeg_path` 来自 Config，缺失/失败 → 415 `{"error":"media_unsupported"}`）→ 校验 → 原子改名。
**V1 不动 SQLite/schema**；元数据只存 `metadata.json`。

### 1.1 离线视觉测试（A）

`POST /api/capabilities/vision/test` 增加可选字段 `media_id` + `pts_us`（与设备路径互斥；
给了 media_id 就**不要求设备存在**、绝不触达 ADB/设备截图）。返回结构沿用现有，
附命中坐标（encoded→oriented 后的 oriented 空间）。实现走现有 matcher，不复制 NCC。

### 1.2 Host API（A）

`extensions/permissions.rs` 权限闭集追加：`media.read`、`media.import`、`media.record`、
`media.write`、`media.events.read`；`extensions/host_api.rs` 增加 media 域 facade
（import/get/list/open_frame/record.*/events/release，形态对齐现有 domain 风格）。
**只加目录与校验，无消费者也必须可用**（D1 依赖）。

## 2. 录制 REST（B 实施于 `api/recording.rs`，服务体在 `crate::recording`）

| 端点 | 形态 | 响应 |
| --- | --- | --- |
| `POST /api/recording/start` | `{"device_id":"..."}` | 202 `RecordingSessionMeta`；设备已有活动会话 409 |
| `POST /api/recording/:id/stop` | - | 200 终态 session（幂等，重复调用返回同一终态） |
| `POST /api/recording/:id/cancel` | - | 200 终态（已落盘部分保留为 interrupted 素材） |
| `GET /api/recording/:id` | - | `RecordingSessionMeta`；404 `{"error":"recording_not_found"}` |
| `GET /api/recording/active?device_id=` | - | 200 session 或 404 |
| `GET /api/recording/:id/events` | - | `{"schema_version":1,"events":[InputEventRecord]}`（时间轴升序） |

实现要点（计划 §5）：帧源挂 scrcpy 编码帧分发点（WebRTC 节流/重放之前），起录等 IDR +
参数集；默认收 `.h264` Annex-B，finalize 用 `ffmpeg -c copy` remux MP4（不重编码）；
分段边界（断连/编码参数变化/磁盘压力）→ `on_device_session_boundary` 收尾分段；
PTS 用原始媒体时钟映射到会话单调时钟；录制链路任何失败**不得阻塞实时投屏**
（有界队列，满了丢帧并计数）。设备会话确死回调由 device 层调用
`RecordingService::on_device_session_boundary`。

### 2.1 操作事件（B）

观察点 = 统一输入分发收敛处（REST `POST /api/devices/:id/control` 与 DataChannel
控制信封的服务端汇合路径；覆盖 manual/keymap/runner/plugin 来源）。要点：
- 原始 down/move/up 与语义 tap/swipe 以 `operation_id` 关联，**不重复生成**语义动作；
- `payload` 形态：tap/swipe=`{x,y[,x2,y2]}`（device-display 像素），key=`{code}`，
  text=`{length}`（**默认脱敏，不留明文**），wait=`{duration_us}`；
- `status` 只记 accepted/rejected；未接受的输入不入事件流；
- 无活动录制会话时不记录、零开销（观察点常驻但直通）。

## 3. 数据布局

```text
data/media/<media-id>/
├── original.<ext>          # 原文件，不可变（录制产出 = finalized MP4）
├── metadata.json           # MediaMetadata（source of truth）
├── recording/              # 仅录制产出：session.json + events-*.jsonl + raw/（中间态）
└── work/                   # 工作副本（V1 预留目录，可不实现生成逻辑）
```

`session.json` = `RecordingSessionMeta`；事件按行写 JSONL（`InputEventRecord`）。

## 4. 跨模块消费点（钉死）

- D1 草稿生成消费：`crate::recording::{RecordingService::events, InputEventRecord}`（骨架签名）。
- D2 消费 REST 的封装：优先调用 `web/src/api.js` 合同方法（C 实现中）；为解除
  并行时序耦合，D2 可在本目录内自建 `components/video/videoApi.js`（fetch 直调
  §1/§2 合同端点，同源 cookie 鉴权默认携带），集成者统一决定是否收编进 api.js。
- C 舞台消费 §1/§2 REST；TemplateCropModal 接收**指定帧**（离线=媒体帧 PNG URL，
  在线=现有截图路径），保存裁切仍走现有模板 PUT 管线。

## 5. gamer.video 扩展（D1）

- Native 扩展：`extensions/video/mod.rs`（manifest 常量 `runtime="core"`、
  `component="VideoWorkbench"`、permissions：`media.read`/`media.import`/`media.record`/
  `media.write`/`media.events.read`）、`extensions/service.rs` 注册（启动即 Running，
  无 Runner）、`extensions/mod.rs` 模块声明。
- `tools/plugins/gamer.video/manifest.toml` 与 server 侧常量**逐字同步**。
- gamer.yaml 侧 call 动作（经现有 `POST /api/extensions/:id/call` 通路，
  不新增 REST 路由）。Phase 7（最终化计划 §10.1）起收敛为 `gamer_yaml/actions.rs`
  的**版本化公开动作清单**（清单 ↔ 实现测试双向锁死；gamer.video 只经此缝调用，
  禁止直写 gamer.yaml 私有目录/解析 YAML）：
  - `automation.create_draft` v1：`values={"recording_id","event_ids"?,"comments"?:{event_id:注释}}`
    → `{"yaml","diagnostics":[{event_id,reason}],"source":{recording_id,events:[…]}}`。
    tap/swipe/key/wait→对应 v3 步骤（注释渲染为步骤上方注释行），间隔→建议 wait；
    无法映射（text 脱敏/未知）→ diagnostics，不丢弃不猜测；`source` 为草稿回查信息。
    草稿只是文本返回，不落盘、不执行。
  - `automation.save_draft` v1（Phase 7 §10.3）：`values={"package_id","name","yaml","overwrite"?}`
    → `{id:"<pkg>/<name>.yaml",path,package_id}`；保存边界走 v3 校验钩子，重名需 overwrite。
  - `template.create_from_frame` v1（Phase 7 §10.2）：`values={"package_id","name","png_base64",
    "region":[x1,y1,x2,y2 相对],"frame":{media_id,frame_index?,pts_us},"calibration":{version,…}}`
    → `{name,short_name,path,size,region,frame,calibration}`；命名规则/灰度归一化/短名冲突
    检测全在 gamer.yaml 服务端（资源字节钩子同路径）。
  - `vision.test_template` v1：**复用 Core REST** `POST /api/capabilities/vision/test`
    （media_id+pts_us/frame_index 离线寻址，不重复实现）。
  - `automation.open_editor` v1：纯前端契约（保存成功后切 `gamer.yaml:automation`
    面板并载入编辑器，经 automationEditorBridge），无服务端往返。

## 6. 前端合同（C 舞台 / D2 面板）

- `web/src/api.js` 新增方法（C 实现，D2 只调用）：
  `listMedia()`、`getMedia(id)`、`importMedia(bytes,name)`、`deleteMedia(id)`、
  `mediaFileUrl(id)`、`mediaFrameUrl(id,{ptsUs,index,maxWidth})`、
  `recordingStart(deviceId)`、`recordingStop(id)`、`recordingCancel(id)`、
  `recordingStatus(id)`、`activeRecording(deviceId)`、`recordingEvents(id)`、
  `createVideoDraft(recordingId,eventIds,comments?)`（→ POST `/api/extensions/gamer.yaml/call`
  action=`automation.create_draft`）。Phase 7 起新增 `saveDraft/createTemplateFromFrame/
  visionTestTemplate/setMediaRefs`（components/video/videoApi.js，动作清单缝与媒体引用同步）。
- `StageSource`（C 实现，计划 §4.2 原样）：`kind:'live'|'media'`、`sourceId`、
  `generation`（来源切换/校准变化递增）、`displaySize/referenceSize`、`canDeviceInput`、
  `frameAt?{mediaId,ptsUs,index}`。媒体模式下舞台产生的设备输入在输入路由统一拒绝。
- C 落点：来源切换 UI 在舞台顶部（实时/视频）、媒体控制条（播放/暂停/逐帧/倍速/
  时间显示/返回实时）、框选与裁切改为对指定帧工作。
- D2 落点：`web/src/components/video/{VideoWorkbench,MediaLibrary,VideoTimeline}.vue`；
  `core-component-registry.ts` 注册宿主组件名 `VideoWorkbench`（安装 gamer.video 即出现，
  卸载即消失，数据在 Package 内保留）。面板三区：素材库（列表/导入/录制入口/删除）、
  时间轴（打开素材→`mediaFileUrl` 预览 + `mediaFrameUrl` 精确帧）、草稿（选事件 →
  `createVideoDraft` → 展示 YAML 文本与诊断，提供"复制"）。

## 7. 集成者负责

`api/mod.rs`/`main.rs`/`api/common.rs` 已定稿不再动；集成阶段：全量 cargo test +
vitest、guard/boundary 测试同步、跨模块编译修复、AGENTS.md/PITFALLS 更新、
ADR（媒体/录制 Core 机制边界）。
