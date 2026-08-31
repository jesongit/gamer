# GameBot 安装、依赖修复与升级指南

> 面向最终用户与维护者的 Windows x64 便携版（full 包）安装 / 修复 / 升级手册。
> 事实依据：`docs/AUTO_UPDATE_DEVELOPMENT_PLAN.md`（计划）、`docs/UPDATE_CONTRACT.md`（目录契约）、
> `release/contracts/*.md`（manifest / API / IPC / schema / 许可契约）、`launcher/`（启动器实现）。
>
> **诚实声明**：本文按「自动升级」项目的批次计划编写，**只在批次 0/1 已落地事实上描述现状**，
> 未落地能力一律标注「**规划中（批次 N）**」——规划中的命令与流程**当前尚不可用**，
> 请勿把本文当成已上线功能的承诺。各阶段完成状态以计划 §17 checklist 为准。

## 1. 能力现状一览

| 能力 | 状态 | 说明 |
|---|---|---|
| 契约冻结（目录/manifest/API/IPC/schema/许可） | 已落地（批次 0） | 见 `release/contracts/` |
| `gamer-launcher` CLI（start/status/doctor/repair/upgrade） | 部分落地（批次 1） | `status`/`doctor` 可用；`start`/`repair` 规划中（批次 2），`upgrade` 规划中（批次 3），当前执行会明确提示「尚未实现」且不改动安装目录 |
| manifest Ed25519 验签（`doctor --manifest`） | 已落地（批次 1） | 先验签后解析，fail closed |
| full ZIP 打包（`release/packaging/package-*.ps1`） | 规划中（批次 2） | 打包脚本由 REL-001/002/003 提供，暂无发布产物 |
| 依赖 inventory 深检与 repair 修复编排 | 规划中（批次 2） | LCH-004/005/006/007 |
| launcher 托管启动 server（`start`） | 规划中（批次 2） | LCH-008 |
| `/api/system/info` 系统信息 API | 规划中（批次 2） | SYS-001，契约已冻结（`release/contracts/system-api-v1.md`） |
| 自动升级（检查/下载/空闲安装）、快照与自动回滚 | 规划中（批次 3） | LCH-009~012、SYS-003~006 |
| Docker 镜像升级与 digest 回滚 | 规划中（批次 4） | DKR-001~004 |

## 2. 完整包安装（Windows x64）

> 本节流程的载体（full ZIP 与 `start`/`repair`）属**规划中（批次 2）**；落地前请使用
> README 的 Docker 或本地开发方式运行。以下为目标流程，供维护者评审与提前准备环境。

### 2.1 下载与解压

1. 从 GitHub Release 下载 `GameBot-<版本>-windows-x64-full.zip`（资产清单含
   manifest、分离签名 `.sig`、`SHA256SUMS`——发布与验签自动化**规划中（批次 2/3）**，落地前任何 ZIP 都应视为不可信）。
2. 解压到一个**建议纯 ASCII、不含空格的路径**（如 `D:\GameBot`）。这不是硬性要求——
   目录契约明确支持中文、空格与长路径（批次 5 QA-005 有专项验收）——但首次安装建议从简，
   减少第三方杀毒/同步盘/旧解压工具引入的变量。
3. 解压后的顶层布局见 §2.6；`versions/`、`runtime/`、`manifests/`、`seeds/`、`quarantine/`
   由 launcher 管理，运行期**只读**，不要手动改动。

### 2.2 首次自检：`gamer-launcher doctor`

```powershell
cd D:\GameBot
.\gamer-launcher.exe doctor          # 安装库存检查
.\gamer-launcher.exe doctor --manifest manifests\<版本>.json   # 校验 release manifest（验签）
```

- `doctor`（不带参数）检查安装根、`state/`、版本指针、`manifests/`、`runtime/`、`versions/`
  的存在性与完整性，输出 `[PASS] / [WARN] / [FAIL]`，存在 FAIL 项时退出码非 0。
  逐文件哈希与「缺 DLL/损坏/版本错」定位属于深检能力，**规划中（批次 2，LCH-004）**。
- `doctor --manifest` 已可用：对 manifest 先做 Ed25519 分离签名验证（公钥内置，未知 key、
  篡改一字节都会拒绝），再校验 schema、平台、版本与路径安全；输出 `signature: verified (key_id=…)`
  与 `release: <版本> (<通道>)`。
- `gamer-launcher status` 可随时只读查看当前版本、上一版本、升级状态机与实例锁。

### 2.3 安装运行依赖：`gamer-launcher repair`

**规划中（批次 2，LCH-007）**。目标行为：首次启动或依赖缺失/损坏时，按
「`seeds/` 离线包 → `cache/artifacts/` 缓存 → 远端下载」优先级补齐 adb/ffmpeg，
逐文件哈希校验后原子安装；详见 §3。当前版本执行 `repair` 会提示尚未实现且不做任何改动。

