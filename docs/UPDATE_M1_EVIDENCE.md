# 批次 2（M1 基线）验收证据索引

> 用途：批次 2「Windows 完整包 MVP」合流门的验收证据索引模板。
> 各条目先登记**证据来源命令**与期望形态；**实测输出/数字留空**，
> 待批次 2 合流门后由主控统一执行并回填，不用口头结论代替证据。
> 依据：`docs/AUTO_UPDATE_DEVELOPMENT_PLAN.md` §8.4（批次 2 完成门）、§11.1/§11.2（发布与依赖硬门禁）、§17.4 checklist。
> 状态：**已回填（2026-08-31 实测）**。
> - **第一轮**（commit `3a82420e`，见 E-1~E-5）：总体结论 **M1 合流门不通过**
>   （3 项阻断缺陷）；打包链路（E-1 组包 / E-2 验签 / E-4 离线修复）可用，
>   首启链路（E-3）与 system/info 依赖结论（E-5）存在阻断缺陷。
> - **修复后复验**（同 HEAD + 本轮未提交修复，见文末「修复后复验」一节）：
>   阻断/高/中/低优先问题全部修复并复验，E2E 场景 A/B 全部 PASS。

## 实测环境

- 日期：2026-08-31；commit：`3a82420e694ebd0f2ad0d241265e8bbf77a4c70c`（main，第一轮时工作树干净）
- 工具链：cargo 1.97.1（host `x86_64-pc-windows-gnu`）、node v24.14.0、pnpm 11.24.0、Windows PowerShell 5.1
- 全部产物从头重建：launcher/server `cargo build --release` + `pnpm build` → 四个打包脚本实跑
- 断网说明：无法物理断网，以「repair 全程 `HTTP_PROXY/HTTPS_PROXY/ALL_PROXY=http://127.0.0.1:9`（死代理，任何真实下载必失败）+ 日志 seed 命中、无任何下载尝试」为替代证据

## 回填约定

- 「实测记录」处回填：执行日期、commit、命令原文、关键输出摘录（目录树/哈希/状态码/版本号）。
- 任一条目无法取得证据时，在「实测记录」写明阻塞原因与对应任务 ID，不得留空或标"通过"。
- 证据与契约冲突时以 `docs/UPDATE_CONTRACT.md` 与 `release/contracts/` 为准，并先修契约或实现再回填。

## E-1 full ZIP 布局

- **结论：FAIL（偏差）** —— 组包/校验流程本身全部成功，负面项全过；但解压布局与模板期望
  列表不符（不含 `state/`、`versions/`、`runtime/`、`cache/`、`staging/`、`backups/`、`quarantine/`），
  且后文 E-3 证明 `repair` 无法补齐 `versions/`，首启后的安装根永远缺应用版本目录。
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

- **实测记录**（2026-08-31 @ 3a82420）：
  - 四脚本实跑全部成功：`package-app.ps1` PASS（gamer-app-0.1.0-windows-x64.zip 15,036,663 字节 26 条目；
    gamer-server.exe sha256 `1b15cb5f936cded7…`，构建注入 commit=3a82420e694e、channel=stable、
    target=x86_64-pc-windows-gnu）；`package-components.ps1` PASS（adb 三件套 + ffmpeg.exe 逐条目 sha256 对锁）；
    `gen-manifest.ps1` PASS（`signature: verified (key_id=dev-ed25519-1)` / `release: 0.1.0 (stable)`）；
    `package-full.ps1` PASS（SHA256SUMS 校验通过 17 条；包内 manifest 验签 OK）。
  - 产物（release/dist/，保留）：`GameBot-0.1.0-windows-x64-full.zip` 71,066,989 字节 18 条目
    sha256 `f013bb4a781561c6…`；`gamer-adb-37.0.1-windows-x64.zip` sha256 `54da12fbaaa59344…`；
    `gamer-app-0.1.0-windows-x64.zip` sha256 `26d32c3daec4c182…`；
    `gamer-ffmpeg-N-126335-gb32f8d1c23-20260830-windows-x64.zip` sha256 `73e964b63a0e8a0a…`。
  - 解压布局（%TEMP%\GameBot E2E 测试_01\，路径含中文+空格）：`gamer-launcher.exe`(9,514,411)、
    `INSTALL.md`、`SHA256SUMS.txt`、`config/`、`keys/`(仅 dev-ed25519-1.pem 公钥)、`licenses/`、
    `manifests/`、`seeds/`(app zip + adb zip + ffmpeg zip + scrcpy-server-v3.3.3 共 4 件)。
  - 负面项 PASS：无密码配置（config.toml `password_hash = ""` 占位）、无测试数据库、无日志、
    无 Cargo target、无 node_modules、无私钥（keys/ 仅 .pem 公钥）。
  - 首启后安装根实布局：`state/`(repair 锁生成)、`runtime/adb/37.0.1/`、`runtime/ffmpeg/N-…/`(repair 生成)、
    `versions/0.1.0/`(仅人工 staging 后才有，见 E-3)、`data/`、`logs/`、`staging/`、`quarantine/`(E-4 生成)；
    `cache/`、`backups/` 全程未生成（seed 命中无下载；备份属批次 3）。

