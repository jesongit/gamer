# GameBot 无兼容基线并行开发计划

> 状态：基线清理完成；本机/Docker/真机/浏览器（WebRTC、DataChannel、viewer 接管、重连、watchdog/idle）验收完成（2026-09-01）；发布链路验收在 docs/AUTO_UPDATE_DEVELOPMENT_PLAN.md 单独跟踪；未完成项与阻塞原因统一见 [docs/REMAINING_BLOCKERS.md](REMAINING_BLOCKERS.md)
>
> 目标：项目仍处于开发阶段，删除旧协议、旧数据和旧容错路径，形成单一、严格、可测试的当前基线。
>
> 原则：可以重建开发数据，不做旧版本升级；运行可靠性降级与未来正式版本迁移不属于“旧兼容”，必须保留或重新建立。

## 1. 结果定义

本轮结束后，仓库只存在一套当前契约：

- 运行只按 `run_id` 管理，不再按 `script_id` 停止或查询。
- 脚本、函数、模板只有一套创建、更新、替换和冲突处理语义。
- YAML 保存、导入、运行、任务校验共用 v2 严格装载器，不再接受或迁移 v1 形态。
- SQLite 从明确的 schema v1 空库建立，不扫描并修补无版本旧表。
- 任务参数快照和签名为必需字段，不存在 `no_snapshot` 旧任务状态。
- 认证只支持当前凭据格式，不再升级旧 SHA-256、配置明文或前端伪 token。
- 前端不再探测旧接口、不再兼容旧响应结构、不再保留旧路由重定向。
- 系统状态、版本、时区和部署说明来自真实数据，不显示原型设置或硬编码状态。

本轮允许的破坏性结果：

- 删除 `server/data/gamer.db` 后重建开发数据库。
- 手工整理或重新导入当前三目录资源；不自动迁移旧 `data/scripts`、全局 `data/templates`。
- 旧前端缓存、旧书签、旧 token、旧 API 客户端直接失效。
- v1 YAML、旧颜色 `else` 结构、旧模板上传 body 直接返回当前结构化错误。

## 2. 边界：删除兼容，不删除可靠性

| 直接删除 | 必须保留 |
|---|---|
| `/api/scripts/:id/stop`、`/status` 等脚本级运行接口 | DataChannel 控制失败后 REST 控制降级 |
| 旧运行响应归一化、缺接口探测、`runScriptId` | WebRTC 看门狗、viewer 接管和重连仲裁 |
| 旧模板上传 `{name,data_b64,pkg}` | 在模式适用时的截图/设备命令降级 |
| 缺少 `expected_version` 即强制覆盖 | 409 版本冲突和显式的 `force` 操作 |
| 文件布局启动迁移、`package` 指令清理 | 当前 `yaml/func/tmpl` 分区及 ZIP 可缺目录规则 |
| YAML v1 特判、旧颜色分支容错 | v2 结构化诊断与前后端一致校验 |
| SHA-256、配置明文密码、`gb_token` 清理 | 当前 Cookie 会话和 Argon2id 校验 |
| `PRAGMA table_info` 式旧库补列、旧任务修复 | 从 schema v1 开始的未来显式顺序迁移 |
| `/templates` 旧路由重定向 | Vite chunk 加载失败后的页面恢复 |

`.yaml` 与 `.yml` 当前都在真实数据中使用，本轮不顺带统一扩展名；如需统一，应另开一个带批量改名和引用检查的任务。

## 3. 先冻结的当前契约

### 3.1 运行 API

- `POST /api/scripts/:id/run`：成功固定返回 HTTP 202 和 `{run_id,state,resolved_args}`。
- `POST /api/functions/:id/run`：同样固定返回 `run_id`。
- `GET /api/runs/:run_id`：查询单次运行。
- `POST /api/runs/:run_id/cancel`：取消单次运行。
- 设备活动运行查询固定返回 `{active:false}` 或 `{active:true,run:<RunRecord>}`，禁止多种形态并存。
- 删除脚本级 stop/status route、RunManager 旧索引和前端回退。

### 3.2 资源写入 API

- `POST` 只创建；资源已存在返回 409。
- `PUT` 只更新；默认必须携带 `expected_version`。
- 确需覆盖时使用明确的 `force:true`，不得用“省略版本”表达覆盖。
- 重命名仍属于更新，必须进行版本检查。
- 模板创建固定使用 `{short_name,region?,pkg,data_b64}`。
- 模板图片替换使用显式端点 `PUT /api/templates/:name/image?pkg=...`，body 为 `{data_b64}`；不能复用旧上传 body 暗示覆盖。
- 前端 API 命名区分 `create*`、`update*`、`replaceTemplateImage`，不保留含糊的 `uploadTemplate`。

### 3.3 YAML 与文件资源

