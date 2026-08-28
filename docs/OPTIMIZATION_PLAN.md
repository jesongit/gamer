# GameBot 优化实施计划

> 状态：阶段0/1已完成；阶段2自动化路由安全验收已收口但仍有凭据迁移、真实设备和内存稳定性项目；阶段3主体完成但 viewer/pusher 断开一致性未收口；阶段4～6部分完成；阶段7未完成，且本轮 Rust 门禁未全绿（2026-08-28）
> 编制日期：2026-08-27  
> 适用范围：Rust 服务端、Vue 前端、部署配置、测试与运维文档  
> 本文只定义后续实施顺序和验收条件，不代表相关改动已经完成。

## 1. 目标

本轮优化不增加新的游戏自动化语法或业务功能，重点解决以下问题：

1. 建立稳定、可重复的测试与构建基线，降低 YAML 引擎、WebRTC 和设备生命周期改动的回归风险。
2. 为 HTTP API、WebSocket 信令和管理动作建立真实的服务端安全边界。
3. 统一手动脚本、定时任务和设备运行状态，避免同一设备被多个执行实例同时控制。
4. 改善日志、SQLite、文件导入导出和 Docker 部署的可靠性。
5. 在不牺牲截图实时性的前提下，降低 ffmpeg 启动、模板预处理和 NCC 匹配的重复开销。
6. 逐步拆分超大文件，形成可以独立测试和维护的模块边界。

## 2. 非目标

本计划暂不包含：

- 新增 YAML 动作、恢复旧 YAML 语法或改变现有脚本语义。
- 支持同一设备同时运行多个自动化脚本。
- 将 scrcpy、WebRTC 或模板匹配整体替换为其他技术栈。
- 在没有基准数据前直接引入常驻 PNG 解码管线。
- 多用户、复杂角色权限和公网 SaaS 化。
- 对现有页面进行视觉重设计。

## 3. 当前基线

2026-08-27 只读检查结果：

- `cargo test`：21 项通过，0 项失败。
- Rust 编译：25 条告警，主要为未使用变量、字段和方法。
- `cargo fmt --all -- --check`：未通过，当前存在大范围格式差异。
- 当前使用 `npm run build` 验证通过；后续统一迁移为 pnpm 命令。
- Console 产物约 340.57 KiB，gzip 后约 183.01 KiB。
- `node test-validate.mjs`：11 项失败；部分断言和模板 fixture 已落后于 2026-08-27 的新语义。
- 前端没有正式的 `test`、`lint`、`typecheck` 脚本。
- 仓库同时存在 `package-lock.json`、`pnpm-lock.yaml` 和 `pnpm-workspace.yaml`。
- `server/gamer-server.log` 已约 108 MiB，没有轮转和保留策略。
- `server/target` 已约 5.8 GiB，Docker 构建上下文没有 `.dockerignore`。
- `Console.vue` 约 4154 行，`engine.rs` 约 2546 行，`webrtc.rs` 约 1552 行，`api/mod.rs` 约 1122 行。
- 登录成功后前端只写本地标记；除登录外的 API 和 `/ws/device/:id` 没有服务端鉴权，且启用了 permissive CORS。
- 手动运行注册表按脚本 ID 互斥，调度器使用另一套运行注册表，没有统一的设备级执行仲裁。

本轮复核结果（2026-08-28）：沿用主线已有测试证据，Rust 最近一次全量回归在本轮实际执行中变为 154 passed、1 failed、1 ignored；失败项为 `device::frames::tests::request_snapshot_bounds_per_cache_decode_concurrency`（并发计数断言 2 != 1）。`cargo clippy --all-targets --all-features -- -D warnings` 亦在本轮执行中失败，主要由现存 dead_code、`too_many_arguments`、`manual_is_multiple_of` 等告警被门禁放大。前端 `npm test` 与 `npm run build` 通过；`docker compose config`、`tools/verify-release.ps1`、`cargo metadata --locked --no-deps` 通过；Docker daemon 可用，`docker build -t gamer .` 已完成。阶段2的真实 HTTP 路由、WS 鉴权/同源、Cookie 过期、敏感日志、ZIP 路径和命令注入边界已有自动化验收，真实设备 DataChannel 冒烟和资源限额后的持续内存观测仍待设备/运行环境。阶段4已落地 `/health/ready`、`/metrics`、原子写入、SQLite WAL/busy-timeout、独立 DB worker 与日志批处理；SQL 在独立线程执行，但调用侧仍同步等待 DB RPC，视频/GOP/ffmpeg/NCC 等指标虽已定义却未接入生产采集点。阶段5的 `request_snapshot` 已接入生产截图路径，精确同帧并发合并、错误恢复、不同帧和不同设备隔离 4 项测试通过；正式跨平台性能报告、完成后短窗缓存和计算池仍未完成。阶段6仅完成 Console geometry、engine syntax/events 和 WebRTC protocol 的局部拆分，全面模块化仍未完成。

### 3.1 本轮 checklist 验收对账

统计包含阶段 0～7 内所有 `- [ ]` / `- [x]`（含嵌套子项）；阶段 5 新增一条独立的 `InFlight` 合并器检查项，因此总项数由 236 增至 237。

| 阶段 | 审计前（HEAD） | 本轮审计后 | 未完成项—原因—下一步动作 |
|---|---:|---:|---|
| 0 | 36/36 | 36/36 | 无。 |
| 1 | 25/28 | 28/28 | 原漏勾的 `server/web-dist` 排除、敏感日志约束和 adb/ffmpeg readiness 均已由主线代码/测试证明。 |
| 2 | 0/36 | 33/36 | 开发模式仍兼容明文密码、缺真实设备 DataChannel 冒烟、资源限额测试未观测进程内存稳定性；下一步移除明文迁移口、跑设备链路与受限资源压力测试。 |
| 3 | 0/34 | 29/34 | RUN-005 的强制断开、原因建模及旧 pusher 回归未实现；下一步统一 disconnect reason/cleanup 并补 viewer/pusher 生命周期测试。 |
| 4 | 0/36 | 20/36 | 原子写覆盖/失败保持、DB RPC 异步化与 `get_task` 直查、定期保留/VACUUM、结构化 reason 和多组生产指标未收口；下一步按 DATA/OBS 子项逐一实现并测试。 |
| 5 | 0/30 | 8/31 | 缺 Windows+Docker/Linux 正式基准、完整模板缓存失效/LRU、受限计算池、50～100ms 完成结果缓存及 NCC 后续评估；下一步先补基准和指标，再决定优化。 |
| 6 | 0/19 | 2/19 | 仅局部 helper 拆分，缺 Console/API/engine/WebRTC 主体边界和浏览器冒烟；下一步按纯移动→测试→内部简化的小提交推进。 |
| 7 | 0/17 | 14/17 | 前端 test/build、`docker compose config`、`tools/verify-release.ps1`、`cargo metadata --locked --no-deps` 与本机 `docker build` 已有证据；Rust 全门禁未在本轮全绿，生产数据副本迁移回滚、带版本日期的依赖安全审计和真实设备矩阵仍无本轮证据。 |
| **总计** | **61/236** | **170/237** | **仍有 67 项 checklist 未完成，不宣称整体优化完成。** |

