# 批次 3（M2 自动升级与回滚）验收证据索引

> 用途：批次 3 合流门「M1 基线能够升级到 M2，并在候选失败时自动恢复旧程序和数据」
> 的真实 Windows 进程级端到端证据。仿照 `docs/UPDATE_M1_EVIDENCE.md` 结构。
> 依据：`docs/AUTO_UPDATE_DEVELOPMENT_PLAN.md` §8.5（批次 3 完成门）、§11.3（数据、停机
> 和升级硬门禁）、§17.5 checklist；契约以 `docs/UPDATE_CONTRACT.md` 与
> `release/contracts/` 为准。
> 状态：**已实测（2026-08-31）**——M1(0.1.0) → M2(0.2.0) 真实升级 committed，
> 候选启动失败自动回滚恢复 0.1.0 + 升级前数据快照，两场景全部 PASS
> （最终验证轮 `logs/e2e-final2.log`：**75 项 PASS / 0 FAIL / 退出码 0**）。
> 复现脚本：`release/packaging/test-upgrade-launcher-e2e.ps1`（幂等，支持
> `-Scenario all|build|upgrade|rollback`、`-SkipBuild`，从零重跑已验证）。

## 实测环境

- 日期：2026-08-31；commit：`6f7792a0d33aea6e38a5da2faad8708e4f4890a9`（main）+
  本轮 launcher 未提交修复（见 §E-6 缺陷清单；launcher `cargo fmt/clippy/test` 全绿，
  174 passed / 1 ignored）
- 工具链：cargo 1.97.1（host `x86_64-pc-windows-gnu`）、node v24、pnpm 11、
  Windows PowerShell 5.1
- 全部产物从头重建：server/launcher `cargo build --release`（GAMER_GIT_COMMIT 注入真实
  commit）+ web `pnpm build` → `package-components / package-app / gen-manifest /
  package-full` 四脚本实跑（工作区自有 dist/keys/manifests，不触碰主仓 `release/dist`）
- M2 候选在**隔离副本**构建：robocopy 整仓（排除 `.git`、`target`、`node_modules`、
  `release/dist`、`release/vendor`），副本内改 `server/Cargo.toml` version → `0.2.0`
  （主树版本号未动），副本内 `cargo build --release` + `pnpm build` +
  `package-app.ps1 -RepoRoot <副本>`
- 安装根含中文+空格：`D:\e2e-upgrade-tmp\m2e2e\GameBot E2E 升级验证_A`（场景 A）与
  `…_B`（场景 B）；server 端口 18443/18444 错峰（8443 留给并行测试）
- 断网说明：与 M1 相同，repair 全程 `HTTP_PROXY/HTTPS_PROXY/ALL_PROXY=http://127.0.0.1:9`
  （死代理），日志仅 seed 命中、无任何下载尝试

## 两个与冻结契约的边界（先读）

1. **artifact URL 仅接受 https（契约冻结），本机 HTTP 服务承载 manifest 本体。**
   launcher manifest 模型（`launcher/src/manifest/model.rs` `is_https_url`）与 JSON Schema
   （`httpsUrl` pattern）双重强制 `https://`，而 `fetch_remote_manifest`（引擎）接受
   `http://` 并按 `<url>.sig` 约定拉分离签名。因此本机临时 HTTP 服务
   （python `http.server`，`127.0.0.1:18630`）承载 `0.2.0.json` + `0.2.0.json.sig`
   （以及 broken 变体），真实走 HTTP 获取 manifest + 验签 + 缓存；候选 app zip 通过
   `cache/artifacts/` 种子命中（seeds→cache→remote 链路的 cache 级），远端下载路径
   由 QA-002 专项测试覆盖。