- 保存、导入预检、运行、函数测试和任务保存调用同一严格 AST 装载器，仅通过“资源类型/是否解析引用”等校验模式表达差异。
- 移除 `LEGACY_TOP_KEYS`、`script.top_level.legacy_format`、旧嵌套 color `else` 容错及相应 fixture。
- 旧字段按当前未知字段或结构错误处理，不额外提供旧语法迁移提示。
- 删除 `scripts::migrate_fs_layout` 及启动调用；检测到旧目录时直接报清晰的启动错误，提示开发者手工删除或导出重建，但程序不搬运。
- 保留当前短模板名、分区隔离和 ZIP 当前布局，不扩大本轮协议面。

### 3.4 数据库与任务

- 建立 schema v1，完整创建当前 devices/tasks/logs 等表。
- `tasks.args_json`、`tasks.param_signature` 改为非空；无参数保存 `{}` 与对应签名。
- 删除 `NoSnapshot`、`reason=no_snapshot` 及旧任务修复分支。
- 本轮不提供“无版本数据库 → v1”的 migration 0：执行前备份，随后重建开发库。
- schema 版本不匹配时 fail fast，不猜测表结构。
- 未来自动更新只允许显式的 `v1 -> v2 -> ...` 顺序迁移；这与删除开发期旧兼容不冲突。

### 3.5 认证与配置

- 配置文件只接受 Argon2id PHC hash；初次开发启动可通过 `GAMER_ADMIN_PASSWORD` 注入明文并仅在进程内使用。
- 删除旧 SHA-256 解析、内存升级、配置文件明文 `password` 和相关测试。
- 删除前端 `gb_token` 清理逻辑；认证状态只来自服务端会话。
- 示例配置和 README 不再给出默认管理员明文凭据。

## 4. 并行模型

### 4.1 分支与工作区

建议建立一个集成分支和每条支线独立 worktree：

```text
codex/clean-baseline               集成分支
codex/clean-run-api                A
codex/clean-resources-yaml         B
codex/clean-schema-tasks           C
codex/clean-auth                   D
codex/clean-web-contract           E
codex/clean-web-views              F
codex/clean-truth-ops              G
codex/clean-validation-docs        H
```

同一时刻最多并行四条支线。每条支线只提交自己的路径；禁止顺手格式化全仓、更新锁文件或修理其他支线测试。集成负责人在每一波结束后合并，不与支线同时修改公共热点。

当前工作区已有未提交的编辑器、数据、测试和文档改动。执行本计划前必须先把这些改动整理成可识别提交；不能用 reset、checkout 或清理未跟踪文件制造“干净基线”。

### 4.2 公共热点所有权

| 公共文件 | 唯一负责人 | 支线提交方式 |
|---|---|---|
| `server/src/api/mod.rs` | 集成负责人 | 支线在交接说明中列出要增删的 route |
| `server/src/api/tests.rs` | 波次 0 拆分负责人 | 之后各支线只改对应测试模块 |
| `web/src/api.js` | E：前端契约 | 其他支线只提交调用点需求清单 |
| `web/src/store.js`、`web/src/runs.js` | E：前端契约 | F 在 E 合并后开始 |
| `web/src/views/Console.vue` | F：前端视图 | 其他支线不碰 |
| `web/src/views/ScriptEditor.vue` | F：前端视图 | 其他支线不碰 |
| `docs/YAML.md`、`docs/SCRIPT_EDITOR_CONTRACT.md` | H：验证与文档 | B 提供契约变化清单与 fixture |
| `README.md`、`AGENTS.md`、`docs/PITFALLS.md` | H：验证与文档 | 只有真实新坑才追加 PITFALLS |
| `server/data/**` | H 或指定数据负责人 | B 只改测试 fixture，避免覆盖开发数据 |

## 5. 执行波次

### 波次 0：串行建立可并行基线（0.5～1 天）

负责人：集成负责人。

1. 整理并提交当前未完成改动，记录基线 commit。
2. 备份开发数据库与资源目录；确认本轮允许重建 DB。
3. 将 `server/src/api/tests.rs` 按 `runs/resources/auth/tasks/system` 拆为测试模块，只做机械移动。
4. 把本计划中的 API、schema、YAML 决策视为冻结契约；存在异议先改计划，不能由支线自行发明第二套协议。
5. 运行完整基线门禁并保存结果。

门禁：

```powershell
cd server
cargo fmt --check
cargo test

cd ../web
pnpm test:run
pnpm build
```

### 波次 1：服务端清理，四路并行（1.5～3 天）

#### A — 运行契约

允许修改：

- `server/src/api/runs.rs`
- `server/src/run_manager.rs`
- 拆分后的 `server/src/api/tests/runs.rs`

任务：

1. 删除按 `script_id` stop/status 的 handler、状态映射和不再使用的索引。
2. 固定 start/status/active/cancel 响应结构和状态码。
3. 验证取消幂等、设备冲突 409、函数运行和任务运行不受影响。
4. 向集成负责人提交 route 删除清单，不直接改 `api/mod.rs`。

完成标准：运行相关测试不包含 `legacy`、旧 `{running:...}` 或 `{ok:true}` 断言。

#### B — 资源与 YAML

允许修改：

- `server/src/api/scripts.rs`
- `server/src/api/functions.rs`
- `server/src/api/templates.rs`
- `server/src/scripts.rs`
- `server/src/script_v2/**`
- `server/tests/fixtures/script_v2/**`
- 拆分后的 resources/YAML 测试

