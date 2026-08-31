# 批次 5 Windows 本机 QA 证据（QA-007 第一条 / 杀毒占用 / 强杀恢复）

> 用途：批次 5 中三项目前未勾选、但本机可实证的 QA 项验收证据
> （`docs/AUTO_UPDATE_DEVELOPMENT_PLAN.md` §17.7）。
> 依据：§11.3（快照/恢复硬门禁）、§11.5（杀毒占用、launcher 强杀恢复）、§17.7 checklist。
> 状态：**已实测（2026-08-31，HEAD `6f7792a` + 本轮未提交 launcher 修复）**。
> 本轮共发现 **3 个真实缺陷**（升级链路阻断级 2 个 + 回滚阻断级 1 个），已按最小修复原则
> 全部修复于 `launcher/src/**` 并回归全绿；详见「发现缺陷与修复」一节。

## QA-005 本轮自动化结果（2026-08-31）

本节是本轮 QA-005 的最新结果；下方原有场景 1～3 保留此前 QA-007/锁/强杀记录，
不把本机结果升级为 clean VM、Win10、真实杀毒软件或真实重启/注销证据。

- 主命令：
  `powershell.exe -NoProfile -ExecutionPolicy Bypass -File release\packaging\test-windows-qa005.ps1 -WorkDir D:\qa005-windows-final`
- 本轮 run：`D:\qa005-windows-final\run-20260831-154643-256`
- 预检：Windows 11 专业版 x64，Build 26200，PowerShell 5.1.26100.9168，
  `LongPathsEnabled=0`；C: 可用约 44.0 GiB，D: 可用约 222.3 GiB；
  `clean_vm_verified=false`。

| QA-005 项目 | 状态 | 实测结论 |
|---|---|---|
| 当前主机 full 首装/repair/升级/回滚（含中文+空格安装根） | **PASS（host-only）** | `test-upgrade-launcher-e2e.ps1 -Scenario all -SkipBuild` exit 0，wall 28.4 s；首装、ready、升级 committed、候选失败回滚均通过；不等价于 clean VM |
| Windows 10 x64 clean VM | **NOT_EXECUTED** | 当前主机为 Win11；没有可用 Win10 clean VM |
| Windows 11 x64 clean VM | **NOT_EXECUTED** | 当前主机虽为 Win11 x64，但不是 clean VM，未勾选该项 |
| 长路径安装 | **PASS（缺陷修复轮 2026-09-01 复核）** | 同一长度（安装根 264、`versions\0.1.0\gamer-server.exe` 296）在本轮修复后全链路 PASS；缺陷分析与修复证据见下方「缺陷修复轮」 |
| 程序/data 分盘 | **PASS（缺陷修复轮 2026-09-01 复核）** | C: 安装根 + D: 物理 data junction 首装/ready/升级 committed/快照校验全部通过；本轮（2026-08-31）曾因快照拒绝 junction 失败，修复见下方「缺陷修复轮」 |
| journal/current/exe 短暂独占锁 | **PASS（FileShare.None 模拟）** | journal status 明确报告读取失败且 journal 字节不变；current status exit 1 且字节不变；exe 锁下 start exit 1、未 ready |
| launcher 强杀/journal 恢复 | **PASS** | snapshotting 状态强杀 launcher；current 仍为 0.1.0；随后 `start` 输出 `RolledBack`、ready=True、journal idle；再次只杀 launcher 后孤儿 server 仍 ready |
| 真实杀毒引擎占用 | **NOT_EXECUTED** | 仅模拟 Windows 独占句柄，未修改或断言真实 AV 行为 |
| Windows 重启/用户注销 | **NOT_EXECUTED** | 未在共享桌面执行会终止 QA 会话的 reboot/logoff；需 clean VM checkpoint 由操作者执行 |

### QA-005 精确命令与证据

主 harness 内实际调用的标准 E2E 命令为：