以上数字只用于确定起点。执行期间若环境变化，应在阶段 0 重新记录基线。

## 4. 实施原则

1. **先测试再重构**：先把当前正确行为固化为测试，再拆文件或改变内部结构。
2. **一阶段一主题**：安全、调度、性能、数据和模块拆分分别提交，保证可独立回滚。
3. **保持协议兼容**：需要改变接口时，先提供兼容层和迁移期，再删除旧接口。
4. **设备级串行**：默认一个设备只允许一个自动化执行实例；第一版冲突直接返回 409，不引入排队复杂度。
5. **以测量驱动性能优化**：先采集 p50/p95、进程启动次数、队列深度和内存，再决定实现。
6. **保护实时性**：截图缓存或请求合并必须携带帧序号与时间戳，不能重新引入陈旧帧问题。
7. **同步维护文档**：修改 YAML 引擎时必须同步检查前端校验、操作模板和 `docs/YAML.md`；新踩坑追加到 `docs/PITFALLS.md`。
8. **不混入用户数据改动**：优化提交不得顺带改写现有模板图片或业务 YAML 脚本，测试 fixture 除外。

## 5. 阶段总览

| 阶段 | 名称 | 主要产出 | 预计工作量 | 前置依赖 |
|---|---|---|---:|---|
| 0 | 基线与质量门禁 | 正式测试、CI、格式与告警基线 | 1～3 人日 | 无 |
| 1 | 构建与运维快速治理 | Docker 上下文、日志轮转、配置失败策略 | 1～2 人日 | 阶段 0 |
| 2 | 服务端鉴权与输入防护 | Cookie 会话、中间件、WS 鉴权、导入限额 | 2～4 人日 | 阶段 0 |
| 3 | 统一运行管理与调度幂等 | `RunManager`、设备级互斥、异步任务 API | 3～6 人日 | 阶段 0 |
| 4 | 数据、日志与可观测性 | 原子写入、DB 写入治理、health/metrics | 2～5 人日 | 阶段 1、3 |
| 5 | 模板匹配性能优化 | 基准、模板缓存、计算池、截图合并 | 3～6 人日 | 阶段 0、4 |
| 6 | 模块化重构 | 拆分 Console、API、engine、webrtc | 4～8 人日 | 阶段 0～5 |
| 7 | 发布验收与文档收口 | 回归矩阵、部署验证、README 更新 | 1～2 人日 | 全部阶段 |

预计总工作量为 17～36 人日。建议按阶段执行并发布，不把全部阶段合并成一个版本。

---

## 6. 阶段 0：建立基线与质量门禁

### 6.1 目标

把当前依赖人工验证的关键行为变成可重复执行的自动化检查，为后续安全、调度和性能改动提供保护。

### 6.2 任务

#### QG-001：统一使用 pnpm

- [x] 将 pnpm 确定为前端唯一包管理器。
- [x] 保留 `pnpm-lock.yaml` 和确有用途的 `pnpm-workspace.yaml`，删除 `package-lock.json`。
- [x] 使用 Corepack 固定项目使用的 pnpm 大版本，并在 `package.json` 增加 `packageManager` 字段。
- [x] 将 README、`gamer.ps1`、Dockerfile 和 CI 中的 npm 命令统一替换为 pnpm。
- [x] CI 使用 `pnpm install --frozen-lockfile`，禁止安装过程隐式改写锁文件。
- [x] 删除 `package-lock.json` 前，使用现有 `pnpm-lock.yaml` 做一次干净安装、测试和构建，确认依赖解析完整。

涉及文件：

- `web/package.json`
- `web/package-lock.json`（迁移完成后删除）
- `web/pnpm-lock.yaml`
- `web/pnpm-workspace.yaml`
- `README.md`
- `gamer.ps1`

验收标准：

- 全新目录执行 `pnpm install --frozen-lockfile` 和 `pnpm build` 可重复成功。
- 仓库只剩 `pnpm-lock.yaml` 这一套依赖锁文件。
- README、CI、Dockerfile 和 `gamer.ps1` 中不再出现用于前端安装/构建的 npm 命令。

#### QG-002：把脚本校验器和行映射移出 Console.vue

- [x] 新建独立模块，例如 `web/src/script-language/validate.js`。
- [x] 新建独立模块，例如 `web/src/script-language/line-map.js`。
- [x] `Console.vue` 通过导入调用，不再内嵌 `validateScriptCode` 和 `computeRunLineMap`。
- [x] 测试直接导入模块，不再从 `.vue` 文本中用花括号计数提取函数。
- [x] 把真实业务 YAML 复制为稳定 fixture，避免测试直接依赖会持续变化的 `server/data`。
- [x] fixture 使用虚拟模板清单，不依赖中文模板当前是否改名。

验收标准：

- 原有前端行为不变。
- `test-validate.mjs` 的有效场景全部迁移后不再需要字符串提取逻辑。
- 新增的测试在没有 Android 设备时也可运行。

#### QG-003：引入正式前端测试

- [x] 引入 Vitest。
- [x] 在 `package.json` 增加 `test` 和 `test:run`。
- [x] 覆盖以下场景：
  - [x] 顶层 `steps`、`func`、`config` 正常形式。
  - [x] 省略 `steps:` 的顶层序列。
  - [x] 省略 `func:` 的纯函数库映射。
  - [x] 列表和映射两种函数定义。
  - [x] 函数名行、函数体行、顶层步骤行的运行映射。
  - [x] `cond`、跨文件函数、`then/else`、`throw`、`loop` 递归校验。
  - [x] 旧语法的定向错误提示。
  - [x] 中文模板名、`#` 区域后缀和短名引用。
- [x] 将当前 11 项失败逐项分类为“测试过期”或“产品回归”，不能简单删除断言。

验收标准：

- `pnpm test:run` 为 0 失败。
- 至少有一组 fixture 同时被 Rust 与前端测试消费，或由同一份用例清单生成。

#### QG-004：Rust 质量基线