任务：

1. 建立 create/update/force/rename/replace 的唯一资源契约。
2. 模板创建和图片替换彻底分离，删除旧 body 分支。
3. 让所有资源入口复用严格 v2 装载与统一诊断。
4. 删除文件布局迁移、旧顶层键表、旧 color `else` 容错和旧 fixture。
5. 提交前端 API 变更表、文档变化表给 E/H。

完成标准：相同非法 YAML 在保存、导入预检、运行准备、任务保存中返回同一诊断 code/field/step_path。

#### C — schema 与任务

允许修改：

- `server/src/store.rs`
- `server/src/task_params.rs`
- `server/src/scheduler.rs`
- `server/src/api/tasks.rs`
- 对应测试

任务：

1. 用 schema v1 一次性创建完整当前表，删除启动补列和旧重复任务修复。
2. 将任务参数快照/签名收紧为非空，删除 `NoSnapshot` 全链路。
3. schema 版本异常时给出可行动的启动错误。
4. 为未来迁移保留单一入口，但不实现旧无版本库迁移。

完成标准：空目录可启动并创建 v1；人为构造 unversioned/错误版本库时拒绝启动；任务无参数和有参数均可保存、触发和立即运行。

#### D — 认证

允许修改：

- `server/src/api/auth.rs`
- `server/src/config.rs`
- `server/config.example.toml`
- 对应认证测试

任务：

1. 删除 SHA-256、明文配置和内存升级路径。
2. 固定 Argon2id PHC 配置与环境变量开发入口。
3. 测试正确密码、错误密码、无效 hash、会话过期和未配置凭据。
4. 输出前端删改清单给 E，输出部署说明给 G/H。

完成标准：生产代码中不再存在旧 hash parser 或默认管理员明文。

### 波次 1 集成门禁（0.5 天）

1. 集成负责人统一修改 `server/src/api/mod.rs`。
2. 按 C → D → B → A 顺序合并，逐个解决编译错误，禁止一次性堆叠四条支线后再排错。
3. 删除并重建测试数据库，执行服务端完整测试。
4. 用 HTTP 冒烟测试确认旧 endpoint 为 404、旧 body 为 400/422、当前 endpoint 正常。

未通过本门禁，不启动波次 2。

### 波次 2：前端、产品真相与文档，四路并行（1.5～3 天）

#### E — 前端核心契约

允许修改：

- `web/src/api.js`
- `web/src/runs.js`
- `web/src/store.js`
- `web/src/auth.js`
- 上述模块测试

任务：

1. API 客户端只暴露当前 run/resource/auth 方法。
2. 删除旧响应归一化、endpoint 缺失探测、`runScriptId` 和 `gb_token`。
3. 为 F 提供简洁稳定的调用接口与错误对象。
4. 删除专门验证旧兼容的测试，新增当前契约测试。

完成标准：生产代码不通过 try/catch 探测 API 版本；强制覆盖只能显式传 `force:true`。

#### F — 前端调用点与交互收口

前置：E 的接口先合入 F 分支或以固定 commit 为基线。

允许修改：

- `web/src/layouts/MainLayout.vue`
- `web/src/views/Console.vue`
- `web/src/views/ScriptEditor.vue`
- `web/src/views/TaskScheduler.vue`
- `web/src/components/console/**`
- 相应组件测试

任务：

1. 全部运行、取消、轮询切换为 `run_id`。
2. 模板新建与替换调用不同方法；脚本/函数覆盖显式传 force。
3. 删除 `no_snapshot`、旧 endpoint 和旧响应 UI 分支。
4. 不在本支线顺带重构编辑器架构；只完成契约迁移和失效状态清理。

完成标准：手动脚本、函数测试、从指定步骤运行、任务立即运行、取消均只走当前 API。

#### G — 真实状态、部署与时区

允许修改：

- `web/src/views/Settings.vue`
- 系统信息/健康状态的新后端模块和前端组件
- `docker-compose.yml`、部署配置文件
- 与上述功能直接相关的测试

任务：

1. 把原型设置改为只读“系统状态”，或在真实接口完成前从导航隐藏。
2. 版本、readiness、ADB/ffmpeg/scrcpy/data/DB 状态来自服务端，删除硬编码 `v0.1.0` 和日志徽标。
3. 任务页/系统状态显示服务端时区；Docker 显式设置 `TZ`，部署文档说明 WebRTC UDP 端口。
4. 复用自动更新计划中的 system info/schema version，不创建重复版本接口。

完成标准：页面不再显示无法保存的设置；容器与本机均能解释“任务按哪个时区运行”和“黑屏依赖是否就绪”。

#### H — 数据、验证与文档

允许修改：

- `server/data/**`
- `docs/YAML.md`
- `docs/SCRIPT_EDITOR_CONTRACT.md`
- `docs/PITFALLS.md`
- `README.md`
- `AGENTS.md`
- `docs/AUTO_UPDATE_DEVELOPMENT_PLAN.md`
- 契约/集成测试，但不改业务实现