`powershell.exe -NoProfile -ExecutionPolicy Bypass -File D:\code\gamer\release\packaging\test-upgrade-launcher-e2e.ps1 -Scenario all -SkipBuild -RepoRoot D:\code\gamer -WorkDir D:\qa005-windows-final\run-20260831-154643-256 -HttpPort 18641 -PortA 18461 -PortB 18462`

跨盘探针命令为同一 E2E 的 `-Scenario upgrade -SkipBuild`，并额外传入：

`-InstallRootA "C:\qa005-windows-cross\GameBot 跨盘 QA" -DataRootA "D:\qa005-windows-cross-evidence\run-20260831-154916-233\cross-drive-data-physical"`

锁与恢复由 `test-windows-qa005.ps1` 直接执行以下等价操作：

- `[System.IO.File]::Open(path, Open, ReadWrite, None)` 子进程分别锁定 `state\update-journal.json`、`state\current.json`、`versions\0.2.0\gamer-server.exe`，随后运行 `gamer-launcher.exe ... status/start`。
- 在 B 根运行 `gamer-launcher.exe --install-root <B> --keys-dir <B>\keys upgrade --manifest <run>\manifests\0.2.0-broken.json`，轮询 journal 至 `snapshotting` 后执行 `taskkill /F /PID <launcher-pid>`，再运行 `gamer-launcher.exe --install-root <B> start` 验证恢复。

证据文件：