2. **server 侧 install API 的 IPC 链路止于 prepare_install，最终接管走 launcher CLI。**
   实测链路：`POST /api/system/update/install` 202 受理 → 后台任务经 IPC 发
   `prepare_install` → launcher 复验 staging 后**驻留 staged**（`ipc/dispatch.rs`
   `run_long_op` 的 PrepareInstall 只调 `phase_prepare_install`，无后续动作）；
   drain→snapshot→switch→候选→commit 的编排入口当前只有 CLI
   `launcher upgrade --manifest`（`Engine::run_full`），且安装锁与 `start` 互斥——
   故先结束 `start` 释放锁（server 成孤儿存活），再由 CLI 接管（此时
   `was_running=true`，CLI 真实 `POST /api/shutdown` 优雅停掉孤儿）。批次 3 合流门
   「install API 202 先行返回」与「候选失败自动回滚」两条均已实证；「HTTP/IPC 一键
   触发完整接管」属尚未接线的缺口（见 §E-6 缺陷清单 #4）。

## E-1 构建与产物（四打包脚本 + M2 候选 + 故障候选）

- **结论：PASS**
- **实测**（2026-08-31，全部脚本实跑，工作区 `D:\e2e-upgrade-tmp\m2e2e`）：
  - `package-components.ps1`：vendor 逐文件 hash 对锁全对；
    `gamer-adb-37.0.1-windows-x64.zip`（4,058,592 字节）、
    `gamer-ffmpeg-N-126335-gb32f8d1c23-20260830-windows-x64.zip`（48,252,451 字节）
  - `package-app.ps1 -SkipBuild`：`gamer-app-0.1.0-windows-x64.zip`
    15,160,820 字节 26 条目（gamer-server.exe sha256 `55ec6d7a7c83f37e…`、
    jar sha256 `7e70323ba7f25964…`）；副本同法产出
    `gamer-app-0.2.0-windows-x64.zip` 15,160,906 字节（exe sha256 `1e2cc800521cc2fa…`）
  - `gen-manifest.ps1`：0.1.0 与 0.2.0 manifest 生成并签发
    （`signature: verified (key_id=dev-ed25519-1)`、`release: 0.2.0 (stable)`，
    validate-manifest check 全过；密钥对为本工作区 keygen，公钥随 full 包分发）
  - `package-full.ps1`：`GameBot-0.1.0-windows-x64-full.zip` 71,599,120 字节 18 条目，
    SHA256SUMS 全对、包内 manifest 验签 OK、doctor 冒烟退出码 0
  - 故障候选（场景 B 用）：副本 `server/src/main.rs` 注入
    `[E2E-SABOTAGE-BEGIN]…` 块——无子命令（= 正常 server 启动）时 3 秒后 exit(1)，
    `maintenance inspect/migrate` 子命令保留（快照 schema 门禁用候选 exe inspect
    校验副本，此路径必须可用，实测 `inspect --data-dir … --json` 退出码 0）。
    重打包为 `gamer-app-0.2.0-broken-windows-x64.zip`（独立 artifact 名避免 cache
    冲突），派生 `0.2.0-broken.json`（release.version 仍 0.2.0，app.artifact 指向
    broken zip 并更新 size/sha256）重签名，`validate-manifest.mjs check
    --expect-current-version 0.1.0` 通过（0.2.0 > 0.1.0 严格升级语义成立）

## E-2 M1 基线首装（解压 → repair → start → managed 能力）

- **结论：PASS**
- **实测**（场景 A，安装根含中文+空格）：
  1. 解压 full ZIP 后目录：`gamer-launcher.exe`、`config/`、`keys/`（仅 dev 公钥）、
     `licenses/`、`manifests/`、`seeds/`、`INSTALL.md`、`SHA256SUMS.txt`；改写
     `config/config.toml` `port = 18443`（用户数据，升级纳入快照）
  2. `repair`（死代理）：**退出码 0**，日志 3 次 `seed 命中且校验通过`
     （adb/ffmpeg/app zip），无任何下载尝试；
     `state/current.json` = `{"schema_version":1,"current":"0.1.0","previous":null,…}`
  3. `launcher start`（env：`GAMER_ADMIN_PASSWORD`、
     `GAMER_LAUNCHER_RELEASE_MANIFEST=http://127.0.0.1:18630/0.2.0.json`）：
     launcher.log `启动受管子进程 … env_keys=17`、`IPC pipe 已创建（DACL=仅当前用户+SYSTEM）
     \\.\pipe\gamebot-launcher-<id>`；`GET /health/ready` → **200**
     `{"checks":{"adb":true,"data_dir":true,"ffmpeg":true,"scrcpy_server":true,"sqlite":true},"ready":true}`
  4. `POST /api/login` → 200（GAMER_ADMIN_PASSWORD 透传链路）；
     `GET /api/system/info`：`app.version=0.1.0`、`deployment.mode=launcher`、
     `update_strategy=managed`、**`capabilities` check/download/install/rollback 全 true**
     （M1 时全 false 的 IPC 缺口已闭合）；三依赖 `source=managed/binding=runtime|application`
  5. 业务数据锚点：`POST /api/devices {"name":"e2e-marker-…","kind":"--"}` → ok
     （升级/回滚后的数据完整性断言用）