- [x] 执行一次独立的纯格式化提交，避免以后业务提交混入大面积格式差异。
- [x] 清理现有编译告警；确有保留价值的字段使用明确注释和局部 `allow`，不做全局屏蔽。
- [x] 增加 `cargo fmt --all -- --check`。
- [x] 增加 `cargo clippy --all-targets --all-features -- -D warnings`。
- [x] 保留 `cargo test`。
- [x] 检查 `[profile.release] debug-assertions = true` 的真实目的；`tracing` 日志等级不应依赖该选项。若没有其他断言需求则移除并做 release 冒烟测试。

验收标准：

- fmt、clippy、test 全部通过。
- `cargo test` 不少于当前 21 项有效测试。
- release 构建中的 debug/info/trace 行为由显式配置验证，而不是依赖注释推断。

#### QG-005：CI 工作流

- [x] 建立 GitHub Actions 或当前代码托管平台等价工作流。
- [x] Rust job：fmt → clippy → test → release check。
- [x] Web job：启用 Corepack → `pnpm install --frozen-lockfile` → `pnpm test:run` → `pnpm build`。
- [x] 增加缓存，但缓存键必须包含锁文件哈希。
- [x] CI 不读取真实 `server/config.toml`，使用临时配置或默认测试配置。
- [x] CI 不需要 adb、ffmpeg 或 Android 设备即可完成单元测试。

验收标准：

- 新 PR/提交可以明确看到所有质量门禁结果。
- 任一测试失败会阻止合并。

### 6.3 建议提交拆分

1. `style(server): 统一 Rust 代码格式`
2. `test(web): 固化脚本校验与行映射用例`
3. `refactor(web): 抽离脚本语言校验模块`
4. `chore(build): 统一前端包管理器与质量脚本`
5. `ci: 增加前后端构建测试门禁`

### 6.4 回滚

- 模块抽离必须先保持原导出签名；若 Console 行为异常，可单独回滚抽离提交而保留测试。
- 格式化必须单独提交，回滚业务改动时不与格式变更纠缠。

---

## 7. 阶段 1：构建与运维快速治理

### 7.1 Docker 构建

#### OPS-001：缩小构建上下文

- [x] 增加 `.dockerignore`，至少排除：
  - [x] `server/target/`
  - [x] `server/data/`
  - [x] `server/*.log`（由 `**/*.log` 通配覆盖）
  - [x] `server/web-dist/`（`1f7abe7` 已加入 `.dockerignore`）
  - [x] `web/node_modules/`
  - [x] `.git/`
  - [x] `.chrome-test-profile/`
  - [x] `.zcode/`
- [x] 决定 Docker build context：
  - 推荐：仓库根目录作为 context，由多阶段 Dockerfile 同时构建 web 和 server。
  - 备选：分别构建前端产物和服务端镜像，但必须由脚本显式串联。
- [x] 使用依赖缓存层或 `cargo-chef`，避免每次源码变化重新编译全部依赖。

验收标准：

- Docker 发送的构建上下文不包含本机约 5.8 GiB 的 `server/target`。
- 在干净环境中无需预先生成 `web/dist` 即可得到可访问的镜像。
- 构建日志可看出 Rust 依赖层能够命中缓存。

#### OPS-002：收口 Compose 语义

- [x] 修正 README 中 profile 说明：默认 `docker compose up` 不启动带 profile 的 redroid；使用 `--profile redroid` 才启动它。
- [x] 明确运行数据目录到底使用仓库根 `data/` 还是 `server/data/`。
- [x] 区分“随镜像发布的初始数据”和“运行期持久数据”，避免 volume 覆盖镜像内种子数据。
- [x] gamer 服务默认不使用 `privileged: true`；仅 USB 直通场景通过 override/profile 开启所需设备和权限。
- [x] redroid 保留其必要的 privileged 配置。

验收标准：

- 默认网络 adb/redroid 部署不需要给 gamer 容器完整宿主机权限。
- README 命令与实际启动服务集合一致。

### 7.2 日志轮转

#### OPS-003：服务文件日志轮转

- [x] 使用按天或按大小轮转的 writer。（tracing-appender daily，产出 `<名>.YYYY-MM-DD`）
- [x] 文件写入使用非阻塞日志 worker，避免 Tokio 工作线程直接等待磁盘。
- [x] 配置保留天数或最大文件数，默认建议保留 7～14 天。（config.toml `log_retain_days` 默认 14）
- [x] 保留 stdout 模式，容器环境优先交给容器日志驱动处理。（GB_LOG 留空即纯 stdout）
- [x] 明确敏感字段不得写入日志：密码、Cookie、完整 Authorization、导入文件内容。（`4cca76b` 的非敏感配置摘要、`b7ab9dd` 的真实 tracing 捕获及运行日志统一脱敏共同覆盖）

验收标准：

- 连续运行不会再生成单个无限增长的 `gamer-server.log`。
- 日志轮转过程中服务和 WebRTC 会话不受影响。

### 7.3 配置可靠性

#### OPS-004：配置失败即退出

- [x] 配置文件存在但解析失败时直接返回错误并终止启动，不再静默使用默认值。
- [x] 配置文件不存在时区分开发与生产：（GAMER_PROFILE：dev 警告放行 / prod 报错退出）
  - 开发模式可允许默认配置，并打印明确警告。
  - 生产模式必须要求显式配置。
- [x] 启动时校验端口、时长、阈值、码率、fps、路径和日志等级。
- [x] 启动时检查 scrcpy jar 是否存在。
- [x] ffmpeg/adb 可执行性放入 readiness 检查；是否阻止启动由部署模式决定。（`/health/ready` 已报告两者探测结果，启动期仍只告警不阻断）
- [x] 将本机绝对路径从可分发配置中移除；`config.example.toml` 保持跨平台。
- [x] `server/config.toml` 改为本机文件并加入忽略，提交前评估历史中是否包含需要轮换的真实秘密。

验收标准：

- 故意写错 TOML 时服务明确失败，而不是以另一套默认参数运行。
- 日志显示最终生效的非敏感配置来源和路径。

---

## 8. 阶段 2：服务端鉴权与输入防护

### 8.1 决策

推荐采用“单管理员、同源 Cookie 会话”：

- 登录成功后生成高熵随机会话 ID。
- Cookie 设置 `HttpOnly`、`SameSite=Strict`；HTTPS 环境设置 `Secure`。
- 服务端保存会话及过期时间，重启后要求重新登录即可。
- 开发环境是否允许非 Secure Cookie 由显式 `dev_mode` 控制。
- 前端不再把伪 token 写入 localStorage。

不建议第一版引入 JWT：当前是单管理员、本地服务，JWT 增加密钥轮换和注销复杂度，没有明显收益。

### 8.2 任务

#### SEC-001：鉴权中间件