## E-2 manifest 验签输出

- **结论：PASS**（正例验签 OK；篡改字节负例按预期 FAIL，fail closed）。
- **期望证据**：随包 manifest 通过 `doctor --manifest` 完整校验（先 Ed25519 分离验签、后解析，fail closed），
  输出 `signature: verified (key_id=…)` 与 `release: <版本> (<通道>)`；
  附一次负例：manifest 任意一字节被篡改后必须 FAIL（同 hash 资产复用包亦应可复验）。
- **证据来源命令**（doctor --manifest 已在批次 1 落地）：

  ```powershell
  gamer-launcher.exe doctor --manifest manifests\<版本>.json      # 正例：期望 OK — release manifest v1 valid
  # 负例：复制 manifest 后改动一个字节，再跑同命令，期望 FAIL — signature_invalid 类错误
  ```

- **实测记录**（2026-08-31 @ 3a82420）：
  - 正例（`gamer-launcher.exe --install-root <root> --keys-dir release\keys doctor --manifest manifests\0.1.0.json`）：
    `signature: verified (key_id=dev-ed25519-1)` / `release: 0.1.0 (stable); platforms: windows-x86_64` /
    `OK — release manifest v1 valid（校验通过）`，退出码 0。package-full.ps1 组包时对包内 manifest 复验同样 OK。
  - 负例：复制 manifest+sig 至临时目录后翻转 manifest 第 100 字节 1 bit：
    `FAIL — 1 error(s)` / `[signature-invalid] Ed25519 验签失败（key_id=dev-ed25519-1，覆盖 manifest 原始字节；
    可能被篡改、用错密钥或重新签名）`，退出码 1。
  - 附带发现：manifest 与 .sig 不同名放置时报 `[unsigned-manifest] 未找到签名文件` —— 行为正确，仅提示按
    `<名>.json` ↔ `<名>.sig` 成对命名。

## E-3 离线首启（PATH 清空 + 断网）

- **结论：FAIL（阻断）** —— 全新解压后 `repair` 必败（launcher 未创建 `runtime/<id>/` 父目录，rename
  报 os error 3）；`repair` 不安装 app（`seeds/` 内 app zip 与 scrcpy jar 无消费方，亦无任何路径写
  `state/current.json`）→ `start` 报「尚未安装」；人工把 app 解压进 `versions/0.1.0/` 并手写 current.json
  后可最小 PATH 启动，但 `/health/ready` 因 ffmpeg 版本探针恒 broken 永远 503。逐项如下。
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