任务：

1. 清理仓库内旧 fixture/示例/开发数据，确保都能被当前严格 loader 读取。
2. 同步 YAML 前端校验、模板代码、权威文档和可执行 fixture。
3. 更新自动更新计划：基线从 schema v1 开始，删除 migration 0 设想。
4. 修正 README、配置、自动启动、STUN、目录布局等产品真相漂移。
5. 追加实际遇到的新构建/运行坑；不要把设计决策或开发流水写进 PITFALLS。

完成标准：文档只描述当前行为；仓库示例可被加载；AGENTS、YAML、契约和测试互相一致。

### 波次 2 集成门禁（0.5～1 天）

合并顺序：E → F → G → H。集成负责人解决公共文件，不让 H 用文档提交覆盖已合入的用户改动。

必须完成以下人工流程：

1. 空数据目录启动，登录，扫描并连接设备。
2. 新建脚本/函数/模板，更新、重命名、版本冲突、显式强制覆盖。
3. 手动运行、从指定步骤运行、函数测试、取消和运行冲突。
4. 创建无参数/有参数定时任务，立即运行，修改参数声明后重新确认。
5. 导出当前分区，清空后导入，验证 yaml/func/tmpl。
6. Docker 环境核对 readiness、时区和 WebRTC UDP。

### 波次 3：性能实验，独立提交，可延后

本波次不与协议清理混合，只有测量证明收益后才进入主分支：

- 检测无新视频帧时跳过重复 ffmpeg 解码，并记录命中率与等待延迟。
- 音频默认关闭或按需启用。
- 评估初始 GOP 缓存与截图解码职责解耦。
- 合并多个恢复入口前，先画出 watchdog/idle/viewer 的状态所有权并做故障注入。

不要引入 OpenCV、常驻 PNG 解码流、多 viewer、脚本排队或重量级前端状态框架；这些都没有当前证据支持。

## 6. 每条支线的统一交付格式

每个 Agent 完成时必须提供：

1. 修改文件列表。
2. 删除了哪些旧路径，以及保留了哪些可靠性路径。
3. 对其他支线的接口变化清单。
4. 执行过的测试和结果。
5. 未解决风险；没有则明确写“无”。
6. 一个自洽的 Conventional Commit，不混入其他主题。

禁止事项：

- 不修改不属于本支线的公共热点。
- 不为了让旧测试通过而恢复兼容分支。
- 不吞掉诊断或把严格错误改成静默默认值。
- 不直接删除当前工作区的用户改动或开发数据。
- 不以“之后再清理”为由同时保留新旧两套 API。

## 7. 提交建议

```text
test(api): 按领域拆分接口测试以支持并行开发
refactor(api)!: 删除脚本级运行兼容接口
refactor(data)!: 建立无旧迁移的当前数据基线
refactor(api)!: 统一资源写入与严格校验契约
refactor(auth)!: 删除旧凭据与伪令牌兼容
refactor(web)!: 全面切换 run_id 与唯一资源契约
fix(ops): 收口正式部署入口与系统状态
docs(agents): 更新无兼容基线与开发约束
```

所有 `!` 提交正文末尾必须带：

```text
BREAKING CHANGE: 项目开发基线已重置，不支持旧接口、旧数据库、旧凭据或旧 YAML；请备份后重建开发数据。
```

## 8. 最终验收

### 自动门禁

```powershell
cd server
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test

cd ../web
pnpm test:run
pnpm build
```

清理扫描应排除历史计划文档，只检查生产代码与当前测试：

```powershell
rg -n "scriptStatus|runScriptId|legacyRun|gb_token|LEGACY_TOP_KEYS|migrate_fs_layout|parse_legacy_sha256_hash|no_snapshot" server/src web/src
rg -n "/api/scripts/.+/(stop|status)" server/src web/src
```

预期：无生产代码命中；若当前测试需要验证旧请求被拒绝，只允许出现在明确命名的 rejection 测试中。

### 完成定义

- 空环境可一次启动，错误环境能 fail fast 并给出可行动原因。
- 前后端只有一套 API、YAML、schema、认证与资源写入契约。
- 不读取、不迁移、不升级旧开发数据。
- 运行可靠性回退和设备生命周期约束未被误删。
- 全量自动测试、前端构建和核心人工流程通过。
- README、AGENTS、YAML 文档、示例配置与实际行为一致。
- 每个破坏性提交可独立理解，集成分支没有夹带当前未完成工作的覆盖或丢失。

## 9. 可复制的 Agent 总提示词

