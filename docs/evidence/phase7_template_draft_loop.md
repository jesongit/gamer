# Phase 7 证据：模板制作、离线测试与 YAML 草稿闭环（计划 §10 + 遗留 1/2）

> 计划：`docs/plans/gamer_plugin_ecosystem_video_finalization_plan.md` §10、§13.1
> 日期：2026-09-07。基线：Phase 8 证据（`fdffc75`）之后的工作树。
> 性质：§10.1/§10.2/§10.3 纵向切片 + Phase 4 遗留 #5（D2 execution_change 提示）
> + Phase 8 遗留（项目保存→媒体引用同步）。全部结论有实测；未做 git commit。

## 1. 动作清单契约（§10.1，唯一声明点 `server/src/extensions/gamer_yaml/actions.rs`）

| 动作 | 版本 | surface | 参数 / 上下文要求 |
| --- | --- | --- | --- |
| `template.create_from_frame` | 1 | Native | caller=gamer.video；values: package_id、name(短名)、png_base64(选框裁剪)、region[相对0..1]、preserve_color?、overwrite?、frame{media_id,frame_index?,pts_us}、calibration{version,reference_size?,rotation?}；服务端负责命名规则（`#x1_y1_x2_y2`×1000 + 可选 `#1`）、灰度归一化（资源字节钩子）、短名冲突（overwrite 门禁）；响应携带帧身份与校准元数据 |
| `vision.test_template` | 1 | Rest | **复用** `POST /api/capabilities/vision/test`（Core vision 能力位；media_id+pts_us/frame_index 离线寻址互斥，响应附帧身份）；不重复实现 |
| `automation.create_draft` | 1 | Native | values: recording_id、event_ids?（顺序=步骤顺序）、comments?{event_id:注释}；→ `{yaml, diagnostics, source}`；不落盘不执行 |
| `automation.save_draft` | 1 | Native | values: package_id、name、yaml、overwrite?；保存边界 v3 校验钩子（非 v3 结构化拒绝）；→ `{id:"<pkg>/<name>.yaml", path, package_id}` |
| `automation.open_editor` | 1 | Frontend | 前端契约：保存成功后切 `gamer.yaml:automation` 面板并载入编辑器（`components/console/automationEditorBridge.js`），无服务端往返 |

清单 ↔ 实现双向锁测试：`actions::tests::catalog_and_dispatch_are_bidirectionally_locked`。
gamer.yaml 非 Running：服务端对全部 Native 动作 409（`call_extension` 统一门禁）；
视频前端按 `GET /api/extensions` 状态禁用制作入口并提示依赖（导入/录制/播放/标记/项目/校准不受影响）。

## 2. 完成工作与修改文件

| # | 项 | 文件 |
| --- | --- | --- |
| 1 | 动作清单 + `template.create_from_frame` + `automation.save_draft`（清单双向锁测试） | `server/src/extensions/gamer_yaml/actions.rs`（新） |
| 2 | 草稿动作扩展：事件注释（换行压平）+ `source` 回查块（kind/时间轴/来源/selected/mapped） | `server/src/extensions/gamer_yaml/video_draft.rs` |
| 3 | 缝重导出（extensions/mod.rs 不变，分发入口换 actions） | `server/src/extensions/gamer_yaml/mod.rs`、`resources.rs`（`template_short_name` pub(crate)） |
| 4 | videoApi：动作清单缝封装 `callGamerYamlAction`、`createTemplateFromFrame`、`saveDraft`、`visionTestTemplate`、`setMediaRefs`；`createVideoDraft` 加 comments | `web/src/components/video/videoApi.js` |
| 5 | 模板工作台：定帧框选→创建模板（帧身份+校准元数据随动作上行）+ 离线测试（阈值/选框搜索区/命中叠加/坐标双读数） | `web/src/components/video/TemplateStudio.vue`（新）、`templateStudio.js`（新，纯函数） |
| 6 | 时间轴正向入口「✂️ 帧上做模板」+ 工作台装配（依赖门禁/媒体引用同步/项目删除解除引用） | `web/src/components/video/VideoTimeline.vue`、`VideoWorkbench.vue`、`videoProject.js`（`projectMediaIds`） |
| 7 | 草稿工作流：选择/取消（删）/↑↓ 重排/注释 → 生成（回查 source 行）→ 命名保存（overwrite 引导）→ open_editor 导航 | `web/src/components/video/VideoDraft.vue` |
| 8 | 编辑器桥 + 消费端（刷新列表→选中→进入编辑态）；面板 key 字面量入 gamer-plugin-ids | `web/src/components/console/automationEditorBridge.js`（新）、`useConsoleScriptRunner.js`、`web/src/gamer-plugin-ids.js` |
| 9 | 依赖门禁判定 | `web/src/components/video/yamlCapability.js`（新） |
| 10 | 遗留 #5：inspect `execution_change:{from,to}` 确认弹窗明确提示（`executionChangeDetail` 可单测） | `web/src/workspace/plugin-center/plugin-service.ts`、`PluginCenter.vue` |
| 11 | 遗留 2：项目保存/创建/删除 → 逐媒体全量替换同步 `gamer.video/project` 引用（按包内全部项目并集重算、他包/他插件条目保留、幂等、404 静默）；删除被引用素材提示文案完善 | `VideoWorkbench.vue`、`MediaLibrary.vue`、`videoApi.setMediaRefs` |
| 12 | 无设备 E2E（临时端口 18443 + 临时数据目录，用完即停） | `tools/e2e_phase7_offline.sh`（新） |
| 13 | 测试 | `video-workbench.test.js`（+11：草稿工作流/依赖门禁/refs 同步）、`video-api.test.js`（+7：缝端点形态）、`template-studio.test.js`（新 +7）、`yaml-capability.test.js`（新 +1）、`plugin-center.test.js`（+2）、server 内联测试（actions 9 + video_draft 8） |
| 14 | 文档 | `docs/plans/gamer_video_workbench_contracts.md` §5/§6（动作清单契约收口）、`docs/PITFALLS.md` |