## E-3 server 驱动的更新链（check / download / install 全 202 + journal 推进）

- **结论：PASS**（install 202 → IPC prepare_install 复验后驻留 staged，见「边界 2」）
- **实测**（登录会话；先 `PUT /api/system/update/policy` strategy=off 关掉协调器
  自动流程，保证时间线由脚本显式驱动）：

  ```
  POST /api/system/update/check    → 202 {"state":"checking","update_id":"upd-…"}
    GET /api/system/update         → 200 state=available/checked
  POST /api/system/update/download → 202 {"state":"downloading"}
    GET                            → 200 state=staged/staged（cache 种子命中，逐 hash 校验）
  POST /api/system/update/install  → 202 {"state":"installing"}
    GET                            → 200 state=staged/staged（prepare_install 复验通过）
  ```

  - journal（launcher 侧 `state/update-journal.json`）同步推进：
    checking→checked→downloading→staged；update_id、from/to 版本、candidate 元数据
    齐备；`GET /api/system/update` 聚合与 system-api 契约 fixture 字段集一致
  - 鉴权门禁：未登录 401 `{"error":"unauthorized"}`（专项测试覆盖；本 E2E 以登录
    会话访问）；curl/PS 客户端不带 Origin 头 → Origin 缺失放行

## E-4 真实升级 E2E（CLI 接管 → committed 0.2.0）

- **结论：PASS**
- **实测**（场景 A；`taskkill` 结束 launcher `start`（**不带 /T**），server 孤儿存活
  且 ready —— 孤儿探活 PASS）：

  ```
  launcher upgrade --install-root <根> --manifest <工作区>\manifests\0.2.0.json
  → 退出码 0；stdout「升级完成: 0.1.0 → 0.2.0（committed，旧版本保留可人工回退）」
  ```

  - journal 15ms 轨迹（`logs/A-upgrade-journal-trace.log`，最终轮实测）：

    ```
    22:30:09.367  staged|staged            ← API 阶段驻留
    22:30:09.403  checking|checking        ← CLI phase_check（新 update id）
    22:30:09.433  checking|checked
    22:30:09.465  downloading|downloading
    22:30:09.715  staged|staged
    22:30:09.745  waiting_idle|waiting_idle
    22:30:11.636  snapshotting|snapshotting
    22:30:11.852  migrating|migrating
    22:30:11.947  switched|switched
    22:30:12.009  candidate_starting|candidate_starting
    22:30:13.117  idle|idle                ← committed → cleaning → idle 复位
    ```

    （draining/stopped/snapshot_verified/candidate_ready/activating/committed/
    cleaning 为亚秒级持久边，15ms 轮询仍可能漏记——journal 严格顺序推进，
    `candidate_starting` 出现即证明 switched/committed 等全部前序边发生过；
    launcher.log 里程碑 + 终态工件共同佐证）
  - launcher.log 里程碑：`check 完成，候选可用 version=0.2.0` →
    `启动受管子进程 versions\0.2.0\gamer-server.exe env_keys=18`（18=契约 12 键 +
    ADMIN_PASSWORD/DEPLOYMENT_MODE/ADMIN_TOKEN + gate/IPC 寻址）→
    `候选处于激活闸内，已先行 activate（幂等）`（闸内候选 /health/ready 恒 503，
    先 activate 才能翻转 200，见 §E-6 #2）→ `升级 committed 并清理完成 from=0.1.0 to=0.2.0`
  - drain 实证：CLI 对孤儿 0.1.0 server `POST /api/shutdown`（带 X-Admin-Token 回环
    管理令牌，见 §E-6 #1）→ server 优雅退出、端口释放 → journal `stopped`
  - 快照实证：`backups/<update-id>/manifest.json`（files 含 `data/gamer.db`、
    `config/config.toml`，逐文件 size+sha256）+ 快照副本逐文件复核全对
    （脚本 Verify-Snapshot：size+sha256 逐条对）
  - 候选身份：commit 前 ready 200 + `/api/system/info`（X-Admin-Token 时可直接观测；
    匿名 401 时按 ready body 回退，观测缺失按 spawn 路径锚定）+ boot_id 差异
  - 验收断言（最终轮全 PASS）：
    - `state/current.json`：`current=0.2.0`、`previous=0.1.0`
    - journal 终态 `idle`（committed → cleaning → idle 复位），error=null
    - `/health/ready` 200（新 server 即候选进程，commit 后成为孤儿继续服务）
    - 重新登录 `GET /api/system/info`：**`app.version=0.2.0`**（built_at/channel/target
      为副本构建注入值），capabilities 全 true
    - 业务数据保留：标记设备可查（升级前写入穿越快照/切换存活）
    - `launcher upgrade` 退出码 0
    - 快照复核：`backups/upd-…-8b4e/manifest.json` 3 文件 / 48,976 字节，
      逐文件 size+sha256 全对