```text
你正在执行 GameBot“无兼容基线”计划中的【支线 X】。项目仍在开发期，不需要兼容旧 API、旧响应、旧数据库、旧凭据、旧 YAML 或旧文件布局；请删除旧分支，不要保留双轨。注意：DataChannel→REST、WebRTC 恢复、设备命令/截图的模式安全降级属于运行可靠性，不得当作旧兼容删除。

只修改计划为本支线分配的文件。公共热点由集成负责人处理；发现跨支线需求时输出精确变更清单，不越界编辑。先阅读 AGENTS.md 和 docs/CLEAN_BASELINE_PARALLEL_PLAN.md 对应章节，再检查真实调用与测试。实现后运行本支线测试；不要为了旧测试恢复兼容逻辑，应改成当前契约或“旧输入被拒绝”测试。

交付时列出：修改文件、删除的旧路径、保留的可靠性路径、跨支线接口变化、测试命令与结果、剩余风险。提交遵循 Conventional Commits；破坏性改动使用 ! 并添加 BREAKING CHANGE。
```

## 10. 执行 Checklist

清单由集成负责人维护。支线 Agent 只报告结果，确认提交已合并且对应门禁通过后再勾选，不能以“代码已写完”代替集成完成。

> 证据索引（2026-08-31）：收口记录为 `2c28531`，清理提交 `5ea58f2` 已移除 `docker-config.toml` 明文 `admin123`、Login 默认凭据/固定版本、MainLayout 固定版本与日志徽标，并有 493 项前端测试。生产代码静态扫描为 0 命中；`cargo fmt`/clippy/test、Web 测试/build 和 compose config 均通过（Rust 306 passed/2 ignored，Web 493 passed）。临时服务使用一次性 `GAMER_ADMIN_PASSWORD` 冒烟：`/health/ready` 200，错误登录 401，正确登录 200 并设置 `gb_session`，`/api/session`、`/api/system/info`（schema=1，ADB/FFmpeg/scrcpy 版本可见）、`/api/devices/scan` 均 200；真实设备 connect 200 且为 online mirror，shutdown 200，未留下 gamer-server/8443/adb forward/reverse/scrcpy 残留。应用内浏览器因 `ERR_BLOCKED_BY_CLIENT` 未验证 WebRTC/DataChannel；Docker daemon 不可用，未伪造 Docker readiness/UDP 结果；设备脚本、viewer 接管、watchdog/idle 生命周期也未实际验证。

### 10.1 波次 0：准备与基线

- [x] 当前未提交改动已逐项确认归属，没有未知来源文件。
- [x] 当前编辑器、数据、测试和文档工作已整理为可识别提交。
- [x] 已记录无兼容清理开始前的基线 commit。
- [x] `server/data/gamer.db` 已备份；确认可以删除并重建开发库。
- [x] `server/data/**` 当前资源已备份或确认可以重新生成。
- [x] 已确认 `.yaml`/`.yml` 扩展名不在本轮统一。
- [x] `server/src/api/tests.rs` 已按领域机械拆分，且没有改变测试语义。
- [x] 运行 API、资源写入、YAML、schema、认证契约已冻结。
- [x] `cargo fmt --check` 基线通过。
- [x] `cargo test` 基线通过。
- [x] `pnpm test:run` 基线通过。
- [x] `pnpm build` 基线通过。

> 波次 0 证据：基线 commit `038a2ad56159b40b4d8d8cfc685676ea7980b36d`；备份 `baseline-backups/wave0-20260831-015238723/`；四项门禁均通过（`cargo test` 297 passed/2 ignored，`pnpm test:run` 486 passed）。
- [x] 已为 A～D 创建独立分支指针，并指定负责人（本次不切换工作区、不创建 worktree）。

### 10.2 波次 1：服务端四路并行

#### A — 运行契约

- [x] 删除脚本级 stop/status handler。
- [x] 删除按 `script_id` 查询或取消运行的旧索引和方法。
- [x] start 固定返回 HTTP 202、`run_id`、`state` 和 `resolved_args`。
- [x] active 查询固定为 `{active:false}` 或 `{active:true,run:...}`。
- [x] cancel/status 只接受 `run_id`。
- [x] 设备运行冲突仍返回 409。
- [x] 取消幂等、脚本运行、函数测试和任务运行测试通过。
- [x] route 删除清单已交给集成负责人。
- [x] A 支线提交已完成并附 `BREAKING CHANGE`。

#### B — 资源与 YAML

- [x] 脚本、函数的 POST 仅创建，目标存在返回 409。
- [x] 脚本、函数的 PUT 更新必须带 `expected_version` 或显式 `force:true`。
- [x] 重命名执行版本检查。
- [x] 模板创建只接受 `{short_name,region?,pkg,data_b64}`。
- [x] 模板图片替换使用独立端点，不再接受旧上传 body。
- [x] 保存、导入预检、运行准备和任务保存复用严格 v2 loader。
- [x] 删除 `LEGACY_TOP_KEYS` 和 `script.top_level.legacy_format`。
- [x] 删除旧嵌套 color `else` 容错及 fixture。
- [x] 删除 `scripts::migrate_fs_layout` 和启动调用。
- [x] 旧目录存在时 fail fast，程序不自动搬运。
- [x] 同一非法 YAML 的多入口诊断一致性测试通过。
- [x] 前端 API 变化清单已交给 E。
- [x] YAML/fixture 文档变化清单已交给 H。
- [x] B 支线提交已完成并附 `BREAKING CHANGE`。