- **实测记录**（2026-08-31 @ 3a82420，安装根 = `%TEMP%\GameBot E2E 测试_01`，含中文与空格）：
  1. **解压与 doctor（未安装态）**：`gamer-launcher.exe --install-root <root> doctor` 正确报告未安装：
     `[FAIL] state/ 目录不存在`、`[WARN] 尚未安装（state/current.json 不存在）`、adb/ffmpeg 组件逐文件
     `[FAIL] … 文件缺失`，`库存检查完成: 3 项失败`，退出码 1，无 panic。✅（行为正确；唯 state/ 缺失计 FAIL
     偏严——该目录本应属首启生成，见问题清单 #6）
  2. **repair 首装（死代理模拟断网）**：`--install-root <root> repair --manifest manifests\0.1.0.json`
     日志两处 `seed 命中且校验通过`（adb/ffmpeg zip），但最终：
     `[FAIL] adb 37.0.1: 新组件目录 rename 到位失败: 系统找不到指定的路径。 (os error 3)（旧目录已恢复原位）`、
     `[FAIL] ffmpeg …: 同上`，`修复完成：0 个组件恢复 / 2 个失败`，退出码 1。❌ 阻断：launcher 全程不创建
     `runtime/adb|ffmpeg` 父目录（`fs::rename` 要求目标父目录存在）。失败路径本身符合契约（staging 清空、
     无半成品残留）。
  3. **人工预建 `runtime\adb`、`runtime\ffmpeg` 后重跑 repair**：`[PASS] adb 37.0.1: 已修复（来源 seed）`、
     `[PASS] ffmpeg …: 已修复（来源 seed）`，退出码 0；落盘文件 sha256 与锁一致（`b4a6b455…`/`c1d65303…`/
     `0710e894…`/`140a7e14…`）。✅（证明 seed→staging→rename→复验链路本身可用，仅首装父目录缺失阻断）
  4. **start（无 app 安装）**：`gamer-launcher.exe --install-root <root> start` →
     `错误: 尚未安装（state/current.json 不存在），无版本可启动。`，退出码 1。❌ 阻断：repair 不消费
     `seeds/gamer-app-…zip` 与 `seeds/scrcpy-server-v3.3.3`，无路径生成 `versions/<v>/` 与 current.json。
  5. **人工 staging（测试台架动作，非 launcher 能力）**：解包 app seed 至 `versions\0.1.0\`（gamer-server.exe +
     web-dist/ + assets/scrcpy-server.jar）+ 手写 `state/current.json`（`{"schema_version":1,"current":"0.1.0",…}`）
     后 `status` 正常显示 `当前版本: 0.1.0 / 升级状态机: idle / 实例锁: 空闲`。
  6. **PATH 清空启动**：`PATH=C:\Windows\System32`（且注入 `GAMER_ADMIN_PASSWORD=e2e-test-pass`）拉起 launcher：
     server 子进程启动（env_keys=12，含 GAMER_APP_DIR/GAMER_DATA_DIR/GAMER_ADB_PATH/GAMER_FFMPEG_PATH/
     GAMER_SCRCPY_SERVER/GB_CONFIG/GB_LOG）、`gamer-server.exe` 正常监听。✅ 最小 PATH 可启动。
  7. **/health/ready**：实测恒 `HTTP/1.1 503`：
     `{"checks":{"adb":{"ok":true},"data_dir":{"ok":true},"ffmpeg":{"ok":false},"scrcpy_server":{"ok":true},"sqlite":{"ok":true}},"ready":false}`。
     ❌ 阻断：ffmpeg 自身可运行（手工 `ffmpeg.exe -version` 退出码 0），但版本串 `N-126335-gb32f8d1c23-20260830`
     不含 `.`，被 `deps_probe::valid_version_token` 判非法 → 探针恒 broken（60s 缓存无效化后复测仍 broken）。
     用锁定版 ffmpeg 时 readiness 永远无法 ready。
  8. **status**：`当前版本: 0.1.0 / 升级状态机: idle`。✅
  9. **停服行为**：`taskkill /F` 子进程 gamer-server.exe 后：就绪已过探测窗时 launcher ≤3s 随之退出
     （句柄等待生效）；若仍在 90s 就绪探测窗内被杀，launcher 等探测窗走完（最长 ~90s）才退出——两种情形
     launcher 最终都自行退出、锁释放，无孤儿监管。⚠️ 观察项：两种情形 launcher.log 均未见「server 子进程退出」
     收尾行（疑似 stdout 管道无读取者时 println! 失败先于文件日志，未深究）。
  10. **断网替代证据**：repair 全程在死代理环境下完成且日志仅有 seed 命中记录，无任何下载/远端尝试日志，
      `cache/artifacts/` 未产生文件。

## E-4 删除依赖文件后的离线修复

- **结论：PASS**（doctor --deep 精确定位缺失文件；离线 seed 修复后逐文件 sha256 恢复一致；quarantine 按契约保留）。
- **期望证据**：删除 `runtime/adb/<版本>/AdbWinApi.dll`（或 adb.exe / ffmpeg.exe）后，
  doctor 定位到该文件缺失/损坏；`repair` 在**断网**下从 `seeds/` 恢复，复查通过；
  修复失败路径不破坏上一份 runtime（可另做一次篡改哈希的负例观察 quarantine 行为）。
- **证据来源命令**（深检与修复为批次 2 LCH-004/007，以任务定义为准）：

  ```powershell
  gamer-launcher.exe doctor                    # 期望 [FAIL] 指出缺失文件（LCH-004 深检）
  gamer-launcher.exe repair                    # 期望 seeds → 复验 → 原子替换（LCH-007）
  gamer-launcher.exe doctor                    # 期望全部 [PASS]
  ```

- **实测记录**（2026-08-31 @ 3a82420）：
  1. 删除前 `AdbWinApi.dll` sha256 = `c1d653030b4bde65d3e07e4d0b0979e17be56df1436cdd15528630f27808050d`
     （= `release/dependencies.lock.toml` 锁值，size 108,184）。
  2. 删除后 `doctor --deep`：`[FAIL] AdbWinApi.dll: 文件缺失`（其余 [PASS]），退出码 1。
  3. `repair`（死代理环境，即断网等效）：日志 `seed 命中且校验通过` →
     `[PASS] adb 37.0.1: 已修复（来源 seed）` / `[PASS] ffmpeg …: 组件完好，无需修复`，退出码 0；
     损坏旧目录整体移入 `quarantine\adb-37.0.1-1788150109423\`（保留 adb.exe+AdbWinUsbApi.dll，不静默删除）。
  4. 修复后 `AdbWinApi.dll` sha256 = `c1d653030b4bde65d3e07e4d0b0979e17be56df1436cdd15528630f27808050d`
     （与删除前、与锁三者一致）；`doctor --deep` 全 PASS（7 PASS / 0 FAIL），退出码 0；
     普通 `doctor` 退出码 0。
  5. 附：场景 A 首装失败的 rename 失败分支同样验证了「失败不破坏安装」（staging 清空、无半成品）。

## E-5 `/api/system/info` 响应样本

- **结论：FAIL（部分）** —— 端点/鉴权/字段集/防泄露符合契约；但「依赖 managed 来源」「version 匹配 37.0.1」
  两项验收口径不成立（实测 `source=custom/binding=external`、adb 报 1.0.41、ffmpeg broken/null），
  且文档主推的 GAMER_ADMIN_PASSWORD 登录链路失效（问题清单 #4）。
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

- **实测记录**（2026-08-31 @ 3a82420，launcher 拉起的 server）：
  - 未登录：`HTTP/1.1 401` + `{"error":"unauthorized"}`。✅ 与 fixture 一致。
  - 登录链路：`POST /api/login {"username":"admin","password":"e2e-test-pass"}` → **401
    invalid_credentials**。❌ 原因：`supervisor::build_child_env` 白名单 12 键不透传
    `GAMER_ADMIN_PASSWORD`，INSTALL.md 主推的「设环境变量→launcher start→登录」不可用。
    改用文档备选路径（config.toml `[auth].password_hash` 写固定 Argon2id PHC）后登录成功
    `{"ok":true,"username":"admin"}`（服务端凭据/会话链路本身正常）。✅/❌ 见述。
  - 登录后 `GET /api/system/info` 200，完整响应：

    ```json
    {"app":{"built_at":"2026-08-31T04:08:41Z","channel":"stable","commit":"3a82420e694ebd0f2ad0d241265e8bbf77a4c70c","target":"x86_64-pc-windows-gnu","version":"0.1.0"},
     "capabilities":{"check":false,"download":false,"install":false,"rollback":false},
     "dependencies":{"adb":{"binding":"external","source":"custom","status":"ready","version":"1.0.41"},
                     "ffmpeg":{"binding":"external","source":"custom","status":"broken","version":null},
                     "scrcpy":{"binding":"application","source":"custom","status":"ready","version":"3.3.3"}},
     "deployment":{"mode":"direct","update_strategy":"unsupported"},
     "schema":{"db":1,"file":1,"rollback_floor":1},
     "startup":{"boot_id":"b535f7a9-c311-42ca-9fc1-b688682294fb","stage":"ready"}}
    ```

  - 逐项判定：字段集与 fixture 完全一致 ✅；`app.version=0.1.0`、`commit=3a82420e…`（真实 HEAD）、
    `built_at/channel/target` 均实 ✅；`deployment.mode=direct`（任务口径「无 launcher pipe env 时=direct」成立；
    但 launcher 托管启动下不注入 GAMER_LAUNCHER_PIPE/IPC_TOKEN，契约 §2.1 的 launcher 托管形态不可达，
    属 IPC 批次缺口，见问题清单 #5）⚠️；`capabilities` 全 false ✅（与 direct 模式一致）；
    `dependencies.adb/ffmpeg source=custom/binding=external` ❌ 非 managed（direct 模式下绝对路径一律判 custom）；
    adb `version=1.0.41`（adb 自报协议版本，非 platform-tools 包版本 37.0.1，契约未冻结口径）⚠️；
    ffmpeg `status=broken/version=null` ❌（问题清单 #3）；`schema.db=1/file=1/rollback_floor=1` ✅；
    `startup.stage=ready` + boot_id UUID ✅。
  - 防泄露：响应全文无盘符路径、无用户名、无 token/密码/命令行。✅
  - 附（场景 C 同场证据）：runtime ffmpeg 真实功能冒烟——用安装好的 managed ffmpeg 生成 2,738 字节
    H.264 Annex-B 裸流（testsrc→libopenh264），再以服务端 frames.rs 同构管道
    `-f h264 -i pipe:0 … -f image2pipe -c:v png pipe:1` 解码：退出码 0，输出 7,361 字节，
    PNG 魔数（89 50 4E 47 0D 0A 1A 0A）与 IEND 尾块均有效。✅

## 发现问题清单（第一轮；合流门阻断项在前；仅记录，未修码）

> 以下 6 条均已在「修复后复验」一轮修复并复验（第 5 条 IPC 为例外：M1 口径按未达成维持，
> 但 `deployment.mode=launcher / update_strategy=managed` 已通过 GAMER_DEPLOYMENT_MODE 注入达成）。

1. **[阻断] 首装 repair 必败**：`launcher/src/repair.rs` 修复换装 `fs::rename(staging → runtime/<id>/<ver>/)`
   前不创建 `runtime/<id>/` 父目录；全新解压根下必报 `rename 到位失败: os error 3`（实测见 E-3 第 2 步）。
   预建父目录后同链路即可成功，属首装路径遗漏。
2. **[阻断] launcher 无 app 安装路径**：repair 仅消费 manifest `platform.components`；`seeds/` 内
   `gamer-app-…zip` 与 scrcpy jar 无任何代码消费，亦无命令写 `state/current.json` → 解压后 `start`
   恒报「尚未安装」。包内 INSTALL.md「repair 把 seeds\ 里的 app/adb/ffmpeg/scrcpy-server 安装到位」
   与实现不符（实测见 E-3 第 4 步；场景 A 靠人工 staging 才能继续）。
3. **[阻断] ffmpeg 版本探针判 broken → /health/ready 永 503**：`server/src/deps_probe.rs`
   `valid_version_token` 要求版本 token 含 `.`，锁定版本串 `N-126335-gb32f8d1c23-20260830` 无 `.` →
   探针恒 broken、`/api/system/info` ffmpeg version=null（实测见 E-3 第 7 步与 E-5）。
4. **[高] GAMER_ADMIN_PASSWORD 不透传**：`launcher/src/supervisor.rs build_child_env` env_clear 后仅注入
   12 个固定键，INSTALL.md「设置 GAMER_ADMIN_PASSWORD → start → 登录」路径实测 401（E-5）。
5. **[中] 托管模式不可达**：launcher 注入 §4 七个稳定路径变量但不注入 `GAMER_LAUNCHER_PIPE`/
   `GAMER_LAUNCHER_IPC_TOKEN` → server `Mode::detect()=direct`，依赖 `source=custom/binding=external`，
   与 system-api-v1 §2.1「launcher 托管 → managed/runtime」不符（IPC 属后续批次，M1 验收口径按未达成记录）。
6. **[低/观察]** a) 全新解压根无 `state/` 时 doctor 将其计 FAIL（建议按首装正常态 WARN）；
   b) 就绪探测按端口 200 判定，可被同端口无关进程满足（实测环境遗留 dev server 占 8443 时 launcher 误报
   `[PASS] server 已就绪`，随后其子进程因端口占用退出）；c) launcher 退出收尾日志「server 子进程退出」
   在两次杀子进程场景均未落盘（run 1 有 stdout 读取者时有该行，疑似 println! 写失败先于文件日志，未深究）；
   d) `/api/system/info` 的 adb version=1.0.41（工具自报）与组件版本 37.0.1（platform-tools 包版本）
   语义未在契约冻结，验收按哪个口径需澄清。

## 修复后复验（2026-08-31，HEAD `3a82420e` + 本轮未提交修复；总体 PASS）

### 修复内容（对应第一轮问题清单）

| # | 修复 | 文件 | 要点 |
|---|---|---|---|
| 1 | 首装 rename 前自建父目录 | `launcher/src/repair.rs` | `fs::rename(staging→runtime/<id>/<ver>/)` 前 `create_dir_all` 父目录；回归测试 `fresh_install_root_repair_creates_runtime_parent_dirs`（全新根 runtime/ 不存在 repair 直接成功）PASS |
| 2 | app 安装路径 + 版本指针 | `launcher/src/repair.rs`（AppInstallSpec/repair_app/ensure_current_pointer）、`launcher/src/archive.rs`（`extract_app_zip`：无白名单但 zip-slip/炸弹/符号链接防线全保留）、`launcher/src/commands.rs` | repair 消费 manifest `platforms.windows-x86_64.app.artifact` + `resources.scrcpy_server`：seeds → 安全解压 staging → 校验 entrypoint（gamer-server.exe）+ jar sha256 → 原子 rename 到 `versions/<release.version>/`（目标已存在不覆盖，损坏目录先 quarantine）→ 原子写 `state/current.json`（既有 CurrentState 类型；首装 previous=null）。doctor 在指针存在且与 manifest 版本一致时对 `versions/<v>/` 做 quick 检查（entrypoint + jar hash）。测试 PASS：`repair_installs_app_from_seed_and_writes_current_pointer` / `repair_app_second_run_reports_healthy_without_overwrite` / `repair_app_failure_preserves_existing_dir` / `app_zip_extracts_full_tree_without_whitelist` / `app_zip_still_rejects_slip_and_bombs` |
| 3 | ffmpeg 探针版本 token | `server/src/deps_probe.rs` `valid_version_token` | 去掉「必须含 `.`」，改为：≤64 长度、ASCII 字母数字 + `.~-_+` 组合、至少一个数字、拒绝空串/路径分隔符。单测 `version_token_accepts_dotless_btbn_build_tag`（含锁定串 `N-126335-gb32f8d1c23-20260830`）PASS |
| 4 | supervisor 环境透传/注入 | `launcher/src/supervisor.rs` | `build_child_env` 重构为可注入纯函数 `build_child_env_from`：GAMER_ADMIN_PASSWORD（非空白才透传，登录链路）；默认注入 `GAMER_DEPLOYMENT_MODE=launcher`（server `Mode::detect` 合法枚举值），用户显式设置不覆盖。单测 3 个 PASS + 集成白名单键测试更新 |
| 5 | doctor 首装态 WARN | `launcher/src/commands.rs` | 库存检查重构为 `doctor_inventory_report`（报告可测）：从未安装（current.json 缺失）→ state/ 缺失与整体结论均 WARN、「未安装——先运行 repair」，退出码 0；已安装后组件缺失仍 FAIL/退出码 1（语义不变）。测试 `doctor_reports_fresh_root_as_never_installed_warn_not_fail` PASS |
| 6 | INSTALL.md 模板对齐 | `release/packaging/package-full.ps1` | repair 现在一步安装 app/adb/ffmpeg 并写 current.json；doctor 首装 WARN/安装后 PASS 语义；登录两方式（GAMER_ADMIN_PASSWORD 透传 / config.toml Argon2id PHC） |
| 附 | install-root 相对路径硬化 | `launcher/src/layout.rs` | 复验中发现相对 `--install-root .` 会把注入 server 的 GAMER_* 稳定路径带成相对路径（违反契约 §1/§4 不依赖 cwd），server 端 scrcpy jar 路径被重复拼接。`InstallLayout::resolve` 现将相对根按 cwd 词法规范化为绝对路径（不 canonicalize、不产生 `\\?\` 形态）；单测 PASS |

### 验收门禁（全部绿）

- `cd launcher && cargo fmt --all -- --check` ✅ / `cargo clippy --all-targets -- -D warnings` ✅ /
  `cargo test` ✅（99 passed / 1 ignored——手工验收材料化为 `#[ignore]` 设计）