### 2.4 配置 `config/config.toml`

- 管理员密码：配置只接受 **Argon2id PHC** 格式的 `[auth].password_hash`；开发调试可用
  环境变量 `GAMER_ADMIN_PASSWORD` 在进程内生成。**没有默认账号/默认密码**，无凭据时拒绝启动（fail closed）。
- 常用项：`adb_path` / `ffmpeg_path`（managed 模式下由 launcher 注入绝对路径覆盖）、
  `scrcpy_server`、监听端口（默认 **8443**）、WebRTC 直连相关
  （`rtc_external_ip` / `rtc_udp_port` / `rtc_external_port`，NAT/Docker 场景必配）。
- `config/`、`data/`、`logs/`、`state/` 属用户数据/可写区，升级会被保留或纳入快照。

### 2.5 启动与访问

```powershell
.\gamer-launcher.exe start           # 规划中（批次 2，LCH-008）
```

启动后浏览器访问 `http://<主机>:8443` 登录。launcher 负责单实例锁、子进程监管与优雅停机；
服务异常退出时的重启策略以批次 2 落地实现为准。落地前的替代启动方式见 README「快速开始」。

### 2.6 目录速览（哪些不能删）

```text
GameBot/
├─ gamer-launcher.exe        # 启动器本体，位于版本目录之外
├─ config/  data/  logs/     # 用户配置 / 业务数据（SQLite + 脚本/模板）/ 日志 —— 升级会保留
├─ state/                    # current.json（版本指针）、update-journal.json、launcher.lock
├─ manifests/                # 已验签 manifest + .sig（current/previous 的属于升级证据）
├─ versions/<semver>/        # 应用版本目录（server、web-dist、scrcpy-server.jar），安装后只读
├─ runtime/adb|ffmpeg/<ver>/ # 锁定版本依赖，逐文件哈希
├─ seeds/                    # full 包内置离线组件包（断网首启/离线修复的来源，建议保留）
├─ cache/artifacts/          # 下载缓存 —— 唯一可随时整个删除的目录（会按需重建）
├─ staging/                  # 安装临时区，自动清空重建
├─ backups/<update-id>/      # 升级前数据快照（见 §4.5，勿手删）
└─ quarantine/               # 回滚失败/损坏数据保留区（见 §4.5，勿手删）
```

注意：`scrcpy-server.jar`（协议 3.3.3）与**应用版本绑定**，随 `versions/<semver>/assets/` 整体更换，
不能像 adb/ffmpeg 那样单独更新。

## 3. 依赖修复（doctor / repair）

### 3.1 doctor 报告解读

- `[PASS]`：检查通过；`[WARN]`：尚缺但可自动补齐（如未安装任何版本）；`[FAIL]`：需要修复，
  且 `doctor` 以非零码退出。`state/current.json` 损坏时 doctor 会先备份为
  `.corrupt-<时间戳>` 再按空状态处理并报 FAIL。
- 深检（对 `runtime/<依赖>/<版本>/` 逐文件哈希、可执行探针，定位「缺 adb DLL /
  ffmpeg 损坏 / 版本不符」）**规划中（批次 2，LCH-004）**；落地后 doctor 才能给出
  精确到文件的损坏报告。

### 3.2 repair 的修复优先级

**规划中（批次 2，LCH-005/007）**，目标顺序：

1. `seeds/` —— full 包自带的离线组件压缩包（最优先，无网络依赖）；
2. `cache/artifacts/` —— 之前下载并验签过的产物缓存；
3. 远端下载 —— 有界超时/大小限制，落盘走临时文件 + 原子改名，截断/哈希不符不污染安装目录。

修复流程为「inventory 发现损坏 → 取包 → 逐文件哈希复验 → staging 解压校验 → 原子替换，
损坏旧目录先移入 `quarantine/`」，保证**失败不破坏上一份可用的 runtime**。

### 3.3 离线修复条件

- full 包且 `seeds/` 目录保留 → 断网也能完成首次启动与依赖修复（批次 2 验收项）。
- `seeds/` 已清理且无网 → 无法修复，需恢复网络或重新下载对应组件包。
- 依赖来源为 `system`（用系统 PATH 工具）或 `custom`（用户显式指定的路径）时，
  repair **永不**改写、覆盖、重装这些路径的内容——这两种模式只探测与报告，修复仅针对
  `managed` 模式安装的 `runtime/` 目录。

## 4. 升级与回滚

> 本节能力（升级检查、下载、空闲安装、快照、自动回滚及对应 UI）整体**规划中（批次 3）**；
> 以下为已冻结契约（`release/contracts/system-api-v1.md`）定义的用户视角行为，
> 写在这里供提前理解与评审，不构成当前可用功能。