> B→E/H 交接证据（2026-08-31）：E 提交 `fd896de` 正文逐项列出资源 create/update/replace、`expected_version`/`force`、`run_id` 与认证清理；H 提交 `d7075de` 的变更集覆盖 `docs/YAML.md`、契约文档、服务端/前端 fixture 及当前分区资源，当前 fixture 严格 loader 测试通过。

#### C — schema 与任务

- [x] schema v1 可以从空目录创建完整数据库。
- [x] 删除 `PRAGMA table_info` 式旧库补列路径。
- [x] 删除旧重复任务修复路径。
- [x] `tasks.args_json` 和 `tasks.param_signature` 为非空。
- [x] 无参数任务保存 `{}` 和有效签名。
- [x] 删除 `NoSnapshot`、`reason=no_snapshot` 及相关测试/UI 契约。
- [x] unversioned 或错误 schema 版本数据库会拒绝启动并给出明确原因。
- [x] 未来 schema 迁移保留单一入口，但没有 migration 0。
- [x] 任务保存、立即运行、cron 和重新确认测试通过。
- [x] C 支线提交已完成并附 `BREAKING CHANGE`。

#### D — 认证

- [x] 删除旧 SHA-256 parser 和测试。
- [x] 删除配置文件明文密码支持。
- [x] 删除凭据内存升级路径。
- [x] 配置文件只接受 Argon2id PHC hash。
- [x] `GAMER_ADMIN_PASSWORD` 只作为开发启动输入，不持久化明文。
- [x] 正确密码、错误密码、无效 hash、会话过期测试通过。
- [x] 示例配置不再包含默认管理员明文。
- [x] 前端认证删改清单已交给 E。
- [x] 部署说明变化已交给 G/H。
- [x] D 支线提交已完成并附 `BREAKING CHANGE`。

> D→E/G/H 交接证据（2026-08-31）：E 提交 `fd896de` 正文明确记录仅使用 `gb_session` Cookie、删除伪 token/旧响应；G 提交 `8e3c026` 覆盖 compose、Docker 配置与系统状态，H 提交 `d7075de` 覆盖 README、AGENTS、示例配置及相关文档。

### 10.3 波次 1：服务端集成门禁

- [x] 集成负责人已统一更新 `server/src/api/mod.rs`。
- [ ] 已按 C → D → B → A 顺序合并。
- [ ] 每合并一条支线均单独运行了对应测试。
- [x] 已删除并重建开发测试数据库。
- [x] 旧 run endpoint 返回 404。
- [x] 旧资源 body 被明确拒绝，不会静默转换。
- [x] 当前 run/resource/auth/task endpoint 冒烟测试通过。
- [x] `cargo fmt --check` 通过。
- [x] `cargo clippy --all-targets --all-features -- -D warnings` 通过。
- [x] `cargo test` 通过。
- [x] 已为 E～H 创建独立分支/worktree，并指定负责人。

> 波次 1 顺序/逐支线测试审计（2026-08-31）：`df1e3e5` 报告实际父链为 A→D→C→B，未满足 C→D→B→A；提交报告只给出集成门禁，未提供每条支线单独测试记录，因此上述两项保持未勾选。

> 波次 1 集成证据（2026-08-31）：最终 commit `1c1fef8`；`cargo fmt --check`、`cargo clippy --all-targets --all-features -- -D warnings` 通过，`cargo test` 299 passed/2 ignored；空库实际启动后 schema v1 为 `user_version=1` 且表完整，重建前 DB 已移入 `baseline-backups/wave1-20260831-db-rebuild/database-pre-rebuild/`；旧 stop/status=404、旧模板 body=422，当前 run/resource/auth/task smoke 通过。
> E～H 分支证据（2026-08-31）：`codex/clean-web-contract`→`fd896de`、`codex/clean-web-views`→`6ee4b23`、`codex/clean-truth-ops`→`8e3c026`、`codex/clean-validation-docs`→`d7075de`；本次以不切换工作区的本地分支指针完成，不创建额外 worktree。

### 10.4 波次 2：前端、产品真相与文档

#### E — 前端核心契约

- [x] `web/src/api.js` 只暴露当前 run/resource/auth API。
- [x] 删除旧运行响应归一化。
- [x] 删除 missing endpoint 探测和兼容 fallback。
- [x] 删除 `runScriptId`。
- [x] 删除 `gb_token` 清理逻辑。
- [x] 资源 API 明确区分 create/update/replace。
- [x] 强制覆盖只能显式传 `force:true`。
- [x] 旧兼容测试已删除或改为旧输入拒绝测试。
- [x] 当前契约单元测试通过。
- [x] E 已先行合入，F 使用固定 commit 开始迁移。

#### F — 前端调用点与交互

- [x] MainLayout 取消运行只使用 `run_id`。
- [x] Console 运行、轮询和取消只使用当前 API。
- [x] ScriptEditor 脚本运行和函数测试只使用当前 API。
- [x] TaskScheduler 立即运行只使用当前 API。
- [x] 模板新建和模板图片替换走不同方法。
- [x] 脚本/函数强制覆盖显式传 `force:true`。
- [x] 删除 `no_snapshot` UI 分支。
- [x] 删除旧 endpoint/旧响应 fallback UI。
- [x] 手动运行、从指定步骤运行、函数测试、任务立即运行和取消流程通过。
- [x] F 支线提交已完成并附 `BREAKING CHANGE`。