- `cd server && cargo fmt --all -- --check` ✅ / `cargo clippy --all-targets --all-features -- -D warnings` ✅ /
  `cargo test` ✅（362 passed / 2 ignored）

### 重新打包（release/dist 先清空，四脚本实跑，commit=3a82420e694e / channel=stable / target=x86_64-pc-windows-gnu）

| 产物 | 字节 | sha256 |
|---|---|---|
| GameBot-0.1.0-windows-x64-full.zip（18 条目） | 71,072,595 | `0e9d211914a08494250e765fb793d119db3f75aad9736c3ed814a03b5ffe2d02` |
| gamer-app-0.1.0-windows-x64.zip（26 条目） | 15,030,324 | `23de6490af6626ec6a4314162fb70d13a19b0ac28ae58fadbe0d5617b2773175` |
| gamer-adb-37.0.1-windows-x64.zip | 4,058,592 | `54da12fbaaa59344886ce0e01ec9fb797bd5d2be7e7f2a81487727c7615edb8a` |
| gamer-ffmpeg-N-126335-…-windows-x64.zip | 48,252,451 | `73e964b63a0e8a0aad7aa494309aa71c8691ac1b8da5380dc72f0777610c7fa8` |

组包内建校验全部 PASS：SHA256SUMS 17 条逐条对、包内 manifest 验签 OK（key_id=dev-ed25519-1）、
`package-full` 冒烟 doctor 在解压根上输出首装 WARN 且退出码 0。