- [x] 静态资源、登录和 `/health/live` 可匿名访问。
- [x] 其他 `/api/**` 全部经过会话鉴权。
- [x] `/ws/device/:id` 在升级前完成鉴权。
- [x] `/api/shutdown`、设备控制、脚本运行/停止、模板删除、ZIP 导入列为高风险接口，增加单独测试。
- [x] 未认证统一返回 401；已认证但被策略禁止返回 403。

#### SEC-002：登录与退出

- [x] 登录成功设置服务端会话 Cookie。
- [x] 增加退出接口并使当前会话立即失效。
- [x] 增加会话绝对有效期和空闲有效期。
- [x] 登录失败增加简单速率限制，例如按来源 IP 的滑动窗口。
- [x] 使用常量时间比较或密码哈希校验。
- [ ] 管理密码优先从环境变量/密钥文件注入；配置中如保存哈希，只保存强哈希而非明文。

当前进度：Argon2id、会话生命周期、IP+用户名限流和真实 HTTP/WS 路由安全验收均已通过测试；生产配置会在缺少环境密码或强哈希时 fail closed。未完成项是开发模式仍保留旧明文兼容，下一步移除明文迁移口并更新示例配置；真实 DataChannel 端到端仍归验收矩阵单独验证。

#### SEC-003：同源与 CSRF 防护

- [x] 移除 `CorsLayer::permissive()`。
- [x] 生产环境默认同源，不额外开放 CORS。
- [x] 开发模式由 Vite proxy 保持同源体验。
- [x] 对状态变更请求验证 `Origin`/`Host`。
- [x] Cookie 使用 `SameSite=Strict`；若以后必须跨站部署，再单独设计 CSRF token。
- [x] WebSocket 验证 Origin。

#### SEC-004：输入和资源限额

- [x] 为不同路由设置不同 body limit，而不是所有接口统一 20 MiB。
- [x] ZIP 导入初始建议限制：
  - [x] 压缩包 ≤ 20 MiB。
  - [x] 解压后总量 ≤ 100 MiB。
  - [x] 文件数 ≤ 500。
  - [x] 单 YAML ≤ 1 MiB。
  - [x] 单模板 ≤ 10 MiB。
- [x] 使用 ZIP entry 声明大小做预检查，并在实际读取时再次计数，防止声明不可信。
- [x] 图片解码前检查字节数，解码后检查像素总量，防止超大尺寸图片消耗内存。
- [x] 日志查询 `limit` 限制在合理范围，例如 1～1000。
- [x] 控制命令校验坐标、时长、文本长度和应用包名，不再用大量 `unwrap_or(0)` 静默接受缺失字段。

### 8.3 验收矩阵

- [x] 未登录访问设备列表返回 401。
- [x] 未登录调用 shutdown 返回 401，服务保持运行。
- [x] 未登录建立 WebSocket 失败。
- [ ] 登录后 REST、WebSocket、DataChannel 正常工作。
- [x] 登出后旧 Cookie 立即失效。
- [x] 跨 Origin 状态变更请求被拒绝。
- [ ] 超限 ZIP、ZIP slip、重复文件、超大图片均返回 4xx，进程内存不持续增长。
- [x] 浏览器刷新后会话行为符合设计，不依赖 localStorage 伪 token。

### 8.4 兼容与部署

- 此阶段属于行为变更。发布说明必须提示现有页面重新登录。
- 如果现有脚本或 `gamer.ps1` 调用 shutdown，需要为本机管理脚本设计受限凭据或仅回环地址可用的管理通道，不能继续裸奔。
- LAN 之外的访问必须通过 HTTPS 反向代理；不在应用内自行实现证书签发。

---

## 9. 阶段 3：统一运行管理与调度幂等

当前状态：主体完成（2026-08-28）。RunManager、设备级互斥、`run_id` 异步运行 API、FinishGuard 收尾、调度幂等、优雅停机 drain 和前端刷新恢复均已有代码与回归证据；RUN-005 的强制断开 viewer/pusher 一致性仍未收口，因此本阶段不标记为全部完成。

### 9.1 当前问题

- 手动脚本注册表键为 `script_id`，只能阻止同一脚本重复运行，不能阻止不同脚本控制同一设备。
- Scheduler 有独立的 task 运行表，不与手动运行共享。
- 不同定时任务可以同时命中同一设备。
- 手动“立即运行任务”会等待整个任务完成后才返回 HTTP。
- 调度器只在内存记录最近触发点；进程重启后可能重新拾取过去一小时内的最近触发点。
- 页面状态围绕 `script_id`，无法准确区分同一脚本的不同执行实例。

### 9.2 目标模型

新增统一 `RunManager`，以 `run_id` 表示一次执行：

```text
RunRecord
├─ run_id: UUID
├─ device_id
├─ script_id
├─ source: manual | scheduled | task_now
├─ task_id: optional
├─ scheduled_at: optional
├─ state: starting | running | stopping | success | failed | cancelled
├─ started_at / finished_at
├─ cancellation token
└─ result/error summary
```

第一版并发策略：

- 一个设备最多一个 active run。
- 冲突时立即返回 409，并返回当前 `run_id`、脚本和来源。
- 暂不排队；排队会引入过期任务、取消和优先级语义，后续有明确需求再做。

### 9.3 任务

#### RUN-001：实现 RunManager

- [x] 统一接管手动脚本和 Scheduler 的运行状态。
- [x] 使用 RAII guard 保证所有退出路径都执行 `devices.run_end` 并释放设备占用。
- [x] panic、任务取消、连接失败、引擎报错和正常完成都必须进入终态。
- [x] 服务关闭时先停止接收新任务，再取消/等待活动任务。（`RunManager::begin_shutdown` 关闸、等待及超时取消，并有 `shutdown_drains_rejects_new_and_force_cancels_on_timeout` 测试）
- [x] 停止动作按 `run_id` 定位，不按脚本名猜测。

#### RUN-002：调整 API

- [x] 启动脚本立即返回 `202 Accepted` 和 `run_id`。
- [x] 立即运行定时任务同样立即返回，不占用 HTTP 连接。
- [x] 增加统一运行查询，例如 `GET /api/runs/:run_id`。
- [x] 增加设备当前运行查询，例如 `GET /api/devices/:id/run` 返回完整 RunRecord 摘要。
- [x] 增加按 `run_id` 停止接口。
- [x] 旧的 script status/stop 接口保留一个迁移版本，内部转到 RunManager，并在前端迁移后删除。

#### RUN-003：前端状态迁移

- [x] store 以 `runId` 为主键，不再以 `scriptId` 充当执行实例 ID。
- [x] 页面刷新后按设备恢复当前 run。
- [x] 冲突时展示正在运行的脚本、来源和开始时间。
- [x] 定时任务立即运行按钮收到 202 后立即恢复，不等待脚本结束。
- [x] 停止按钮只停止当前 run。

