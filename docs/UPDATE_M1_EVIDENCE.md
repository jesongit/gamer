# 批次 2（M1 基线）验收证据索引

> 用途：批次 2「Windows 完整包 MVP」合流门的验收证据索引模板。
> 各条目先登记**证据来源命令**与期望形态；**实测输出/数字留空**，
> 待批次 2 合流门后由主控统一执行并回填，不用口头结论代替证据。
> 依据：`docs/AUTO_UPDATE_DEVELOPMENT_PLAN.md` §8.4（批次 2 完成门）、§11.1/§11.2（发布与依赖硬门禁）、§17.4 checklist。
> 状态：**模板（待回填）** —— 本文不声明任何一项已通过。

## 回填约定

- 「实测记录」处回填：执行日期、commit、命令原文、关键输出摘录（目录树/哈希/状态码/版本号）。
- 任一条目无法取得证据时，在「实测记录」写明阻塞原因与对应任务 ID，不得留空或标"通过"。
- 证据与契约冲突时以 `docs/UPDATE_CONTRACT.md` 与 `release/contracts/` 为准，并先修契约或实现再回填。

## E-1 full ZIP 布局

- **期望证据**：full ZIP 解包后的顶层目录树，与计划 §5.1 / `docs/UPDATE_CONTRACT.md` §1 一致：
  `gamer-launcher.exe`、`config/`、`state/`、`manifests/`、`versions/<ver>/`（含 `web-dist/`、
  `assets/scrcpy-server.jar`）、`runtime/adb|ffmpeg/<ver>/`（adb 三件套 exe+DLL）、
  `seeds/`（离线组件包）、`cache/`、`staging/`、`backups/`、`quarantine/`；
  包内无密码配置、无测试数据库、无日志、无 Cargo target、无 node_modules、无私钥。
- **证据来源命令**（预期；打包脚本 package-full.ps1 由批次 2 REL-002 提供，以任务定义为准）：

  ```powershell
  release/packaging/package-full.ps1            # 生成 GameBot-<ver>-windows-x64-full.zip（REL-002）
  Expand-Archive GameBot-<ver>-windows-x64-full.zip D:\m1-evidence\unpacked
  Get-ChildItem D:\m1-evidence\unpacked -Recurse -Depth 2 | Select-Object FullName, Length
  ```

- **实测记录**：（待主控回填）

## E-2 manifest 验签输出

- **期望证据**：随包 manifest 通过 `doctor --manifest` 完整校验（先 Ed25519 分离验签、后解析，fail closed），
  输出 `signature: verified (key_id=…)` 与 `release: <版本> (<通道>)`；
  附一次负例：manifest 任意一字节被篡改后必须 FAIL（同 hash 资产复用包亦应可复验）。
- **证据来源命令**（doctor --manifest 已在批次 1 落地）：

  ```powershell
  gamer-launcher.exe doctor --manifest manifests\<版本>.json      # 正例：期望 OK — release manifest v1 valid
  # 负例：复制 manifest 后改动一个字节，再跑同命令，期望 FAIL — signature_invalid 类错误
  ```

- **实测记录**：（待主控回填，含 key_id 与版本/通道输出）

## E-3 离线首启（PATH 清空 + 断网）

- **期望证据**：clean 环境（无 Node/Rust/adb/ffmpeg，PATH 清空，网络断开）解压 full ZIP 后
  首次启动成功：launcher 从 `seeds/` 完成依赖安装，`/health/ready` 返回 200，
  data、SQLite、scrcpy_server、adb、ffmpeg 各项 ok；全程无远端网络请求依赖。
- **证据来源命令**（start 托管启动为批次 2 LCH-008，以任务定义为准）：

  ```powershell
  # 断网 + 新开终端：set PATH= 后执行
  gamer-launcher.exe start
  curl http://127.0.0.1:8443/health/ready      # 期望 {"ready":true,"checks":{...}}（匿名，契约 health-ready.success）
  gamer-launcher.exe status                    # 期望显示当前版本与升级状态机 idle
  ```

- **实测记录**：（待主控回填，记录 VM/环境与 ready 各项结果）

## E-4 删除依赖文件后的离线修复

- **期望证据**：删除 `runtime/adb/<版本>/AdbWinApi.dll`（或 adb.exe / ffmpeg.exe）后，
  doctor 定位到该文件缺失/损坏；`repair` 在**断网**下从 `seeds/` 恢复，复查通过；
  修复失败路径不破坏上一份 runtime（可另做一次篡改哈希的负例观察 quarantine 行为）。
- **证据来源命令**（深检与修复为批次 2 LCH-004/007，以任务定义为准）：

  ```powershell
  gamer-launcher.exe doctor                    # 期望 [FAIL] 指出缺失文件（LCH-004 深检）
  gamer-launcher.exe repair                    # 期望 seeds → 复验 → 原子替换（LCH-007）
  gamer-launcher.exe doctor                    # 期望全部 [PASS]
  ```

- **实测记录**：（待主控回填，注明删除的文件与前后 doctor 摘要）

## E-5 `/api/system/info` 响应样本

- **期望证据**：登录后 `GET /api/system/info` 返回 200，字段与冻结契约
  `release/contracts/system-api-v1.md` §2 及 fixture `system-info.success.json` 一致
  （`app.version/commit/built_at/channel/target`、`deployment.mode=launcher`、
  `schema.db/file/rollback_floor`、三依赖 `status/version/source/binding`、
  `capabilities`、`startup.stage/boot_id`）；响应中**不得**出现盘符路径、用户名、
  token、密码或完整命令行；未登录访问返回 401。
- **证据来源命令**（SYS-001 为批次 2 任务，落地前该端点为原型字段，以契约为准）：

  ```powershell
  # 登录取得会话 Cookie 后：
  curl -b <session-cookie> http://127.0.0.1:8443/api/system/info
  # 未登录对照：
  curl -i http://127.0.0.1:8443/api/system/info   # 期望 401 {"error":"unauthorized"}
  ```

- **实测记录**：（待主控回填，附完整响应 JSON 与 fixture 对照结论）