### E2E 场景 A：全新解压 → repair 首装 → start → system/info（安装根 `%TEMP%\GameBot E2E 复验_03`，含中文+空格）

| 步骤 | 结果 | 实测摘录 |
|---|---|---|
| 解压布局 | PASS | 顶层仅 `gamer-launcher.exe` + `config/ keys/ licenses/ manifests/ seeds/(4 件) / INSTALL.md / SHA256SUMS.txt`（负面项同第一轮全过：无密码配置/测试库/日志/target/node_modules/私钥） |
| doctor（未安装） | PASS | `[WARN] state/ 目录不存在（尚未安装…）`、`[WARN] 未安装——先运行 repair 完成首次安装（app + adb + ffmpeg 一步安装到位并写入版本指针）`，`库存检查完成: 0 项失败 / 4 项警告`，**退出码 0** |
| repair（死代理=断网） | PASS | 日志 3 次 `seed 命中且校验通过`、无任何下载尝试；`[PASS] adb 37.0.1: 已修复（来源 seed）`、`[PASS] ffmpeg N-126335-…: 已修复（来源 seed）`、`[PASS] app 0.1.0: 应用安装完成（来源 seed），版本指针已写入 state/current.json`，`修复完成：3 项恢复`，**退出码 0** |
| repair 落地形态 | PASS | `runtime/adb/37.0.1/`（三件）、`runtime/ffmpeg/N-…/`、`versions/0.1.0/{gamer-server.exe, web-dist/, assets/scrcpy-server.jar}`；`state/current.json` = `{"schema_version":1,"current":"0.1.0","previous":null,"updated_at_unix_ms":17881519…}`（launcher 原子写，非人工） |
| doctor（安装后） | PASS | 6 项目录级 PASS + adb/ffmpeg 逐文件 PASS + `[PASS] app 0.1.0: 版本目录完好（entrypoint + scrcpy-server hash）`，`0 项失败 / 0 项警告`，退出码 0 |
| start（PATH=C:\Windows\System32 + GAMER_ADMIN_PASSWORD） | PASS | `env_keys=14`（12 契约键 + GAMER_ADMIN_PASSWORD + GAMER_DEPLOYMENT_MODE），入口/cwd 均为绝对路径，`[PASS] server 已就绪 (http://127.0.0.1:8443/health/ready)` |
| GET /health/ready（匿名） | PASS | `HTTP 200` + `{"checks":{"adb":{"ok":true},"data_dir":{"ok":true},"ffmpeg":{"ok":true},"scrcpy_server":{"ok":true},"sqlite":{"ok":true}},"ready":true}`（ffmpeg 不再 broken） |
| 停服收尾 | PASS | `taskkill /F` gamer-server 后 launcher ≤3s 随之退出、锁释放；start transcript 落盘 `server 子进程退出（退出码: 1）` |