## E-5 候选失败自动回滚 E2E（恢复 0.1.0 + 升级前数据快照）

- **结论：PASS**
- **失败注入**：候选 exe 为「启动即退出」故障构建（E-1），upgrade 在
  `candidate_starting` 后由 `wait_candidate_ready` 的 `child.try_wait()` 立即检出
  「候选进程在就绪探测期间退出」→ `fail_candidate` → `rollback_procedure`
- **实测**（场景 B，fresh 基线，broken manifest 同样经 server API check/download/
  install 202 推进至 staged）：

  ```
  launcher upgrade --install-root <根B> --manifest 0.2.0-broken.json
  → 退出码 1（FailedOldHealthy）
  ```

  - journal 15ms 轨迹（最终轮实测）：`staged → checking → downloading →
    waiting_idle → snapshotting → migrating → switched → candidate_starting →
    candidate_starting|rolling_back →`（终态 idle/failed）；
    `journal.error.message = 「候选进程在就绪探测期间退出」`
  - 验收断言（最终轮全 PASS）：
    - `state/current.json`：`current=0.1.0`、`previous=null`（回滚到基线，previous 链正确）
    - 快照恢复：`snapshot::restore` 现网 data/ 与 config.toml 整体挪入
      `quarantine/rollback-<ts>/`（不静默删除）→ 快照副本同卷换入 → 恢复后终验
      逐文件 hash 精确匹配；`quarantine/` 实测 1 项
    - 旧版本程序恢复：`restart_old_and_verify` 拉起 `versions/0.1.0/gamer-server.exe`
      → `/health/ready` 200 → 登录 `system/info` **`app.version=0.1.0`**
    - 数据完整性：标记设备原样保留（id/name/kind/addr 指纹一致）；新增设备仅
      旧版 server 重启后 adb 扫描自举的真实环境设备（kind=usb），非业务写入
      （候选在激活闸内无任何业务路由且 3 秒即退出）
    - 快照留存：`backups/<rollback 事务 id>/` 快照复核全对（回滚保留期证据）
    - switched 工件：`versions/0.2.0/gamer-server.exe` 存在（候选确实换入过，
      回滚才有效力）

## E-6 发现缺陷与修复清单（本轮真实链路暴露）