## 3. 坐标系决策（§10.2「一致变换」）

- **模板存储空间 = 匹配器的屏幕帧像素空间**：live = 设备显示像素；media = 服务端
  确定帧 PNG 的 oriented 展示像素（ffmpeg autorotate）。与现有 live 模板完全同空间：
  模板 PNG 从帧像素裁出、搜索区域以 0~1 相对坐标进文件名 `#x1_y1_x2_y2`（×1000）。
- 选框直接在 oriented 帧上进行（存储空间即帧空间，恒等）；工作台校准的 reference
  空间用于**对账读数**：`templateStudio.orientedToReference`（content 裁剪 + 单一等比
  因子 + letterbox，接入 `calibration.js`）给出参考坐标，离线测试结果同时报帧空间与
  参考空间坐标。identity 校准下两空间重合。
- 不同参考尺寸的一致变换：区域以相对坐标存文件名，离线测试时若显式传 region 则按
  当前帧像素换算（`regionToPixelRect`）——不缩放模板而忘搜索区。
- 帧身份/校准元数据随 `template.create_from_frame` 请求上行并在响应回显（来源追溯）；
  模板本体保持纯 PNG + 文件名语义（无 sidecar，不污染 templates 目录）。

## 4. 测试结果（本机，`CARGO_PROFILE_DEV_DEBUG=0`，`-j 4`）

| 命令 | 结果 |
| --- | --- |
| `cargo clippy --all-targets` | 0 警告（含 `-D warnings` 口径检查项） |
| `cargo fmt --all -- --check` | PASS |
| `cargo test -- extensions::gamer_yaml extensions::video video_draft actions` | **134 passed / 0 failed** |
| `cargo test -- architecture_guard resources::tests packages_tests api::vision media::tests` | **56 passed / 0 failed**（守卫七测试全绿） |
| `pnpm test:run`（web 全量） | **64 文件 810 passed / 0 failed** |
| `bash tools/e2e_phase7_offline.sh`（无设备 E2E） | **E2E ALL GREEN**：登录→ffmpeg 造视频→导入→帧身份解析（index=15/pts=500000）→建包→装启 gamer.yaml（官方 .gplugin 免签名+权限确认头）→动作缝创建模板（带帧身份+校准）→templates 列表出现且为 8-bit 灰度 PNG（depth=8 color=0 实测）→vision/test 于指定帧命中（score 1.0，响应帧身份=请求帧）→create_draft（重排/注释/text 诊断/source 回查）→save_draft（v3 校验）→automations 列表出现→重名拒绝→yaml stop 后动作缝 409、vision REST 不受影响→临时目录清理 |

## 5. 设计偏差与边界（报告项）

1. **TemplateStudio 为视频域自实现组件**（复用语义与保存链路，不直接复用
   `TemplateCropModal.vue`）：后者与 `useConsoleTemplates` 的 canvas 裁切子系统
   （约 200 行 crop 状态机）深耦合，直接复用需先抽共享裁切层，超出本轮范围；
   交互为「拖拽框选（contain 映射/越界钳制）+ 放大滚轮不做」，语义等价
   （服务端定帧冻结 + generation 校验 + 绝不保存时重抓）。
2. **动作内自建 PackageStore**：`native_call_action` 缝签名只有 data_dir（未改
   `service.rs`），动作内 `PackageStore::open` 同源实例并注册 gamer.yaml 自己的
   内容钩子，保证动作写路径与 REST 写路径同一套 v3 校验/灰度归一化。
3. **媒体引用同步的 before 基准** = 打开项目时的引用快照（`loadedMediaIds`），
   外部改动/素材移除由「保存前后并集 + 包内全部项目重算」覆盖；跨页并发以
   幂等全量替换收敛。项目无素材编辑 UI，外部编辑场景由快照语义兜底。
4. **草稿持久化范围**：草稿 JSON（yaml + diagnostics + source 回查）为前端工作
   流状态（可复制/保存），保存到 automations 的只有 YAML 正文；不自动创建任务、
   不启动 Runner、不自动执行（保存成功仅打开编辑器）。

## 6. NOT_VERIFIED（留 Phase 9）

- 真机运行链路：保存的草稿在真实设备上显式运行验证（一次性消耗行为按计划
  不做自动回放）；本阶段仅验证到「合法 v3 源落盘 + 可被现有运行机制寻址」。
- 多指/旋转/黑边素材上的模板制作人工体验（机制由校准/坐标测试覆盖）。