#### G — 真实状态、部署与时区

- [x] 原型 Settings 已改为真实只读系统状态，或已从导航隐藏。
- [x] 版本来自服务端，不再硬编码 `v0.1.0`。
- [x] readiness、ADB、ffmpeg、scrcpy、data、DB 状态可见。
- [x] 固定日志徽标已删除。
- [x] 页面明确显示任务使用的服务端时区。
- [x] Docker 显式配置 `TZ`。
- [x] 部署入口明确说明 WebRTC UDP 端口。
- [x] system info/schema version 与自动更新计划共用一套定义。
- [x] 本机与 Docker 状态页冒烟测试通过。（本机浏览器状态页与 /api/system/info 逐项一致：docs/CLEAN_BASELINE_FUNC_EVIDENCE.md；Docker 浏览器状态页 docker/external、依赖三行正常、更新能力禁用、控制台 0 报错：docs/UPDATE_DOCKER_E2E_EVIDENCE.md 状态页冒烟节；首轮发现的服务端时区显示缺失缺陷已修复——server 7166f1f + web f27acdd）
- [x] G 支线提交已完成。

> G 最终复核（2026-08-31）：`5ea58f2` 已移除 `docker-config.toml` 明文 `admin123`、Login 默认凭据/固定版本、MainLayout 固定版本与日志徽标；新增的基线真相测试随 Web 493 项测试通过。`/api/system/info` 认证后返回 schema=1 及 ADB/FFmpeg/scrcpy 版本，故版本与固定徽标项勾选；Docker runtime 与浏览器页面状态未实测，综合状态页项保持未勾选。

#### H — 数据、验证与文档

- [x] 仓库当前脚本/函数示例均通过严格 loader。
- [x] 旧 fixture、旧开发示例和旧迁移说明已删除。
- [x] 前端 YAML 校验与服务端当前契约一致。
- [x] 模板代码与当前 YAML 契约一致。
- [x] `docs/YAML.md` 已同步。
- [x] `docs/SCRIPT_EDITOR_CONTRACT.md` 已同步。
- [x] `docs/AUTO_UPDATE_DEVELOPMENT_PLAN.md` 从 schema v1 开始，不再规划 migration 0。
- [x] README、AGENTS、示例配置只描述当前行为。
- [x] 自动启动、STUN、目录布局等漂移文案已修正。
- [x] 仅将真实新增构建/运行坑追加到 `docs/PITFALLS.md`。
- [x] H 支线提交已完成。

### 10.5 波次 2：集成与人工验收

- [ ] 已按 E → F → G → H 顺序合并。
- [x] 合并未覆盖波次开始前的用户改动或开发数据。
- [x] 空数据目录启动成功并创建 schema v1。
- [x] 登录、设备扫描、连接和投屏的 HTTP/session 冒烟成功（真实设备为 online mirror；浏览器视频轨道未验证）。
- [x] 脚本/函数/模板的新建、更新、重命名成功。
- [x] 版本冲突返回 409，显式强制覆盖成功。
- [x] 手动运行、从指定步骤运行、函数测试和取消成功。
- [x] 同设备并发运行仍被 409 拒绝。
- [x] 无参数/有参数定时任务保存和立即运行成功。
- [x] 参数声明变化后任务重新确认流程成功。
- [x] 当前分区导出、清空、重新导入成功。
- [x] DataChannel 控制正常，REST 可靠性降级仍可用。（DC 静音开关命中服务端独有日志、DC 触控设备侧生效；REST press/tap 200 且生效——docs/CLEAN_BASELINE_FUNC_EVIDENCE.md）
- [x] viewer 接管、重连、watchdog 和 idle 生命周期未回归。（接管/被顶不重连/idle 拆会话与唤醒：FUNC_EVIDENCE 首轮；配置变更踢 viewer 后自动重连 4.4s 恢复、watchdog 确死强拆 viewer 重连 25s 恢复、脚本运行中确死强拆服务端 132ms 重连且 run Success：G3 回归节；回归暴露的「自动重连成功一次后再次被踢不再重连」前端缺陷已修复并加测试——539a073，web 580 tests 全绿）
- [ ] Docker readiness、时区和 WebRTC UDP 验证通过。