#### RUN-004：调度幂等

- [x] 新增运行或触发表，持久化 `(task_id, scheduled_at)`。
- [x] 建立唯一约束，保证同一计划触发点至多创建一次运行。
- [x] 明确 misfire 策略：
  - 推荐默认：服务恢复后只补最近一次、且延迟不超过配置窗口的触发。
  - 过期更久的触发记录为 skipped，不批量补跑。
- [x] `last_run_at` 使用可排序的 UTC 时间格式或整数时间戳，不再依赖本地格式字符串。
- [x] 系统时区/DST 变化需要测试。

#### RUN-005：修复断开与 viewer 状态一致性

- [ ] REST 强制 disconnect 时同步关闭并移除该设备 viewer。
- [ ] 所有拆会话路径明确是否：关闭 viewer、发送通知、允许自动重连。
- [ ] 将这些差异建模为枚举原因，而不是多个布尔参数。
- [ ] 增加“旧 pusher 不再收到新补帧”的回归测试。

### 9.4 必测场景

- [x] 同一设备手动运行两个不同脚本，第二个返回 409。
- [x] 手动脚本运行时定时任务命中，定时任务按策略记录冲突/跳过，不注入控制。
- [x] 两个定时任务同秒命中同一设备，只有一个取得执行权。
- [x] 两台设备可以并行运行。
- [x] 启动阶段连接失败后设备锁被释放。
- [x] 运行中请求停止，最终状态为 cancelled，run count 归零。
- [x] 服务重启不会重复执行已经持久化的 scheduled_at。
- [x] task-now API 在任务完成前已经返回 202。
- [ ] 强制 disconnect 后旧 viewer/pusher 全部退出。

---

## 10. 阶段 4：数据、日志与可观测性

当前状态：部分完成（2026-08-28）。健康检查、原子写入、事务化导入、SQLite WAL/busy-timeout、独立 DB worker、日志批处理和部分低基数指标已落地并有测试；rusqlite 已移出 Tokio 核心线程，但同步 DB RPC 仍可能阻塞异步 handler。运行日志只在启动时触发清理，结构化 reason 和视频/GOP/ffmpeg/NCC 指标生产端尚未完整接线；下一步是异步化调用侧、补周期保留任务并接通生产指标。

### 10.1 文件原子写入

#### DATA-001：统一 atomic write

- [x] 新建文件写入工具：同目录临时文件 → 写入 → flush/sync → rename/replace。
- [ ] 脚本保存、模板上传和配置生成使用统一工具。
- [x] Windows 下验证替换已有文件的行为，避免 rename 语义差异。
- [ ] 写入失败时旧文件保持完整。

#### DATA-002：分区导入事务化

- [x] `confirm=false` 只做解析和冲突报告。
- [x] `confirm=true` 先解压到同文件系统 staging 目录。
- [x] 完成文件类型、数量、大小和 YAML 编码校验后再提交。
- [x] 覆盖文件前建立可恢复备份或以目录交换实现提交。
- [x] 任一文件失败时不留下半导入状态。
- [x] 导入完成后清理 staging；服务启动时清理过期 staging。

### 10.2 SQLite

#### DATA-003：数据库写入治理

- [x] 配置 `busy_timeout`、WAL 和必要的 foreign key。
- [x] 不在 Tokio 核心线程直接执行可能较慢的 rusqlite 操作。（SQL 已集中到 `gamer-db-worker` 独立线程；调用侧同步等待仍是未完成的延迟风险）
- [x] 方案优先级（已采用单独 DB worker/actor；调用侧同步等待仍待异步化）：
  1. 单独 DB worker/actor，异步通道提交操作。
  2. 低频查询使用 `spawn_blocking`。
  3. 不建议只把当前 Mutex 换成异步 Mutex，因为同步 SQLite 调用仍会阻塞 executor。
- [x] 日志写入按小批量事务提交，例如 100 条或 250ms 一批；异常退出允许损失极少量 debug 日志，但 success/error 终态必须可靠落盘。
- [ ] `get_device`/`get_task` 使用直接 SQL，不再先 list 全表再内存查找。

#### DATA-004：数据保留

- [x] 为运行日志增加最大保留天数或最大条数。
- [ ] 定期分批删除，避免一次大事务。
- [ ] 暂不每次清理后自动 VACUUM；根据数据库大小提供手动维护动作。
- [ ] 清理动作记录删除范围和数量。

### 10.3 可观测性

#### OBS-001：健康检查

- [x] `/health/live`：进程事件循环可响应即可。
- [x] `/health/ready`：检查数据目录、SQLite、scrcpy jar，并报告 adb/ffmpeg 探测结果。
- [x] readiness 返回结构化 JSON，不泄露密码和主机敏感路径。
- [x] Docker healthcheck 使用 readiness 或轻量专用端点。（`1f7abe7` 的 Compose healthcheck 调用 `/health/ready`）

#### OBS-002：结构化关联字段

- [ ] 全链路使用 `device_id`、`session_generation`、`viewer_id`、`run_id`、`task_id`。
- [ ] 连接、重连、踢 viewer、拆会话必须记录标准 reason 枚举。
- [ ] 避免只靠自由文本推断状态迁移。

#### OBS-003：指标

- [ ] 初始至少暴露：
  - [x] 当前设备/会话/viewer/run 数。
  - [ ] scrcpy 连接成功/失败/重连次数及原因。
  - [ ] 视频输入帧率、RTP 发送帧率、队列深度和丢帧数。
  - [ ] GOP 帧数和字节数。
  - [ ] ffmpeg 解码次数、耗时、超时和失败次数。
  - [ ] NCC 匹配次数、耗时、命中率、区域/全屏分类。
  - [ ] Scheduler 触发延迟、冲突、跳过和失败次数。
  - [x] DB 写入队列深度和批处理耗时。
- [x] 指标标签不得包含模板完整路径、日志消息等高基数字段。

验收标准：

- 一次“连接慢/截图慢/任务没跑”的问题可以从指标和结构化日志判断卡在哪一层。
- 关闭指标采集时业务行为不变。

---

## 11. 阶段 5：模板匹配性能优化

当前状态：部分完成（2026-08-28）。matcher 内容哈希模板缓存已完成；截图请求合并器已接入生产 `decode_latest_png`，以 config/GOP generation + frame sequence 精确区分帧，并通过同帧合并、失败恢复、不同帧并行和设备隔离测试。正式跨平台性能报告、完成后 50～100ms 短窗缓存、可观测 decode 指标和受限计算池仍未完成。

### 11.1 性能基准先行