| # | 优先级 | 缺陷 | 根因 | 修复 | 验证 |
|---|---|---|---|---|---|
| 1 | 阻断 | launcher drain 旧版本 `POST /api/shutdown` 无凭据 → 服务端受保护组 401 → 引擎等端口关闭 90s 超时，升级恒被取消 | `/api/shutdown` 在受保护组；引擎未接回环管理通道 | （launcher 轨道修复）`state/admin-token` 持久令牌 + `LaunchExtras.admin_token` 注入子进程 `GAMER_ADMIN_TOKEN` + `drain_old_server` 携带 `X-Admin-Token` | 实测 drain 秒级完成，journal `stopped` |
| 2 | 阻断 | 候选在激活闸内 `/health/ready` 恒 503，而引擎等 ready 200 才 activate → 自我死锁 90s 超时回滚，真实链路永远无法 commit | 闸内路由 ready 固定 503（契约 §8），activate 由引擎在 ready 之后才调 | （launcher 轨道修复）`wait_candidate_ready` 检测 `503 + ready:false` 且已配置 IPC 时先行 `activate`（幂等），ready 翻转后继续身份校验 | 实测 launcher.log「候选处于激活闸内，已先行 activate（幂等）」→ ready 200 → committed |
| 3 | 高 | CLI 拉起的候选/回滚旧版进程以 `Stdio::inherit` 继承一次性升级器进程的 stdio；CLI 退出、管道读取端关闭后，继承句柄失效，候选再派生的外部探针（adb/ffmpeg readiness）持续失败 → `/health/ready` 恒 503 | 升级器进程是一次性的（commit 后即退出，候选成孤儿），stdio 不能继承 | （本轮修复）`engine.rs` `start_candidate` 与 `restart_old_and_verify` 改 `Stdio::null()`（server 日志走 GB_LOG 文件，不依赖继承 stdio） | 修复前实测：同 binary 同 env 手动重启后 ready 200；修复后脚本全链路 PASS。launcher fmt/clippy/`cargo test` 全绿（174 passed/1 ignored） |
| 4 | 中（缺口） | server install API 无法一键完成升级：IPC `prepare_install` 只复验 staging 驻留 staged；drain→switch→commit 编排只有 CLI 入口，且安装锁与 `start` 互斥 | `ipc/dispatch.rs` `run_long_op(PrepareInstall)` 未接 `run_full` 余下阶段（批次 3 的设计取舍：CLI=手动语义、API 只到「可安装」） | 本轮未改（属跨轨道接线决策：由 launcher IPC 线程执行 run_full 需先解决锁与所有权） | E2E 以「结束 start → CLI 接管」替代并在本节声明；任务口径允许 |
| 5 | 低 | 候选身份校验在真实鉴权下退化为空观测：`/api/system/info` 匿名 401 → 回退 `/health/ready` body（无 app_version/boot_id 字段）→ 版本/boot_id/schema 比对全部跳过，直接 commit | 身份探针未携带回环管理令牌 | 未改代码；#1 的 X-Admin-Token 通道已具备条件（info 请求附带该头即可拿到 200 真实字段），留接线 | 场景测试（mock 200）覆盖错版本/错 schema/boot_id 复用；真实链路本轮以登录态人工复核 app.version |

## E-7 未能完成 / 仍缺证据

- Windows clean VM（Win10/11、PATH 清空、真断网）、真实 Android 设备升级前后功能
  验证——本轮为单机 Windows 实测（端口错峰 + 死代理替代）
- 真实 GitHub Release / GHCR 下载源与生产签名密钥——本轮 manifest/签名全部本地
  dev-ed25519-1；远端 artifact 下载路径由 QA-002 专项测试覆盖，E2E 用 cache 种子
- server install API 一键接管（缺陷 #4）与候选身份探针带令牌（#5）的接线
- cron 冻结窗口/活动运行阻塞 install 的真实竞争（QA-006 已有测试；真实设备场景缺）
- launcher 自更新 trampoline（LCH-013）在真实占用场景的验证

## E-8 QA-007 大数据与磁盘压力（本轮）