### 4.1 用户视角的升级状态机

页面上的更新状态共 11 态，典型成功链路：

```text
idle → checking（检查新版本）→ available（发现候选）→ downloading（后台下载+验签）
 → staged（就绪待装）→ waiting（等维护窗口/空闲）→ installing（停机、快照、迁移、切换）
 → restarting（新版本启动验证）→ idle（新版本生效）
```

要点：

- `install` 受理后服务会**自动重启、连接会断开**——断开不等于失败；重连后以
  「设置页显示的应用版本变化 / boot id 更新」判断结果。
- `failed`：提交前失败，旧版本仍在正常服务，可以重试或主动回滚。
- `rolling_back`：正在恢复旧程序 + 升级前数据快照，恢复成功回到 `idle`。

### 4.2 更新策略与维护窗口

| 策略 | 行为 |
|---|---|
| `off` | 不检查更新 |
| `notify`（产品默认建议值） | 自动检查、可后台下载，**由用户确认后才安装** |
| `auto` | 后台下载，且只在「维护窗口内 + 无活动脚本运行 + 无进行中的其他事务 + 距下一次定时任务触发大于冻结窗口」时自动安装；不满足就一直等待，**不会硬杀正在运行的脚本** |

维护窗口与冻结窗口为 `HH:MM` 本地时间与分钟数，建议默认 `02:00–06:00`、冻结 30 分钟
（建议值，最终以设置页/配置为准；设置页真实接入 UI **规划中（批次 3，WEB-005）**）。

### 4.3 失败与 `manual_recovery`：用户该看什么

- 升级失败通常伴随错误码（如 `signature_invalid` 验签失败、`artifact_invalid` 产物损坏、
  `insufficient_space` 空间不足、`schema_incompatible` 数据库 schema 超出新版兼容范围）。
  前两类重试可恢复；空间不足先清理磁盘；schema 不兼容需等待兼容版本，不可强行升级。
- **`manual_recovery` 是唯一没有自动出口的终态**：升级与自动回滚都失败时进入，系统会
  停止一切自动重试，并完整保留 `state/update-journal.json`（升级日志/步骤/错误摘要）、
  `backups/`（升级前快照）、新旧版本目录与 `quarantine/`。此时请联系维护者按人工恢复手册处理
  （维护者手册 DOC-002 **规划中（批次 5）**），**不要**自行删除上述任何目录后重试。

### 4.4 回滚边界

自动回滚只承诺「提交（committed）之前」：候选版本启动、迁移或健康检查失败时自动恢复旧程序
与升级前数据。一旦新版本提交成功，之后产生的新数据不属于自动回滚范围；此时降级旧版本属于
人工操作，schema 不兼容时必须明确接受「丢失升级后数据」的代价。

### 4.5 `quarantine/` 与 `backups/` 纪律

- `backups/<update-id>/`：每次升级前的数据+配置快照，至少保留与上一版本对应的已验证快照，
  是自动回滚的唯一依据——**不要手删**。清理只能依赖系统按数量/年龄/磁盘上限的保留策略
  （且永不删除 current、previous 与唯一有效回滚点）。
- `quarantine/`：回滚失败或损坏数据的保留区，**只增不自动删**，供人工恢复取证；
  只有显式的人工清理动作才能处置其中内容。
- `versions/`、`runtime/`、`manifests/` 同理：升级证据与版本目录交由 launcher 管理，
  手动删除可能导致无法回滚或修复。

## 5. Docker 用户注意事项

- **容器内没有安装/升级能力**：镜像在构建阶段已内置 Linux adb/ffmpeg/scrcpy jar，
  容器内不存在 launcher，`/api/system/info` 的更新策略恒为 `external`，
  一切 check/download/install/rollback 请求都会返回 `409 update_not_managed`——这是设计行为，不是故障。
- **升级由宿主负责**：拉取新镜像、重建容器即可；数据通过绑定挂载的 `data/` 目录保留。
  按 digest 固定与「新镜像不健康自动回旧 digest」的宿主升级脚本**规划中（批次 4，DKR-002）**，
  落地前请自行记录当前镜像 digest 以便回退。
- 不要在容器内尝试自更新、也不要给容器额外特权来「装依赖」；需要换依赖版本时换镜像。
- 容器停止/升级必须走 `docker stop`（SIGTERM），服务端会优雅停机并清理 scrcpy/adb 会话。

## 6. 相关文档

- 计划与任务拆分：`docs/AUTO_UPDATE_DEVELOPMENT_PLAN.md`
- 安装目录契约：`docs/UPDATE_CONTRACT.md`
- manifest/API/IPC/schema/许可契约：`release/contracts/`
- 日常使用（设备、脚本、定时任务）：`README.md`、`docs/YAML.md`
- 踩坑记录：`docs/PITFALLS.md`