#### PERF-001：建立可重复基准

- [x] 准备固定 H.264 GOP、截图和模板 fixture，不使用实时设备作为唯一基准。
- [ ] 分别记录：
  - [ ] `decode_latest_png` 总耗时。
  - [ ] ffmpeg 启动、输入写入、解码和 PNG 输出耗时。
  - [ ] PNG 解码与灰度化耗时。
  - [ ] 全屏和区域 NCC 耗时。
  - [ ] 模板文件读取和预处理耗时。
  - [ ] 单次 `find` 主模板 + N 个 block 的整轮耗时。
- [ ] 记录 p50、p95、最大值、CPU 和峰值内存。
- [ ] Windows 和 Docker/Linux 至少各跑一轮。

验收标准：

- 后续每个 perf 提交都附带相同 fixture 的前后数据。
- 不使用 README 中现有 `<50ms` 描述作为验收依据。

### 11.2 模板缓存

#### PERF-002：缓存模板预处理结果

- [ ] 缓存键包含：规范化路径、mtime、文件大小，必要时加内容哈希。
- [x] 缓存内容包括：灰度图、尺寸、缩放版本、均值、方差和 NCC 所需数组。
- [ ] 模板上传、覆盖、重命名和删除后主动失效；mtime 检查作为兜底。
- [ ] 缓存设置总内存上限和 LRU 淘汰。
- [ ] 短名解析结果也可按目录代数缓存，目录变化后失效。

验收标准：

- 未改变模板时，连续匹配不再重复读文件、解 PNG、灰度化和计算模板统计。
- 覆盖同名模板后下一次匹配必定使用新内容。
- 相同 fixture 的匹配坐标和分数误差保持在约定范围内。

### 11.3 计算池

#### PERF-003：隔离 CPU 密集任务

- [ ] NCC、PNG 解码和大图缩放不占用 Tokio 核心工作线程。
- [ ] 使用专用 `spawn_blocking`/计算池，并设置并发上限。
- [ ] 避免 Tokio blocking pool 与 Rayon 双层无界扩张。
- [ ] 多设备并发时提供背压，而不是无限排队。

验收标准：

- 高负载匹配期间 API 健康检查和控制消息仍能及时响应。
- 计算并发上限可配置或根据 CPU 合理计算。

### 11.4 截图解码合并

#### PERF-004：按帧序号合并短时间请求

- [x] 提供独立可复用的 `InFlight<K, T, E>` 请求合并器，并通过并发共享、错误广播、取消清理和重试测试（`570ba85`、`505bc5d`）。
- [x] `FrameCache` 暴露单调递增的帧序号和最近帧到达时间。
- [x] 同一设备、同一帧序号上的并发截图请求共享一个 in-flight decode future（生产路径接入提交 `04361e7`）。
- [ ] 已解码结果只允许在很短新鲜度窗口内复用，初始建议 50～100ms，并以指标调整。
- [x] 新帧到达后不得长期返回旧 PNG。
- [x] 分辨率/config 代数变化立即失效缓存。
- [x] `find` 主模板和 block 是否共享同一截图必须保持现有语义；第一版只合并真正并发或同帧请求，不主动改变“每模板独立取新截图”。

验收标准：

- 并发截图不会为同一帧启动多个 ffmpeg。
- 返回结果包含内部可观测的 frame sequence/age。
- 分辨率切换、GOP 清空和解码重试测试全部通过。

### 11.5 NCC 算法优化候选

仅在模板缓存和计算池完成、指标仍显示 NCC 为主要瓶颈时执行：

- [ ] 使用积分图加速滑窗均值和方差。
- [ ] 检查 x/y 遍历顺序的缓存局部性。
- [ ] 评估灰度 `u8`/`f32` 转换复用和 SIMD。
- [ ] 评估 coarse-to-fine 搜索，但必须验证小模板和相似 UI 的误判率。
- [ ] 是否引入 OpenCV/其他原生依赖单独决策，考虑镜像体积和跨平台构建成本。

停止条件：若 p95 已满足真实脚本轮询需求，且 ffmpeg 才是主要耗时，则不继续复杂化 NCC。

### 11.6 禁止项

- 未建立 generation、freshness 和健康检查前，不恢复旧式常驻 PNG 流。
- 不为了跑分取消模板区域精确匹配。
- 不通过降低阈值掩盖算法误差。
- 不在同一个提交里同时改匹配算法和脚本点击语义。

---

## 12. 阶段 6：模块化重构

当前状态：部分完成（2026-08-28）。Console geometry、engine syntax/events 和 WebRTC Annex-B/SDP protocol helper 已局部拆分并通过回归；Console 状态/UI、API 资源、engine 执行层和 WebRTC pusher/viewer 的全面拆分仍未完成。

此阶段只在前述测试和运行管理稳定后执行。每次先“原样移动”，后“内部简化”，禁止边拆文件边改变协议。

### 12.1 前端 Console 拆分

建议目标结构：

```text
web/src/
├─ components/console/
│  ├─ DevicePanel.vue
│  ├─ VideoStage.vue
│  ├─ TemplateCapture.vue
│  ├─ ScriptRunner.vue
│  └─ RunLogPanel.vue
├─ composables/
│  ├─ useWebRtcViewer.js
│  ├─ useDeviceConnection.js
│  ├─ useTemplateCapture.js
│  ├─ useScriptRun.js
│  └─ useRunLogs.js
└─ script-language/
   ├─ validate.js
   ├─ line-map.js
   └─ fixtures/
```

拆分顺序：

- [ ] 先抽纯函数：坐标换算、指纹、YAML 校验、行映射、模板名解析。
- [ ] 再抽无 UI 的状态 composable：运行状态、日志轮询、设备加载。
- [ ] 再抽 WebRTC 生命周期，保持唯一 cleanup 入口。
- [ ] 最后拆视觉组件和模板。
- [ ] 每一步执行前端单测和浏览器连接冒烟测试。

目标不是追求任意行数，但 `Console.vue` 最终应主要负责页面编排，不再同时实现信令、视频看门狗、设备表单、模板裁切和 YAML 解释。

### 12.2 API 拆分

建议按资源拆分：

```text
server/src/api/
├─ mod.rs
├─ auth.rs
├─ devices.rs
├─ templates.rs
├─ scripts.rs
├─ runs.rs
├─ tasks.rs
├─ logs.rs
└─ ws.rs
```

- [ ] `mod.rs` 只负责状态组装、Router 和共享错误类型。
- [ ] 建立统一 `ApiError`，避免每个 handler 手工拼 Response。
- [ ] 统一输入校验和 4xx/5xx 映射。
- [ ] 阻塞文件/DB 工作不得散落在 handler 内。