- 汇总：`D:\qa005-windows-final\run-20260831-154643-256\evidence\qa005-summary.md`、`qa005-summary.json`
- 预检：`...\evidence\preflight.json`
- 标准 E2E stdout/stderr 与命令：`...\evidence\qa005-standard-e2e.*`；细粒度 journal/repair/info 记录在 `...\logs\`
- 锁证据：`...\evidence\lock-journal.log`、`lock-current.log`、`lock-server-exe.log`，以及对应 `qa005-*-status.*`
- 强杀恢复：`...\evidence\force-kill-fixture.txt`、`force-kill-mid-upgrade.txt`、`force-kill-recovery.txt`
- 长路径：`...\evidence\long-path-plan.txt`、`qa005-long-path-e2e.stderr.log`
- 跨盘：`D:\qa005-windows-cross-evidence\run-20260831-154916-233\evidence\cross-drive-layout.txt`（含 C/D 盘、junction 和 journal error）

附注：曾直接尝试 `powershell.exe -NoProfile -ExecutionPolicy Bypass -File release\packaging\test-windows-stress.ps1 -Phase all -Port 28543`，该其他 Agent 未提交脚本在入口因 `$Tools` 未定义退出；本轮锁/journal/强杀结论改由新增 harness 独立完成，未改动该文件。

## 缺陷修复轮（2026-09-01，Agent A）

针对本轮 QA-005 表中实测 FAIL 的两项（长路径安装、程序/data 分盘）的修复与复验。
工作树在 2026-08-31 未提交修复轮（缺陷 #1/#2/#3）基础上继续，修复仅涉及
`launcher/src/**` 与两个 QA 台架脚本；server/src 未改动。

### 缺陷 4：跨盘 data（C: 安装根 + D: data junction）升级快照失败（阻断）

- **现象**（2026-08-31 实测）：junction 建好后首装/ready 通过，升级在 snapshotting 报
  `artifact_invalid: 快照失败: 遍历 data/ 失败: 拒绝 symlink/reparse point`。
- **根因**：`upgrade/snapshot.rs` 的 `walk_files`/`restore` 对**遍历根自身**也做
  symlink/reparse 拒绝——而 data 根本身是 junction（指向另一块盘）是合法部署形态
  （UPDATE_CONTRACT §1 未禁止）；树内部条目仍需拒绝（防链接攻击语义）。
- **修复**（最小化，不放宽其它检查）：
  - 新增 `dir_root_metadata()`：**仅遍历根**允许是 symlink/junction，但必须解析到目录；
    `walk_files` 与 `restore` 的 data 根检查改走它，树内条目的 `safe_metadata` 逐项拒绝
    **原样保留**；
  - `restore` 回退后 data/ 变为含快照内容的真实目录，原 junction（指向被候选写坏的
    物理数据）随 rename 移入 quarantine 保留（不静默删除，物理数据不丢失）。
- **新增回归测试**（`launcher/src/upgrade/snapshot.rs`，Windows 实建 junction）：
  - `junctioned_data_root_snapshots_but_nested_reparse_is_rejected`（junction 根快照成功
    file_count=2 + 树内嵌套 junction 仍拒绝）；
  - `restore_with_junctioned_data_root_swaps_real_dir_and_quarantines_link`（junction 根
    回滚成功、恢复后为真实目录、旧 junction 留在 quarantine）。
- **复验证据**（官方 harness `-Phase cross-drive -SkipBuild -SourceAssets D:\qa-agentA-tmp\qa005
  -PortSeed 19640`，run `D:\qa-agentA-tmp\qa005-harness\run-20260831-172611-119`，wall 12.1s exit 0）：
  C: 安装根 + junction→`D:\...\cross-drive-data-physical`，upgrade 全链路 committed，
  journal 终态 `idle/idle / from=0.1.0 / to=0.2.0 / error=null`，
  `current.json = 0.2.0 / previous=0.1.0`，快照 `upd-1788197178940-72f6`：**3 文件 / 48976 字节，
  台架逐文件 sha256 复核全对（0 mismatch）**；`QA-005-cross-drive-layout: PASS`（journal 无错误，
  修复前同位置为 `artifact_invalid`）。另按证据文档命令做的探针（18700 段）同样 ALL PASS。

### 缺陷 5：长路径安装 FAIL（阻断）

- **现象**（2026-08-31 实测）：安装根 264 / `versions\0.1.0\gamer-server.exe` 296 字符，
  PS 5.1 `Expand-Archive` 在 `LongPathsEnabled=0` 下创建目录失败。
- **实测约束矩阵**（本机探针，`D:\qa-agentA-tmp\logs\lp-experiment.log` 等）：>260 路径下
  PS 5.1 `Join-Path`（verbatim 也炸）、`Expand-Archive`/`ZipFile::ExtractToDirectory`（verbatim 也炸）、
  `tar.exe -C`（chdir 受限，verbatim 也不行）、`Start-Process`/`ProcessStartInfo`（>260 exe
  即使 verbatim 也报「文件名或扩展名太长」）全部不可用；`robocopy`、PS `-LiteralPath`+verbatim、
  Rust `Command`（lpApplicationName verbatim）可用。另实测 **CreateProcessW 的
  lpCurrentDirectory 受 DOS 当前目录 ~260 上限（verbatim 超限同样 os error 267）**。
- **产品层修复**（`launcher/src/**`）：
  - `winutil::extended_len_path`：绝对路径统一规范化为 `\\?\` 扩展长度形态（`/`→`\` 词法归一、
    UNC→`\\?\UNC\`、幂等）；`InstallLayout::resolve` 收口调用，安装根及其派生路径（含注入
    server 的 GAMER_*/GB_* 环境变量）全部 verbatim 化——契约 §1「安装根可为长路径」在
    `LongPathsEnabled=0` 主机上的落地方式，注入值仍为绝对路径（§4）；
  - `supervisor::spawn_child_with_extras`：cwd 超限时经 `winutil::fallback_current_dir`
    回退到同一目录树最近 ≤240 祖先（server 全部路径经 env 绝对注入，不依赖 cwd）——修复
    `start` 在长根下 spawn 报 os error 267 的实测缺陷；
  - `commands::normalize_cli_paths`（`main` 调用）：`--install-root/--keys-dir/--manifest`
    等 CLI 路径统一 verbatim 化；
  - `winutil::terminate_pid_if_image`：镜像比较前剥离 verbatim 前缀（防 spawn 形态与
    QueryFullProcessImageNameW 返回形态差异导致拒绝终止孤儿候选）。