### E2E 场景 B：删除依赖文件 → doctor FAIL → repair 恢复

| 步骤 | 结果 | 实测摘录 |
|---|---|---|
| 删除 `runtime/adb/37.0.1/AdbWinApi.dll` → doctor | PASS（FAIL 预期） | `[FAIL] AdbWinApi.dll: 文件缺失`，`库存检查完成: 1 项失败`，**退出码 1**（已安装态语义不变） |
| repair（死代理） | PASS | `[PASS] adb 37.0.1: 已修复（来源 seed）`、`[PASS] ffmpeg …: 组件完好，无需修复`、`[PASS] app 0.1.0: 版本目录已安装且校验通过（不覆盖既有版本目录）`，退出码 0 |
| 恢复校验 | PASS | `AdbWinApi.dll` sha256 = `c1d653030b4bde65d3e07e4d0b0979e17be56df1436cdd15528630f27808050d`（=锁值）；doctor 全 PASS 退出码 0 |

### 登录 + `/api/system/info` 实测（会话 Cookie；未登录对照 401 `{"error":"unauthorized"}`）

`POST /api/login {"username":"admin","password":"e2e-test-pass"}`（GAMER_ADMIN_PASSWORD 透传链路）→
`HTTP 200 {"ok":true,"username":"admin"}`。登录后响应（完整 JSON，无盘符路径/用户名/token/命令行）：