### 12.3 Engine 拆分

建议边界：

```text
server/src/engine/
├─ mod.rs
├─ model.rs
├─ normalize.rs
├─ parse.rs
├─ validate.rs
├─ substitute.rs
├─ execute.rs
├─ match_ops.rs
└─ tests/
```

- [ ] `normalize/parse/validate` 尽量保持纯函数。
- [x] `$N`、`^N` 替换独立测试。
- [ ] 执行上下文和函数栈集中管理。
- [ ] 模板匹配、设备控制通过窄 trait 注入，单元测试使用 fake。
- [ ] 跨文件函数解析与文件系统寻址分离。
- [x] 重构不改变 YAML 语义；如必须改变，另起破坏性提交并同步文档。

### 12.4 WebRTC 拆分

建议边界：

```text
server/src/webrtc/
├─ mod.rs
├─ viewer.rs
├─ signaling.rs
├─ pusher.rs
├─ rtp_h264.rs
├─ rtp_audio.rs
├─ control.rs
└─ probe.rs
```

- [ ] RTP H.264 packetization 使用录制 fixture 测试 SPS/PPS、IDR、FU-A、marker 和时间戳。
- [ ] pusher 队列、waiting_key、初始 GOP 重放建模为可测试状态机。
- [ ] viewer 接管/conflict/taken_over 与 RTP 推送解耦。
- [ ] 诊断 probe 与生产推流隔离，确保关闭时零开销。

### 12.5 设备 actor 候选

如果阶段 3 后仍存在大量交错锁和状态竞争，再考虑每设备 actor：

- 一个 actor 串行处理 connect/disconnect/reset/viewer/run/activity。
- 外部通过命令通道交互。
- actor 持有 session generation，迟到任务必须校验 generation。

这是较大架构变更，不作为阶段 3 的前置条件。只有在测试和指标证明当前共享状态仍难以维护时才执行。

---

## 13. 阶段 7：发布验收与文档收口

当前状态：未完成（2026-08-28）。安全自动化矩阵和本轮三份文档已对账；Rust 全门禁沿用既有结果而未重跑，真实设备矩阵、正式性能表、干净 Docker 构建、生产数据副本迁移回滚及依赖安全审计仍无本轮证据。下一步按 §13.1～13.4 在相应环境逐项执行并回填日期、版本和实测值。

### 13.1 自动化验收

- [ ] Rust fmt、clippy、test 全通过。当前 `cargo test` 仍有 1 条并发断言失败，`cargo clippy` 仍受现存 dead_code / lint 阻断。
- [x] 前端 test、build 全通过。
- [x] Docker 镜像构建成功；但这次是在当前可用 Docker daemon 上完成，未证明“完全干净无缓存环境”。
- [ ] 数据迁移在生产数据副本上成功，并可回滚。
- [ ] 依赖安全审计无未处置的高危项；审计结果记录版本和日期。

### 13.2 设备回归矩阵

至少覆盖当前可用的设备类型：

| 场景 | 镜像屏 | 虚拟屏 | USB | WiFi/emu |
|---|---:|---:|---:|---:|
| 首次连接与初始出画 | 必测 | 必测 | 必测 | 可用则测 |
| 页面接管 viewer | 必测 | 必测 | 必测 | 可用则测 |
| 静态画面 10 分钟 | 必测 | 必测 | 必测 | 可用则测 |
| 分辨率/方向变化 | 必测 | 必测 | 必测 | 可用则测 |
| 脚本运行与停止 | 必测 | 必测 | 必测 | 可用则测 |
| 定时任务与重启幂等 | 模式无关 | 模式无关 | 必测 | 可用则测 |
| 空闲低功耗与恢复 | 必测 | 必测 | 必测 | 可用则测 |
| 优雅停机与重启 | 必测 | 必测 | 必测 | 可用则测 |

### 13.3 安全验收

- [x] 匿名访问所有受保护 API 均失败。
- [x] shutdown 和控制接口不能被跨站页面调用。
- [x] Cookie/Authorization 不进入日志。
- [x] 大包、ZIP bomb、恶意文件名和超大图片均被限制。
- [x] gamer 容器在非 USB 场景不使用 privileged。

### 13.4 性能验收

目标值应以阶段 5 的基线为基础填写，不预先承诺脱离环境的绝对延迟：

| 指标 | 基线 | 目标 | 实测 |
|---|---:|---:|---:|
| 单次按需解码 p50/p95 | 待测 | 不劣化实时性 | 待填 |
| 区域 NCC p50/p95 | 待测 | 较基线下降 | 待填 |
| 全屏 NCC p50/p95 | 待测 | 较基线下降 | 待填 |
| 每分钟 ffmpeg 启动次数 | 待测 | 同帧请求显著合并 | 待填 |
| 高负载 health p95 | 待测 | 保持可响应 | 待填 |
| 服务日志日增长量 | 待测 | 受保留策略约束 | 待填 |
| Docker 构建上下文 | 当前含约 5.8 GiB target | 不含本地产物 | 待填 |

### 13.5 文档更新

- [x] README 删除“连接自动启动应用”等过期说明。
- [x] README 的帧缓存描述改为“GOP 帧环 + 按需 ffmpeg 解码”。
- [x] README 的 Docker profile 和前端构建步骤与实际一致。
- [x] 补充鉴权、Cookie、HTTPS 反向代理和管理脚本说明。
- [x] 补充 RunManager、run_id 和冲突行为。
- [x] 新的环境/部署坑追加到 `docs/PITFALLS.md`。
- [x] 若 YAML 语义未变，不改写 `docs/YAML.md`；若有变化则同步引擎、前端校验、模板和文档。

---

## 14. 决策门

开始相关阶段前，需要明确以下选择：

### D1：部署边界

- 推荐：应用保持同源，LAN 内访问；公网入口由 HTTPS 反向代理承担。
- 如果永远只允许 localhost，可进一步限制监听地址，但不能继续把“前端登录页”当作鉴权。

### D2：运行冲突策略

- 推荐：第一版直接 409 拒绝。
- 暂不推荐：自动排队。需要额外定义过期、优先级、取消、重启恢复和队列上限。

### D3：前端包管理器

- 已决定：统一使用 pnpm。
- 执行 QG-001 时同步修改 CI、README、Dockerfile 和 `gamer.ps1`，删除 `package-lock.json`，并用 Corepack 固定 pnpm 大版本。

### D4：截图优化边界

- 推荐：模板预处理缓存 + 同帧 in-flight 合并。
- 暂不推荐：直接恢复常驻 ffmpeg PNG 流。

### D5：指标暴露方式

- 推荐：本地 `/metrics` + 结构化 `/health/ready`，不强制绑定外部监控系统。
- 如果以后接 Prometheus/Grafana，再增加部署配置。