- **台架层修复**（`release/packaging/test-upgrade-launcher-e2e.ps1` + `test-windows-qa005.ps1`）：
  - 解压：`Expand-Archive` → `System32\tar.exe -xf` 到短 staging + `robocopy /E` 搬入安装根
    （tar 解包、robocopy 负责长路径最后一跳；两者均为 Windows 自带，不依赖注册表开关；
    tar 直接 `-C` 长目录实测进不去）；
  - 长根（>240 字符）下台架自身 FS 操作统一走 `\\?\`（新增 verbatim 感知的 `Join-Path`
    脚本内 shim + `ConvertTo-ExtendedPath`），launcher exe 从短路径 staging 中转 spawn
    （.NET 无法启动 >260 exe，产品侧 verbatim spawn 能力由被测对象承担）；
  - 清理阶段补 `Save-ProcessOutput`（子进程 stdout/stderr 落证据）；journal `migrating`
    为亚 15ms 透传快边，采样缺漏时按状态机顺序性（`switched` 已出现）判定；
  - `test-windows-qa005.ps1` 新增 `-SourceAssets`/`-PortSeed` 参数（默认值=原行为），
    支持指向带新 launcher 的自有资产目录与并行错峰端口。
- **复验证据**（官方 harness `-Phase long-path ... -PortSeed 19640`，run
  `D:\qa-agentA-tmp\qa005-harness\run-20260831-172630-843`，wall 11.9s exit 0）：
  安装根长度 **264**、server exe 路径长度 **296**（与修复前 FAIL 完全相同的长度）全链路 PASS：
  tar+robocopy 解压、repair 首装（seeds 离线）、`start` `/health/ready` **200**（launcher.log
  实录 `启动受管子进程 exe=\\?\C:\...（300 字符 verbatim）cwd=\\?\C:\...（222 字符回退祖先）`），
  `launcher upgrade` committed（exit 0）、journal `idle/idle / error=null`、
  `current.json = 0.2.0 / previous=0.1.0`、快照 `upd-1788197198624-08e2` **3 文件 / 48976 字节
  逐文件 sha256 复核全对**、升级后业务数据（标记设备）可查。标准 E2E 阶段（中文+空格根、
  upgrade+rollback 双场景）同轮回归 PASS（run-20260831-172751-067，wall 26.6s exit 0）。
- **doctor 说明**：长路径场景未单独跑 `doctor` 子命令；E2E 内的 repair（inventory 深检 +
  逐文件 sha256 + entrypoint/jar 校验）+ start ready 200 覆盖并超出 doctor 检查面，
  满足「能启动、ready 200」且高于 doctor 门槛。

### 回归与资源

- `launcher/`：`cargo fmt --check`、`cargo clippy --all-targets --all-features -- -D warnings`、
  `cargo test` 全绿（2026-09-01：**105 lib + 78 集成**，含 2 个新 junction 回归测试、
  1 个 cwd 回退测试、3 个 verbatim/归一化测试）。
- 资源：所有本 QA 拉起的 server/launcher/http.server 进程已结束（残留进程数 0），
  18700 段/19640 段/20640 段端口已释放；探针安装根（C:\qa-agentA-tmp\long、cross、
  lp-exp*、C:\qa005-windows-long\run-20260831-172630-843）已删除；
  证据保留在 `D:\qa-agentA-tmp\{qa005,qa005-harness,logs}\`。
- 新坑备忘（供 docs/PITFALLS.md 汇总，本文件仅记录）：PS 5.1 `Join-Path` 对 `\\?\` 路径
  抛「drive 为空」；`tar.exe -C` 无法 chdir 进 >260 目录（verbatim 也不行）；.NET
  ProcessStartInfo 无法启动 >260 exe（verbatim 也被拒）；CreateProcessW lpCurrentDirectory
  超 ~260 报 os error 267（verbatim 也不例外）；`Start-Process -ArgumentList` 数组项含空格
  不加引号会截断参数。

## 实测环境

- 日期：2026-08-31；commit：`6f7792a`（main，launcher 修复未提交，server/Cargo.toml 未改动）
- 工具链：cargo 1.98.0、node v24.14.0、python 3.14.3（sqlite3 内置）、Windows PowerShell 5.1、Windows 10 x64（本机，非 clean VM）
- 测试台架（test rig）：`D:\qa-stress-tmp\rig`（安装根，端口 28443，避免与本机 8443/其他并行 QA rig 冲突）
  - 基线安装：`repair --manifest manifests\0.1.0.json` 从 seed 离线安装 app 0.1.0 并写入 current.json；
    doctor 6 PASS / 0 FAIL / 0 WARN
  - runtime/adb/37.0.1（vendor 平台工具三件套）、runtime/ffmpeg/local-9.0（本机 ffmpeg，保证 /health/ready 200）
  - dev Ed25519 密钥对现场生成于 `D:\qa-stress-tmp\keys`（私钥不入库）；候选 manifest 用
    `release/packaging/sign-manifest.mjs` 签名、`release/contracts/validate-manifest.mjs check` 与
    launcher `doctor --manifest` 双重校验通过（`components: []`，runtime 依赖由台架预置）
- 压力数据（**真实复制，非稀疏文件**）：
  - SQLite：server 启动自建 schema v1 库后，向既有 `logs` 表插入 **2048 行 × 512 KiB 随机 blob
    （约 1 GiB）**，DB 实际 **1,075,052,544 字节（1025.2 MiB）**；`PRAGMA user_version` 保持 1；
    `PRAGMA integrity_check` = ok；server `inspect --json` = `status: ok / user_version: 1 / file_layout_v1: true`
    - 注意：server 启动时校验 v1 表集合**精确匹配**（实测多出的 `_qa_stress_blob` 自建表会被拒绝：
      `schema v1 is incomplete: expected tables [...], found [...]`），故压力数据写入既有 `logs` 表
  - 小文件：`data/com.example.qastress/{tmpl,func,yaml}/` 共 **4096 个**（≥2048 达标）
- 复现入口：`release/packaging/test-windows-stress.ps1`（`-Phase setup|data|scenario1|scenario2|scenario3|cleanup`，
  幂等可清理）；数据构造与校验工具在 `D:\qa-stress-tmp\tools\`（`fill_stress_data.py`、`verify_snapshot.py`、`gen_qa_manifests.py`）
- 证据留档：`D:\qa-stress-tmp\logs\`（逐条命令输出、journal 快照、launcher.log 拷贝）；本文引用的均为实测摘录

## 场景 1：QA-007 第一条——真实 1 GB DB + 大量小文件升级压力

- **结论：PASS** —— 真实 1 GiB DB + 4096 小文件经 launcher **真实进程升级链路**
  （check → download → drain → snapshot → switch → candidate gate → activate → committed）；
  快照 manifest 逐文件 hash 独立复算全部一致；快照副本与恢复后现网 `integrity_check` 通过；
  耗时与磁盘占用记录在案。
- **证据来源命令**：
  `launcher start`（托管起 server）→ `taskkill /F` launcher（留孤儿 server 模拟真实存量）→
  `launcher upgrade --manifest manifests\0.2.0.json` → `verify_snapshot.py <root> <upd-id>`

- **实测记录**：
  1. 升级链路全程（0.1.0 → 0.2.0，数据 1 GiB）：**exit 0，wall 14.4 s**。launcher.log 时间线：
     `12:44:13.263 check 完成` → `12:44:13.284 seed 命中且校验通过`（离线）→
     `12:44:25.924 启动受管子进程 versions\0.2.0`（drain+快照+换入共 12.6 s）→
     `12:44:26.473 候选处于激活闸内，已先行 activate（幂等）` → `12:44:27.096 升级 committed 并清理完成`。
     journal 终态：`state=idle / from=0.1.0 / to=0.2.0 / schema before=1 after=1 / error=null`；
     `current.json = {"current":"0.2.0","previous":"0.1.0"}`。
  2. 快照 manifest 独立复核（python 逐文件重算 sha256+size，不复用 launcher 代码）：
     `files=4099 bytes=1,091,830,072 mismatch=0 elapsed=8.7s`；快照副本 `gamer.db integrity_check: ok (1.6s)`。
  3. 磁盘占用（升级后）：`backups/` 1,092,617,224 B（约 1.02 GiB 快照）、`data/` 1,091,862,568 B、
     `versions/` 85.2 MiB（0.1.0 + 0.2.0 两套完整版本目录并存）、`staging/` 已清空（0 MiB）。
  4. 该场景在 3 轮实测（含复跑）中一致通过：4099 文件 / 1,091,830,072 字节 / 0 mismatch × 3。

## 场景 2：杀毒软件短暂占用模拟（FileShare.None 独占锁）

- **结论：PASS** —— journal / current.json / exe 三类占用分别实证：
  **明确失败且无半写状态、不误删 previous**；锁释放后重试成功；并观测到 journal rename 的
  有界重试成功路径。
- **证据来源命令**：PowerShell 子进程 `[System.IO.File]::Open($p,'Open','ReadWrite','None')` 持锁 N ms
  （模拟杀软扫描句柄），锁窗口内触发 `launcher upgrade / status / start`。

- **实测记录**：
  1. **(a) state/update-journal.json 整窗占用（3 s ＞ 重试窗口 ~10×25 ms）**：
     `launcher upgrade` **0.1 s 即明确失败**，`错误: journal 恢复失败: ... (os error 32)`，exit 1；
     journal 前后均为 `idle/idle`，current.json 不变，无 staging 残留——半写状态不存在（原子写+独占读失败路径）。
  2. **(a2) 锁落在启动窗口内的有界重试成功路径**（60 ms 后持锁 150 ms，配篡改签名 manifest 作为判定器）：
     upgrade 最终 stderr 为业务错误 `signature_invalid`（而非 journal IO 错误）——说明锁窗口内的
     journal 原子写经 `rename_with_retry`（10×25 ms，ERROR 5/32/33）成功落盘。
     注：`rename_with_retry` 无逐次重试日志，判定以「最终错误类型」为准（IO 错误=重试耗尽 / 业务错误=重试成功）。
  3. **(b) state/current.json 占用**：`status` exit 1 `读取 current.json 失败 ... os error 32`；
     `start` exit 1（0.1 s）同样明确失败、**未拉起任何进程**；释放后 `status` exit 0 且指针完整
     （current=0.2.0 / previous=0.1.0）。
  4. **(c) versions/0.3.0/gamer-server.exe 占用（升级切换期）**：升级链路走到
     `启动受管子进程 versions\0.3.0` 后 **spawn 失败**（CreateProcess 需真实打开 exe，被 FileShare.None 拦截；
     `verify_app_dir` 对入口只做存在性检查，故换入校验不拦）→ 触发 **committed 前自动回滚**：
     `现网数据已隔离保留（不静默删除）` → 恢复 1 GiB 快照 → current.json 切回 0.2.0 → journal `idle/failed`，
     **exit 1（FailedOldHealthy），wall 24.8 s**。恢复复核：live data 与快照 manifest 逐文件
     **mismatch=0**（4101 项，含当期库旁车文件）+ 双侧 `integrity_check: ok`；
     `versions/0.1.0、0.2.0` 均在（previous 未被误删）。**解锁后重跑同命令 → committed（exit 0）**。
  5. 附带实证：对**无令牌旧进程**的 drain 得到 `401 → 端口 90 s 未关闭 → 升级取消（旧版本未动）`——
     即缺陷 #1 的现象；修复后（X-Admin-Token）drain 正常返回、链路继续。

## 场景 3：launcher 强杀后 journal 恢复（真实进程级）

- **结论：PASS（强杀部分）** —— 单实例锁正确接管/清理、journal 恢复稳定、无半截 current.json、
  终态无多进程并存；升级中强杀可恢复到稳定状态。**真实 Windows 重启 / 用户注销无法在本环境模拟，
  该部分仍缺**（见「剩余缺口」）。
- **证据来源命令**：`launcher start`（后台）→ `taskkill /F /PID <launcher>` → `status` → 再 `start`；
  以及 `launcher upgrade`（后台）运行中按 journal 状态注入 `taskkill /F`。

- **实测记录**：
  1. **强杀托管中的 launcher**（server 子进程成孤儿）：孤儿 server 存活（procs=1）；
     `state/launcher.lock` **未被持有**（崩溃遗留锁文件被新实例静默接管，符合 LCH-002 设计）；
     `current.json` 可正常解析（`current=0.3.0`，无半截）；journal 稳定。
  2. **锁释放后 `status`**：exit 0，`实例锁: 空闲`，版本指针完整。
  3. **孤儿仍占端口时重启 `start`**：新子进程启动后因端口被孤儿占用退出（code 1），
     launcher 因就绪探测被孤儿满足而误报 `[PASS] server 已就绪` 后随子进程退出（≤3 s）——
     **终态单进程（孤儿）**、无双进程并存、无监管（该误报为已知观察项 M1 #6b，未扩大）。
     清理孤儿后 `start` → ready=True，监管恢复正常。
  4. **升级中途强杀（snapshotting 注入点）**：`launcher upgrade 0.3.0→0.4.0` 运行中，轮询
     journal 至 `state=snapshotting` 后 `taskkill /F`：journal 稳定停在
     `snapshotting / snapshot:null`，current.json 保持 `0.3.0`，半截快照目录留存。
  5. **重启恢复**：`launcher start` 输出
     `启动恢复: Aborted { reason: "快照阶段中断且快照不完整，数据未改动，已回退" }` →
     journal 回 `idle/failed`，旧版本 0.3.0 正常拉起，ready=True，server 进程数=1；
     事后 `integrity_check: ok`、2048 行 blob 数据完整。
  6. 「staged 状态强杀」变体：downloading 窗口过短未能确定性注入（轮询仅观测到该态）；
     staged/pre-install 各持久边的恢复语义由 `launcher/tests/qa004_journal_recovery.rs`（12 passed）
     与 snapshotting 实杀共同覆盖。

## 发现缺陷与修复（均为本轮实测暴露，修复限于 launcher/src/**）

| # | 级别 | 缺陷（实测现象） | 根因 | 修复 | 回归证据 |
|---|---|---|---|---|---|
| 1 | 阻断 | launcher 托管的 server 被 drain 时 `/api/shutdown` 恒 401（`status=401` → 等 90 s → 每次升级被取消） | launcher 匿名 POST；server 受保护组要求会话或 `X-Admin-Token` 回环管理通道，launcher 既不注入也不携带 | `installation::load_or_create_admin_token`（state/admin-token，64-hex）；supervisor 注入 `GAMER_ADMIN_TOKEN`（start/重启/候选三路）；engine drain 携带 `X-Admin-Token` | 修复后 drain 正常，场景 1/2/3 全链路可走通；`cargo test` 98 lib + 65 集成全绿 |
| 2 | 阻断 | 候选启动后死锁：引擎先等 `/health/ready` 200 再 activate，而维护闸内 ready 恒 503（`update_not_ready`）→ 90 s 超时 → 每次升级回滚，**M1→M2 真实升级永不能 commit** | server 闸内契约（503 待激活）与引擎轮询顺序不匹配；引擎场景测试用的是「立即 200」mock，未暴露 | `wait_candidate_ready` 在闸内 503（body `ready:false` 且 ipc 已配置）时先行 `activate`（幂等；403 立即失败；其余失败继续等待） | 实测日志 `候选处于激活闸内，已先行 activate（幂等）` → 0.6 s 后 committed；重复 activate 命中幂等回执 |
| 3 | 回滚阻断 | exe 被占导致回滚时必进 `manual_recovery_required`：`恢复快照失败: 快照 文件集合与快照不符：实际 4101 项，清单 4099 项` | server `inspect` 对 WAL 库的读写兜底打开在**快照副本旁**留下 `gamer.db-shm/-wal`（只读打开失败后的副作用）；恢复验证把这两个未收录旁车当多余文件 | `snapshot.rs`：验证时按良性旁车忽略「清单未收录的 `<db>-wal/-shm`」（恢复只复制清单内文件，不受影响）；快照创建收尾清理之；新增回归测试 `sqlite_sidecar_left_by_inspect_is_ignored_on_verify_and_cleaned_after_create` | 修复后同场景回滚成功：exit 1（FailedOldHealthy）、live data 与快照 mismatch=0、current 切回 0.2.0 |
| 附 | 观察 | 已提交候选（升级后现网进程）不带管理令牌 → 下一次升级 drain 401（与 #1 同源） | `start_candidate` 未注入 admin token | 一并注入（`with_admin_token`） | 场景 2/3 连续两次升级链路可 drain 前一版本 |

- 服务端连带观察（不在本轮修复范围，server/src 未改）：
  `maintenance::inspect` 的 READ_WRITE 兜底打开对 WAL 库有创建旁车文件的副作用（缺陷 #3 根因的
  server 侧），建议后续在 server 侧收敛（如只读打开失败时改用 `?immutable=1` 或显式清理）。

## 建议勾选的 checklist 条目（§17.7）

- [x] QA-007：1GB DB 和大量小文件升级压力测试通过。（本轮场景 1：真实 1 GiB blob 行 DB + 4096 小文件，
      真实升级链路 committed，快照 manifest 4099 文件逐 hash 独立复核 0 mismatch，integrity_check ok，
      耗时/磁盘占用留档；不代表磁盘满分支——该分支此前已单独验证并勾选）
- [x] 杀毒软件短暂占用 exe/current/journal 时有界重试且不误删 previous。（本轮场景 2：
      journal/current/exe 三类占用分别实证明确失败/有界重试成功/自动回滚不误删 previous，锁释放后重试成功）
- [~] Windows 重启、用户注销、launcher 强杀后 journal 恢复通过。（**强杀部分本轮已实证**：
      锁接管/清理、journal 恢复 idle 或按契约回滚、无半截 current.json、终态无多进程；
      **真实 Windows 重启与用户注销仍缺**，需 clean VM 物理操作，保持 [ ] 的重启/注销子项）

## 剩余缺口

1. 真实「Windows 重启 / 用户注销」恢复路径未模拟（本环境无法重启验证；QA-005 clean VM 项另计）。
2. 杀软占用为 FileShare.None 行为模拟，非真实杀毒引擎（真实扫描的锁窗口时序与重试/回退交互未验证）。
3. 本机非 clean VM；中文/空格路径仅有 host-only PASS。长路径与双磁盘（程序/data 分盘）
   两项原 FAIL 已在「缺陷修复轮（2026-09-01，Agent A）」修复并于本机 PASS（host-only），
   但 Win10/11 clean VM 上的最终验收仍缺。
4. `rename_with_retry` 无重试次数/间隔日志，「有界重试成功」以最终错误类型+代码路径（10×25 ms，ERROR 5/32/33）为证。
5. 升级 downlonding 阶段的强杀未确定性注入（窗口过短），该持久边恢复由 qa004 集成测试覆盖。