> 波次 2 集成证据（2026-08-31）：公共 `GET /api/system/info` 已在 `protected_json` 接入，未放入 public；集成测试验证未认证 401、认证后结构化响应和无临时路径泄露。`cargo fmt --check`、`cargo clippy --all-targets --all-features -- -D warnings`、`cargo test`（306 passed/2 ignored）、`pnpm test:run`（493 passed）、`pnpm build` 通过；`docker-compose.yml`、主+`docker-compose.local.yml`、主+`docker-compose.usb.yml` 的 `config --quiet` 通过。一次性 `GAMER_ADMIN_PASSWORD` 临时服务已完成 readiness、错误/正确登录、session、system info、设备扫描、真实设备 connect（online mirror）和 shutdown 冒烟，且无进程/端口/ADB/scrcpy 残留；该结果覆盖 HTTP/session，浏览器视频轨道仍未验证。应用内浏览器 `ERR_BLOCKED_BY_CLIENT` 阻断 WebRTC/DataChannel 验证；Docker daemon 不可用，不能伪造容器 readiness/UDP；设备脚本、viewer 接管、watchdog/idle 生命周期及发布运行验收未执行，故对应项目保持未勾选；`baseline-backups/` 保持未跟踪且未改动。
> 波次 2 合并顺序审计（2026-08-31）：实际父链为 E→H→G→F→最终集成，不是计划要求的 E→F→G→H，故顺序项保持未勾选。
> 2026-09-01 收口：DataChannel 控制 + REST 降级、viewer 接管、配置变更踢 viewer 自动重连、watchdog 确死强拆（无脚本/脚本运行中两分支）、idle 拆会话与唤醒均已实测（docs/CLEAN_BASELINE_FUNC_EVIDENCE.md，含 G3 回归节）。Docker readiness/时区已实测（docs/UPDATE_DOCKER_E2E_EVIDENCE.md，API+浏览器双确认），WebRTC UDP 媒体面需容器内真机（USB 设备无法透传容器、未擅自改动设备网络配置），对应项保持未勾。10.3/10.5 的两个合并顺序审计项为历史过程记录（当时已注记实际父链），不作追溯改写。

### 10.6 最终清理与发布门禁

- [x] `cargo fmt --check` 通过。
- [x] `cargo clippy --all-targets --all-features -- -D warnings` 通过。
- [x] `cargo test` 通过。
- [x] `pnpm test:run` 通过。
- [x] `pnpm build` 通过。
- [x] 生产代码不存在 `scriptStatus`。
- [x] 生产代码不存在 `runScriptId` 或 `legacyRun`。
- [x] 生产代码不存在 `gb_token`。
- [x] 生产代码不存在 `LEGACY_TOP_KEYS`。
- [x] 生产代码不存在 `migrate_fs_layout`。
- [x] 生产代码不存在 `parse_legacy_sha256_hash`。
- [x] 生产代码不存在 `no_snapshot`。
- [x] 生产代码不存在脚本级 `/stop` 或 `/status` route。
- [x] 所有旧行为命中只存在于明确的 rejection 测试或历史文档中。
- [x] 所有破坏性提交均包含 `BREAKING CHANGE`。
- [x] 每个提交主题单一、可独立理解和回滚。
- [x] 最终文档、配置、fixture 和实际行为一致。（2026-09-01：本文件与 AUTO_UPDATE 计划 checklist 已按实测证据同步；web 580/server 453/launcher 184 测试、compose 5 变体 config、tools/verify-release.ps1（含严格在线 cargo audit）全绿；清理扫描生产代码 0 命中；实测发现的缺陷（时区显示、taken_over 持久提示、二次重连、drain 超时、长路径/跨盘）均已修复；剩余 GitHub Release/GHCR/生产签名/clean VM 属 AUTO_UPDATE 计划外部环境项，不在本基线清理范围）
- [x] 已记录最终基线 commit 和开发数据重建说明。
- [x] 性能实验未夹带进本轮兼容清理提交。

> 自动门禁与提交审计证据（2026-08-31）：生产代码扫描上述 8 个禁用标记及脚本级 stop/status route 均为 0 命中；旧行为命中仅在明确 rejection/负向测试中。`d22ff00`、`76ac7ee`、`48ea895`、`46e7369`、`1c1fef8`、`fd896de`、`6ee4b23`、`9b3e7be` 等实际契约破坏提交均含 `BREAKING CHANGE`；无 footer 的 `d7075de`（docs）与 `8e3c026`（fix）不按破坏性提交计入。
> 最终本机复核（2026-08-31）：Docker/Compose CLI 可用但 Docker Linux daemon 不可用；主 compose、主+local、主+USB 的 `config --quiet` 均通过。一次性 `GAMER_ADMIN_PASSWORD` 临时服务已完成 `/health/ready`、登录/session、system info、设备扫描、真实设备 connect（online mirror）和 shutdown 冒烟，未留下 gamer-server/8443/ADB/scrcpy 残留；`baseline-backups/` 仍未跟踪，`server/data` 无工作树改动。
> 最终验收边界（2026-08-31）：`5ea58f2` 已清理明文凭据、Login/MainLayout 固定版本与日志徽标，生产代码静态扫描为 0 命中，自动门禁与 compose config 通过；但应用内浏览器 `ERR_BLOCKED_BY_CLIENT` 阻断 WebRTC/DataChannel，Docker daemon 不可用，设备脚本、viewer 接管、watchdog/idle 生命周期和发布运行未执行。因此“最终文档、配置、fixture 和实际行为一致”仍保持未勾选，不能宣称 CLEAN_BASELINE 全部完成。