---

## 15. 风险清单

| 风险 | 触发阶段 | 缓解措施 |
|---|---|---|
| 鉴权导致 `gamer.ps1` 无法调用 shutdown | 2 | 先设计本机管理凭据/回环管理通道，再保护接口 |
| Cookie 在纯 HTTP LAN 环境无法设置 Secure | 2 | 明确 dev/LAN 模式；生产公网必须 HTTPS |
| RunManager 改接口后页面无法恢复运行态 | 3 | 保留旧接口兼容期，前端先消费 run_id 再删除旧逻辑 |
| 调度幂等迁移导致任务漏跑或重复跑 | 3 | 数据副本演练、唯一键、明确 misfire 策略和重启测试 |
| 原子替换在 Windows 语义不同 | 4 | 独立文件工具测试，不直接假设 Unix rename 行为 |
| DB 批量日志导致异常退出丢少量日志 | 4 | success/error 强制刷新；仅允许丢低级诊断日志 |
| 模板缓存没有及时失效 | 5 | mtime/size/hash + 主动 invalidation + 覆盖上传测试 |
| 截图复用重新引入陈旧帧 | 5 | frame sequence、generation、最大 age、分辨率切换测试 |
| 计算池配置不当导致 CPU 过量并发 | 5 | 有界队列、并发上限、指标与压力测试 |
| 大文件拆分时生命周期 cleanup 丢失 | 6 | 原样移动、每步测试、连接/卸载冒烟、禁止同提交改行为 |

---

## 16. 后续执行记录模板

每开始一个阶段，在本节追加记录：

```markdown
### 阶段 N：名称

- 开始日期：
- 完成日期：
- 执行分支：codex/<branch-name>
- 基线提交：
- 完成提交：
- 已完成任务：
- 未完成任务及原因：
- 测试结果：
- 性能前后对比：
- 新增 PITFALLS：无 / 链接
- 发布/回滚说明：
```

### 阶段 0：建立基线与质量门禁

- 开始日期：2026-08-27
- 完成日期：2026-08-27
- 执行分支：main
- 基线提交：a67ebfe
- 完成提交：a7bbd49（纯格式化）/ 638b76e（清理编译告警）/ fb0786b（抽离脚本校验与行映射模块）/ c0e2e5e（修复校验器解析失败越界崩溃，缺陷修复①）/ 8558f4b（修复校验器对齐引擎多函数映射简写语义，缺陷修复②）/ 0a2ce3c（引入 Vitest 清零遗留失败）/ d5fe6e2（统一 pnpm + Corepack 固定）/ d2bef78（clippy -D warnings 门禁通过）/ d55e562（移除 release debug-assertions 依赖）/ 6354b85（CI 前后端门禁工作流）；d901121 为验收日补强（Rust 测试消费前端共享 YAML 以例清单，补齐 QG-003「同一 fixture 双侧消费」验收）
- 已完成任务：QG-001～QG-005 全部子项（见 §6 复选框）
- 未完成任务及原因：无
- 测试结果：cargo test 34 通过（基线 21 → 持续递增全绿）；cargo fmt --check、cargo clippy -D warnings、release 构建通过；vitest 115 通过；全新克隆目录 corepack + `pnpm install --frozen-lockfile` + `pnpm test:run` + `pnpm build` 全链路复现成功（QG-001/003「全新目录可重复」验收标准达成）；tools/ci-local.ps1 于 6354b85 后全绿
- 性能前后对比：不适用（质量基线建设）
- 新增 PITFALLS：无
- 发布/回滚说明：格式化与模块抽离各自独立成提交，可按 §6.4 单独回滚

### 阶段 1：构建与运维快速治理

- 开始日期：2026-08-27
- 完成日期：2026-08-27
- 执行分支：main
- 基线提交：6354b85（阶段 0 收尾）
- 完成提交：23ea36b（日志按天轮转 + 非阻塞 worker 落盘）/ 11cee56（根上下文多阶段镜像 + 构建上下文收窄）/ ff81627（运行时层预建 /app/data）/ 4cca76b（配置失败即退出 + 启动期逐项校验 + GAMER_PROFILE）/ db6e423（config.toml 出库为本地文件 + config.example.toml 跨平台化）/ 3e16e96（gamer 去 privileged、USB 直通以 override 承载）/ 016e9fc（PowerShell 编码坑记录）
- 已完成任务：OPS-001～OPS-004 主体子项（见 §7 复选框及行尾注记）
- 未完成任务及原因：阶段 1 checklist 已全部完成；USB 直通真机验证受 Windows Docker Desktop 平台限制，仅完成 compose 配置校验，真机回归仍归阶段 7 设备矩阵。
- 测试结果：docker build 同 HEAD 两遍——首遍 Rust 依赖预热层 CACHED、业务 crate 增量重编仅 38.39s，第二遍全层 CACHED；构建上下文传输 1.07MB（约 5.8GiB 的 server/target 不在内）；`docker compose config -q` 默认 / usb override / --profile redroid 三组合全部通过；镜像 569,866,252 字节 ≈570MB；cargo test 34 通过（含 4cca76b 配置校验 7 项、23ea36b 日志轮转 6 项新单测）
- 性能前后对比：Docker 构建上下文由约 5.8GiB 降至 1.07MB；服务日志由单文件无限追加改为按天轮转默认保留 14 天；裸容器启动即退出问题随 /app/data 预建消除
- 新增 PITFALLS：「Docker 镜像启动即 GLIBC_2.39 not found」「BuildKit cache mount + dummy-main 依赖预热会产出空壳二进制」「Docker Desktop 构建 load metadata auth.docker.io 连接超时而 CLI pull 正常」「PowerShell 管道改写含中文的 UTF-8 源码会静默毁坏文件结构」（016e9fc）「配置加载不再自建 data 目录后裸容器启动即退出」
- 发布/回滚说明：config.toml 自此为本机文件（本机副本保留，不随仓库分发）；自定义配置挂载到容器 GB_CONFIG 路径；原 privileged 部署迁移需叠加 docker-compose.usb.yml

## 17. 推荐的第一次执行范围

第一次实施建议只完成以下内容，不同时进入鉴权或性能重构：

1. QG-001：统一前端包管理器。
2. QG-002～003：抽离脚本校验器并建立 Vitest，清零当前 11 项失败。
3. QG-004～005：Rust 质量基线与 CI。
4. OPS-001：增加 `.dockerignore` 并验证干净 Docker 构建。
5. OPS-003：日志轮转。
6. OPS-004：配置解析失败即退出。

这批完成后，项目才具备安全执行阶段 2、3 和 5 的回归保护。
