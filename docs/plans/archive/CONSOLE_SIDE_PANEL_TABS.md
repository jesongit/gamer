# 控制台右侧面板扩展为五页签（模板 / 脚本 / 日志 / 任务 / 设置）改动方案

> 状态：已实施。本文档记录最终目标形态、工作区已有误改的处置、逐文件改动点与验收标准。
> 背景：需求是「删除左侧边栏 + 在投屏控制台右侧功能面板的 模板/脚本 切换页签处新增 日志/任务/设置，共 5 个页签」。
> 约束：原左侧导航承载的独立 `ScriptEditor.vue` 页面及其入口已一并删除，不保留旧 URL 重定向；主控制台内部的脚本可视化编辑核心仍保留。

## 一、目标形态

1. 全局左侧边栏删除，MainLayout 只留顶栏设备芯片、服务状态、运行芯片、用户/退出；不再保留独立脚本管理入口。
2. 投屏控制台（Console.vue）右侧功能面板顶部的 模板/脚本 切换页签扩展为 **5 个**：
   `🖼️ 模板 | 📜 脚本 | 日志 | 任务 | 设置`。
   - 日志页签：历史运行日志，按「设备+脚本」连续段插分割线分组，可辨不同脚本的运行；
   - 任务页签：首行「＋ 新建任务」按钮，下面任务列表每行只展示 任务名 / 启用开关 / 测试 / 编辑 / 删除；
     编辑走弹窗（新建复用同一弹窗），「测试」= 立即运行一次；
   - 设置页签：系统信息、软件更新、更新策略（原「系统与更新」页全部内容，对接
     `/api/system/info`、`/api/system/update`、`/api/system/update/policy`，与 config.toml 无关）。
3. 删除原左侧导航对应的独立脚本管理页（`ScriptEditor.vue`）及其三栏资源管理外壳；脚本/函数库/模板的主控制台运行与编辑能力保留在 `Console.vue` / `ScriptRunner.vue`。

## 二、工作区现状处置清单（上一轮误改的保留 / 回退）

| 文件 | 处置 | 说明 |
| --- | --- | --- |
| `web/src/layouts/MainLayout.vue` | **保留** | 删除侧边栏与独立脚本管理入口，仅保留顶栏状态 |
| `web/src/views/Console.vue` | **保留并增强** | 清理侧边栏联动；五页签共用稳定的分区操作行，新增左右分区可拖拽宽度；文件内另有本次会话之前就存在的未提交改动，**不可整文件 git checkout** |
| `web/src/components/LogsPanel.vue` | **保留** | 新组件，日志面板（分组分割线 + 5s 轮询），改挂到 Console 右侧面板 |
| `web/src/components/TaskBoard.vue` | **保留** | 新组件，任务面板（新建/列表/复用弹窗/参数签名门禁），改挂到 Console 右侧面板 |
| `web/src/components/SystemPanel.vue` | **保留** | 新组件，设置面板（原设置页全量内容），改挂到 Console 右侧面板 |
| `web/src/views/TaskScheduler.vue` `RunLogs.vue` `Settings.vue` | **保持删除** | 功能分别由三个面板组件承接 |
| `web/src/task-scheduler.test.js` | **保持删除** | 由 `task-board.test.js` 替代 |
| `web/src/task-board.test.js` `logs-panel.test.js` | **保留** | 面板组件行为测试，与挂载位置无关，继续有效 |
| `web/src/settings.test.js` | **保留** | 已指向 `components/SystemPanel.vue`，继续有效 |
| `web/src/console-components.test.js` | **保留并更新** | 删除对独立 ScriptEditor 的静态断言，增加 Console 五页签断言；文件内另有本次会话之前的未提交改动，不可整文件还原 |
| `web/src/views/ScriptEditor.vue` `web/src/script-editor-tabs.test.js` | **删除** | 原左侧导航对应的独立脚本管理页面及其专属外壳测试一并移除；`script-editor/` 共享编辑核心仍保留 |
| `web/src/router.js` | **保留并重做** | 只保留登录、Console 和通用兜底路由；不保留旧资源/任务/日志/设置 URL |
| `AGENTS.md` 关键文件表 | **修正** | 改为记录 Console/ScriptRunner 共享编辑核心，不再列独立 ScriptEditor |

## 三、逐文件改动

### 1. 删除独立 ScriptEditor 页面

删除 `web/src/views/ScriptEditor.vue` 与 `web/src/script-editor-tabs.test.js`，同时移除 MainLayout 的脚本管理导航和路由入口。`web/src/script-editor/`、`useScriptEditorShell.js`、`ScriptRunner.vue` 不删除，因为它们仍由主控制台提供脚本运行/编辑能力。

### 2. router.js：清理旧页面路由

删除 `/scripts`、`/templates`、`/tasks`、`/logs`、`/settings` 全部旧页面路由，不保留兼容重定向；未知地址统一由通用兜底路由回到 `/console`。

### 3. Console.vue：右侧面板扩成五页签（核心改动）

现状（右侧面板 `<aside class="panel">` 内）：

```html
<div class="func-pkg-row"> …分区下拉 + 导入/导出… </div>
<div class="func-tabs">
  <button :class="{ active: panelTab === 'tpl' }" @click="panelTab = 'tpl'">🖼️ 模板</button>
  <button :class="{ active: panelTab === 'script' }" @click="panelTab = 'script'">📜 脚本</button>
</div>
<div v-show="panelTab === 'tpl'" class="panel-sec tpl-tab">…TemplateCapture…</div>
<div v-show="panelTab === 'script'" class="panel-sec script-tab">…ScriptRunner…</div>
```

改动：