> 复现脚本：`release/packaging/test-windows-stress.ps1`；fixture/helper：
> `tools/fixtures/perf-stage5b/`；临时台架与逐步日志：`D:\qa-stress-tmp\`。
> 本节只记录本轮真实执行结果，不修改主计划 checklist。

### 结论

- **真实大数据升级：PASS**。`-Phase qa007` 从零台架运行成功（退出码 0）：真实物理
  SQLite DB、4096 个小文件、真实 launcher `0.1.0 → 0.2.0` 升级、snapshot manifest
  self-hash/逐文件 SHA-256/size/文件集合复核及 SQLite integrity check 全通过。
- **中断恢复：PASS**。真实快照复制处于 `snapshotting` 时 `taskkill /F` launcher，旧
  `current.json`/data 未切换；重启后半截快照被丢弃，旧版本恢复 ready。
- **磁盘不足：替代测试 PASS，真实 OS 磁盘填满未执行**。launcher 的固定可用空间为 0
  测试验证 preflight 在 `current.json`、`data/`、`config.toml` 和 `backups/` 变更前拒绝；
  同时用稀疏 1 GiB 文件验证逻辑长度计数。宿主 D: 盘本轮仍有约 221.14 GiB 空闲，未用
  填满整盘的方式制造风险，因此不能把“真实 OS 磁盘满”写成全量通过。

### 实测环境与命令

- 日期：2026-08-31；Windows 10 x64；Windows PowerShell 5.1；本机 release launcher/server
  已构建；台架端口 28443。
- 真实主流程：

  ```powershell
  powershell.exe -NoProfile -ExecutionPolicy Bypass -File release\packaging\test-windows-stress.ps1 -Phase qa007
  ```

- 空间专项（可独立复跑）：

  ```powershell
  powershell.exe -NoProfile -ExecutionPolicy Bypass -File release\packaging\test-windows-stress.ps1 -Phase space
  ```

### 真实 DB/小文件/快照证据

- fixture 输出：`mode=real`、DB **1,075,576,832 bytes（1025.2 MiB）**，目标 1 GiB，
  **2049 × 512 KiB** materialized blob rows，`db_sparse_flag=not-sparse`，SQLite
  `integrity_check=ok`，`user_version=1`，server `maintenance inspect status=ok`。
- `data/com.example.qastress/` 下 **4096** 个小文件（超过 2048 门槛）；真实升级只允许
  在 `mode=real` 且 DB 被证明非稀疏时进入。
- 真实升级：`0.1.0 → 0.2.0`，最终 `exit=0`、wall **16.2 s**，journal 为
  `state=idle / last_step=idle`；snapshot update id 为
  `upd-1788191329866-0df9`，manifest 为 **4100 files / 1,075,983,008 bytes**。
- 独立复核：snapshot 与 live 均 **4100 files / 1,075,983,008 bytes / mismatch=0**；
  manifest self-hash 为
  `ad8bf159cdc2c02b174b90f19056ce422de1c302c00217309b61ba156b79a50f`；snapshot/live
  `gamer.db` 的 `integrity_check=ok`。hash 复核脚本未把 `backups/` 误算进 live 范围。
- 证据文件：`D:\qa-stress-tmp\logs\data-fill.txt`、`data-profile.txt`、
  `data-inspect.txt`、`s1-upgrade.txt`、`s1-verify-snapshot.txt`、`s1-journal.txt`、
  `s1-summary.txt`。

### 空间前置拒绝证据

- 稀疏替代 fixture：逻辑长度 **1,073,741,824 bytes**、`db_sparse_flag=sparse`、
  4096 个小文件；fixture 明确标记 `sqlite_integrity_check=not-run-sparse-fixture`、
  `real_snapshot_copy_allowed=false`，不代表真实 SQLite 复制。
- `cargo test --manifest-path launcher\Cargo.toml qa007_ -- --nocapture`：**3 passed / 0
  failed**，覆盖：固定可用空间为 0 的 `insufficient_space` 前置拒绝、稀疏 1 GiB
  preflight 逻辑计数、2048 小文件 snapshot/manifest/hash 测试。
- `space-summary.txt` 明确记录：`real_os_disk_full=NOT RUN`；`qa007` 主流程空间阶段记录
  D: 盘约 **219.5 GiB** 空闲，随后独立 `-Phase space` 复核记录约 **220.65 GiB**；未修改
  current/data/config，
  未创建有效 snapshot 的结论由固定 provider 测试断言给出。

### 中断恢复证据

- 升级 `0.2.0 → 0.3.0` 在 `snapshotting` 阶段强杀 launcher；journal 当时
  `state=snapshotting`、`snapshot=null`、`current=0.2.0`，半截 backup 存在但尚未登记
  为有效快照，`current.json parses=True`。
- 重启后：`ready=True`、server 进程数 1、`current=0.2.0`、journal `idle/failed`、
  `snapshot=null`、半截 backup `partial_backup_after=False`，启动输出明确为“快照阶段中断且
  快照不完整，数据未改动，已回退”；0.3.0 未提交。
- 证据文件：`D:\qa-stress-tmp\logs\s3b-killed-mid-snapshot.txt`、`s3b-state.txt`、
  `s3b-journal-after-kill.txt`、`s3b-final-start.txt`、`s3-pass.txt`。

### 测试脚本修复与实际阻塞

- 本轮修复了原脚本空目录 `Measure-Object.Sum` 在 strict mode 下的假失败、live verifier
  扫描 `backups/` 的范围错误，以及 PATH 选中 ffmpeg shim 导致 readiness 503 的台架问题；
  fixture helper 改为仓库内 `tools/fixtures/perf-stage5b/`，并对真实/稀疏模式 fail closed。
- 保留的覆盖缺口：**真实 Windows OS 磁盘满场景未执行**（本轮以 fixed provider + sparse
  preflight 替代）；真实 Windows 重启/注销与 clean VM 也不属于本轨本次执行。上述替代结果
  不应写成“磁盘满全量通过”。

## 踩坑记录（供主控收口 PITFALLS）

1. **PS 5.1 无 BOM 的 .ps1 按 ANSI(GBK) 解析**——脚本内含中文路径时按 UTF-8 写文件
   会读成乱码路径（「系统找不到指定的文件」）。规避：脚本统一 UTF-8 **带 BOM** 落盘
   （与 release/packaging 现有脚本惯例一致）。
2. **PS 5.1 `ProcessStartInfo.ArgumentList` 不存在**（.NET Core API）——用引号规则拼
   `Arguments` 字符串替代。
3. **PS 5.1 `$ErrorActionPreference='Stop'` 下原生命令 `2>&1`** 会把 stderr 行包装成
   终止性 ErrorRecord（vite/pnpm 一行 stderr 就炸掉整个脚本）。规避：原生调用统一走
   ProcessStartInfo + 输出重定向 + `$LASTEXITCODE` 判定。
4. **PS 脚本块事件委托（DataReceivedEventHandler + BeginOutputReadLine）在 .NET 线程
   池线程并发回调时可能静默崩掉整个解释器**（无 finally、无错误输出、exit code 2）。
   规避：`ReadToEndAsync` 两流并行读 + 退出后 `IsCompleted` 判定再取结果。
5. **子进程会继承父进程的管道写句柄**——升级 CLI 退出后，其拉起的候选 server 若继承
   stdio（Rust `Stdio::inherit`），读取端关闭会让候选再派生的子进程探针持续失败
   （缺陷 #3）；PS 侧 ReadToEndAsync 任务也因等不到 EOF 永不完成（阻塞 `.Result`）。
   规避：升级路径子进程 stdio 一律 `Stdio::null()`；PS 侧只取 `IsCompleted` 任务。
6. **.NET 正则 `$` 不匹配 CRLF 的 `\r` 之前**——`(?m)^port = 8443$` 改写 CRLF 文本
   不生效。规避：`(?m)^…\r?$`。
7. **adb 守护进程的 CommandLine 不含自身路径**（`adb -L tcp:5037 fork-server …`），
   按命令行清理进程匹配不到它，`runtime/adb/…/adb.exe` 文件锁会导致安装根整删失败。
   规避：按 `ExecutablePath` 匹配；taskkill 异步，删根前等进程列表清空并重试。