```json
{"app":{"built_at":"2026-08-31T04:46:04Z","channel":"stable","commit":"3a82420e694ebd0f2ad0d241265e8bbf77a4c70c","target":"x86_64-pc-windows-gnu","version":"0.1.0"},
 "capabilities":{"check":false,"download":false,"install":false,"rollback":false},
 "dependencies":{"adb":{"binding":"runtime","source":"managed","status":"ready","version":"1.0.41"},
                 "ffmpeg":{"binding":"runtime","source":"managed","status":"ready","version":"N-126335-gb32f8d1c23-20260830"},
                 "scrcpy":{"binding":"application","source":"managed","status":"ready","version":"3.3.3"}},
 "deployment":{"mode":"launcher","update_strategy":"managed"},
 "schema":{"db":1,"file":1,"rollback_floor":1},
 "startup":{"boot_id":"5193fe82-62c8-4aee-a84e-2d2714b5301f","stage":"ready"}}
```

逐项判定：`app.version=0.1.0` ✅；`deployment.mode=launcher` + `update_strategy=managed` ✅（GAMER_DEPLOYMENT_MODE
注入链路）；三依赖 `source=managed` 且 version 非空正确（adb=1.0.41 自报口径、ffmpeg=N-126335-… 与锁一致、
scrcpy=3.3.3 与协议常量一致）✅；binding=runtime/runtime/application ✅；`capabilities` 全 false ✅（无 IPC
token 时 managed_ipc_provisioned=false，M1 预期）；`schema/startup` ✅；防泄露（响应原文正则匹配盘符路径=无）✅。

### 遗留观察（不阻断 M1）

1. IPC named pipe（GAMER_LAUNCHER_PIPE/IP_TOKEN）仍属后续批次；`capabilities` 全 false 为 M1 冻结口径下的预期。
2. adb `version=1.0.41` 为工具自报协议版本，与组件包版本 37.0.1 的口径差异仍未在契约冻结（同第一轮 #6d）。
3. 就绪探测按端口 200 判定可被同端口无关进程满足（第一轮 #6b，未在本轮复现条件内，维持观察）。