1. **页签条**：`panelTab` 类型从 `'tpl' | 'script'` 扩为 `'tpl' | 'script' | 'logs' | 'tasks' | 'settings'`
   （`const panelTab = ref('script')` 初值不变），`.func-tabs` 追加三个按钮：

   ```html
   <button type="button" :class="{ active: panelTab === 'logs' }" @click="panelTab = 'logs'">日志</button>
   <button type="button" :class="{ active: panelTab === 'tasks' }" @click="panelTab = 'tasks'">任务</button>
   <button type="button" :class="{ active: panelTab === 'settings' }" @click="panelTab = 'settings'">设置</button>
   ```

   340px 宽度下 5 个按钮每个约 68px，两字文案可容纳（前两个保留 emoji 也可，挤则去掉 emoji）。

2. **内容区**：tpl/script 两个 `v-show` 区不动；在其后新增三个区块挂面板组件。
   **用 `v-if`/`v-else-if` 而非 `v-show`**——日志面板有 5s 轮询、设置面板有系统状态轮询，
   v-show 会常驻后台轮询，v-if 只在页签激活时挂载：

   ```html
   <div v-if="panelTab === 'logs'" class="panel-sec extra-tab"><LogsPanel /></div>
   <div v-else-if="panelTab === 'tasks'" class="panel-sec extra-tab"><TaskBoard /></div>
   <div v-else-if="panelTab === 'settings'" class="panel-sec extra-tab"><SystemPanel /></div>
   ```

   注意 `v-if` 链与前面两个 `v-show` 区是并列兄弟节点（v-show 不参与 v-if 链，无冲突）。

3. **分区行保持稳定**：`.func-pkg-row`（分区下拉/导入/导出）在五个页签上方始终保留，页签切换不隐藏、不改变顶部布局。

4. **左右分区可拖拽**：在画面区与功能区之间增加 `panel-resizer` 分隔条，右侧面板默认 340px，拖动实时调整到 280–560px 范围；宽度写入本地存储，下次进入 Console 继续使用；分隔条同时支持键盘左右方向键微调。

5. **导入**：`import LogsPanel from '../components/LogsPanel.vue'` 等 three 行。

### 4. 面板组件窄面板适配（实测微调）

三个组件原本按整页设计，放进可调整的 280–560px 面板需要过一遍：

- `LogsPanel.vue`：日志行 `white-space: nowrap`，窄面板内靠 `.log-stream` 横向滚动即可，基本不用动；
  筛选行 `.lp-head` 已 `flex-wrap: wrap`。
- `TaskBoard.vue`：表格为 任务名/启用/操作 三列，340px 可用；`.board-head` 已允许换行。
  弹窗 `.modal` 是全局样式（居中大弹窗），不受面板宽度影响，无需改。
- `SystemPanel.vue`：`.two-col` 栅格 `repeat(auto-fit, minmax(360px, 1fr))` 在较窄面板自动变单列；
  系统信息依赖表（5 列）最挤，必要时给卡片内表格套 `overflow-x: auto`。根节点
  `.system-panel` 已是 `overflow: auto` 的纵向滚动容器，与 `.panel-sec.extra-tab` 的
  `flex: 1; min-height: 0;` 配合即可撑满面板高度。

`.panel-sec` 基础类已有 `flex: 1; min-height: 0; overflow: hidden`（tpl-tab/script-tab 附加规则同款），
新增 `.panel-sec.extra-tab { flex: 1; min-height: 0; overflow: hidden; display: flex; flex-direction: column; }`，
让内部面板组件自行滚动。

### 5. 文档与 AGENTS.md

- `AGENTS.md` 关键文件表：
  - 删除独立 `web/src/views/ScriptEditor.vue` 行，补充 `ScriptRunner.vue / useScriptEditorShell.js` 为主控制台共享编辑核心；
  - `web/src/components/ScriptPicker.vue` 行的「TaskScheduler」改为「TaskBoard」；
  - 新增一行：`web/src/components/LogsPanel.vue / TaskBoard.vue / SystemPanel.vue`｜投屏控制台右侧面板的
    日志/任务/设置页签（Console.vue 五页签：模板/脚本/日志/任务/设置；旧独立页面与 URL 已删除）。
- `docs/PITFALLS.md`：如实施中踩坑（如面板内表格挤压、轮询常驻）按惯例补条目。

## 四、测试与验收

测试（改动后应全绿，`cd web && pnpm run test:run`）：

- `task-board.test.js` / `logs-panel.test.js` / `settings.test.js`：不动，应通过；
- `console.test.js` / `console-components.test.js`：保留主控制台回归，并断言五个面板页签与三个面板组件挂载。

验收清单：

1. 侧边栏不再出现；顶栏不再出现独立脚本管理入口；
2. 投屏控制台右侧面板为 5 个页签：模板 / 脚本 / 日志 / 任务 / 设置，切换正常、互不串内容；
3. 日志页签：时间正序、按「设备+脚本」分段带分割线组头、5s 自动刷新、筛选/清空可用；
4. 任务页签：新建按钮 + 精简列表（任务名/启用/测试/编辑/删除），新建与编辑共用弹窗，测试即立即运行，
   参数过期（param_stale）任务测试按钮禁用并提示；
5. 设置页签：系统信息/软件更新/更新策略完整可用（接口为 /api/system/*，非 config.toml）；
6. 旧 ScriptEditor/TaskScheduler/RunLogs/Settings 页面与 URL 入口均已删除；
7. 主控制台脚本运行/编辑/函数测试不受影响；
8. 页签切换不隐藏顶部包名/导入/导出行、不自动改变面板宽度；拖拽分隔条可调整左右分区宽度，切走后台无轮询请求（v-if 生效）。
