# GameBot 第三方运行依赖分发决策（ARC-005）

> 状态：**决策记录（已评审基线）**
> 编制日期：2026-08-31
> 关联任务：AUTO_UPDATE_DEVELOPMENT_PLAN §17.2 ARC-005 三个 checklist 项；下游 DEP-001（锁文件）、DEP-002/003/004（获取与门禁）、DEP-005（第三方声明）
> 性质说明：本文是工程层面的许可决策与验收契约，不构成法律意见；发布前如条款发生变化，以来源原文为准并回改本文。

## 0. 结论速览

| 依赖 | 决策 | 许可 | 能否随 full 包再分发 |
|---|---|---|---|
| adb（Android Platform-Tools 三件套） | 来源锁定 Google 官方 dl.google.com；只取 `adb.exe` + `AdbWinApi.dll` + `AdbWinUsbApi.dll` | Apache-2.0（SDK 许可 §3.5 开源组件豁免） | **是**（须附带 Apache-2.0 文本与归属，逐文件 hash 入 lock） |
| ffmpeg.exe | **BtbN/FFmpeg-Builds 的 win64-lgpl（static）构建**；gyan.dev 已全面 GPLv3，排除 | LGPL-3.0-or-later（现行 BtbN 构建带 `--enable-version3`） | **是**（须附 LGPL 文本 + 对应版本源码 offer + buildconf 归档；严禁 GPL/nonfree 构建） |
| scrcpy-server.jar | 与应用版本强绑定 **3.3.3**，不独立升级 | Apache-2.0 | **是**（附 Apache-2.0 文本 + 归属；无 NOTICE 文件，无 §4(d) 保留义务） |
| Rust crate 依赖 | 不随包分发源码义务（编译进自家二进制） | 以扫描结果为准 | 由 cargo-deny 门禁保证清单一致 |

---

## 1. adb 决策（Android Platform-Tools）

### 1.1 来源锁定

- 官方渠道（Google 唯一权威源）：
  `https://dl.google.com/android/repository/platform-tools-latest-windows.zip`
  带版本号形态：`https://dl.google.com/android/repository/platform-tools_r<revision>-windows.zip`
- 版本号以 **`<PLATFORM_TOOLS_VERSION>` 占位**，实际值由 DEP-001 在 `release/dependencies.lock.toml` 锁定（版本、下载 URL、zip 级 sha256、逐文件 sha256）。
- 参照点（非锁定值，仅为调研当日实测记录）：**37.0.1**（zip 内 `source.properties` 的 `Pkg.Revision=37.0.1`），zip sha256 `45f4d63113e895ebde0c90f194099a4676b6ac653bd28d54314a9e022bbc1a99`（8,044,989 字节）。
- DEP-002 裁包规则：**只提取并打包 `adb.exe`、`AdbWinApi.dll`、`AdbWinUsbApi.dll` 三个文件**（与 manifest `components[id=adb].required_files` 一致）。包内其余文件（fastboot、mke2fs、sqlite3、etc1tool 等）不属于本项目运行面，不入包。

实测包内容（platform-tools-latest-windows.zip，2026-08-31）：
`adb.exe`、`AdbWinApi.dll`、`AdbWinUsbApi.dll`、`etc1tool.exe`、`fastboot.exe`、`hprof-conv.exe`、`libwinpthread-1.dll`、`make_f2fs.exe`、`make_f2fs_casefold.exe`、`mke2fs.conf`、`mke2fs.exe`、`NOTICE.txt`、`source.properties`、`sqlite3.exe`。

### 1.2 许可结论：**可随 full 包再分发（是）**

依据链（三步，全部有原文/实测支撑）：

1. **Android SDK 许可协议**（https://developer.android.com/studio/terms ，2026-08-31 取得全文）：
   - §3.1 仅授予"为 Android 兼容实现开发应用"的目的使用 SDK 的许可；
   - §3.4 限制条款原文：*"Except to the extent required by applicable third party licenses, you may not copy (except for backup purposes), modify, adapt, **redistribute**, decompile, reverse engineer, disassemble, or create derivative works of the SDK or any part of the SDK."*——注意开头让步状语 *"Except to the extent required by applicable third party licenses"*；
   - §3.5 豁免条款原文：*"**Use, reproduction and distribution of components of the SDK licensed under an open source software license are governed solely by the terms of that open source software license and not the License Agreement.**"*
2. **platform-tools Windows 包内自带 Apache-2.0 许可全文**（实测 `NOTICE.txt` 首行）：
   *"Android used by: sdk-repo-windows-x86-platform-tools.zip — Apache License Version 2.0"*。
   即 Google 自己以 Apache-2.0 声明了该包内软件（含 adb.exe 与两个 AdbWin DLL）的开源许可；adb 源码即 AOSP `platform/packages/modules/adb`（Apache-2.0）。
3. **结论**：按 §3.5，这些**开源许可组件**的再分发"仅由该开源许可条款管辖，不受 SDK 协议管辖"；Apache-2.0 允许再分发。因此 §3.4 的 redistribute 禁令因 "to the extent required by applicable third party licenses" 的让步不适用于 adb 三件套。
4. **行业先例**（实测佐证）：scrcpy v3.3.3 官方 Windows 发行包 `scrcpy-win64-v3.3.3.zip` 内即含 `adb.exe`、`AdbWinApi.dll`、`AdbWinUsbApi.dll`（Genymobile 长期在 Apache-2.0 发行包中直接再分发官方 platform-tools 组件）。

**再分发须满足的条件（DEP-002/DEP-005 落地）**：

- full 包 `licenses/adb/` 内附 Apache-2.0 许可文本与归属声明（见 §4）；
- 不修改三个文件的任何字节；重打包仅允许"从官方 zip 中原样提取"（裁剪非必需文件），逐文件 sha256 记入 `dependencies.lock.toml` 与 manifest `required_files`；
- lock 文件记录来源 URL、zip 级 sha256、`Pkg.Revision` 值，保证可追溯到官方渠道。

### 1.3 风险与退路

- **残余风险**：Google 未对"platform-tools 包是否整体属于可独立再分发的 SDK 产物"给出书面澄清；存在保守解释认为整个 zip 受 SDK 协议约束（§3.4）。本决策依据的是 §3.5 明文 + 包内 Apache-2.0 声明 + 事实先例，属合理的工程结论；但本文不是法律意见，商业化发行前可请法律顾问复核一次。
- **缓解**：只随包分发 `adb.exe` + 两个 DLL（不打包 fastboot 等其余工具）；二进制字节不动；声明目录齐全；来源与 hash 可审计。
- **退路（计划 §15 已预置）**：若该结论被推翻或法务要求收紧 → 公开发行退回 **lite 包 / 内部 seed 路线**（full 包不公开，仅内部使用；用户自装 platform-tools，GameBot 以 `system/custom` 模式引用）。ARC-005 未闭环前**禁止公开发布 full 包**。

---

## 2. ffmpeg 决策（Windows x64）

### 2.1 候选构建源对比

| 来源 | 构建与许可 | 结论 |
|---|---|---|
| gyan.dev（ffmpeg.org 官方推荐 Windows 源） | 页面明示 **"All builds are 64-bit, static and licensed as GPLv3"**；曾有的 release-lgpl 变体已停发 | **排除**（GPL，履约义务与预期不符） |
| **BtbN/FFmpeg-Builds**（GitHub Actions 自动构建） | 提供 `win64-gpl` 与 **`win64-lgpl`**（static）及 `-shared` 变体；lgpl 变体 buildconf 无 `--enable-gpl`/`--enable-nonfree` | **选定（首发路线）** |
| 自建（MSYS2/MinGW 或 Linux 交叉编译） | 完全可控，可做 `--disable-everything` 最小 LGPL 构建（仅 h264 解码 + PNG 编码 + pipe），体积可从 ~110MB 降到个位数 MB | **备选（M1 后体积优化）**，须按 2.3 归档 configure 行 |

### 2.2 选定路线与验收方式（含 2026-08-31 实测记录）

**选定**：`https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-<branch>-latest-win64-lgpl.zip`（static 变体，产物为单文件 `bin/ffmpeg.exe`，与计划 §5.1 的 `runtime/ffmpeg/<version>/ffmpeg.exe` 布局一致）。

⚠️ **供应链注意**：BtbN 的 `latest` 是持续滚动更新的 tag，资产会被覆盖。DEP-001 锁定时必须：记录**下载当时**产物的 zip sha256 与 `ffmpeg -version` 输出的版本串（形如 `N-xxxxx-g<commit>-<date>`）；并把该产物副本收入 seed/发布存储，后续修复/重装一律从 seed 取已锁产物，**而不是反复追 latest**。

**验收门禁（DEP-003 每次锁新版本必须全部通过）**：

1. `ffmpeg.exe -buildconf`：configuration 中 **不得出现 `--enable-gpl`、`--enable-nonfree`**；确认 x264/x265 等 GPL 库为 `--disable-libx264...`（BtbN lgpl 构建默认如此）。
2. `ffmpeg.exe -L`：输出为 **GNU Lesser General Public License**（现行构建因 `--enable-version3` 显示 version 3）。
3. **真实冒烟**（计划 §11.2）：项目实际使用的 H.264 Annex-B **stdin 管道 → PNG stdout** 命令成功，输出为合法 PNG。
   - 实测记录（2026-08-31，`ffmpeg-master-latest-win64-lgpl.zip`，zip sha256 `c55a9c349ef915565c5755473d858c98d80d96feefe693fe6ab38705d29c920e`，版本串 `N-126335-gb32f8d1c23-20260830`）：buildconf 无 gpl/nonfree ✓；`-L` 输出 LGPL v3 ✓；`type smoke.h264 | ffmpeg -f h264 -i pipe:0 -frames:v 1 -f image2pipe -c:v png pipe:1` 输出 22,948 字节、PNG 魔数有效 ✓。
4. 裁包：保留 `bin/ffmpeg.exe` 与许可文本，其余（ffplay/ffprobe/doc/presets）不入包；`ffmpeg.exe` 单文件 sha256 入 lock 与 manifest `required_files`。
5. 体积注意：该构建 `ffmpeg.exe` ≈ **110 MB**（全功能静态链接）。首发可接受；若需瘦身，转 2.1 备选"自建最小 LGPL 构建"，验收门禁不变。

### 2.3 LGPL 履约清单（DEP-003/DEP-005 落地）

依据 ffmpeg 官方法务页（https://ffmpeg.org/legal.html ）合规清单：*"Compile FFmpeg without '--enable-gpl' and without '--enable-nonfree'"*；*"Distribute the source code of FFmpeg, no matter if you modified it or not"*；*"Make sure the source code corresponds exactly to the library binaries"*；*"Explain how you compiled FFmpeg, for example the configure line, in a text file added to the root directory of the source code"*。

本项目通过 **subprocess 管道调用独立的 ffmpeg.exe**（聚合而非链接），应用自身不是 FFmpeg 派生作品、不受 LGPL 传染；义务只针对随包分发的 ffmpeg.exe 二进制：

- [ ] `licenses/ffmpeg/COPYING.LGPL-3.0.txt`（现行 BtbN 构建带 `--enable-version3`，按 **LGPL-3.0-or-later** 履约；若未来换用无 version3 构建则改附 LGPL-2.1 文本并在 lock 中记录所选版本）；
- [ ] **源码 offer**：与二进制精确对应的 FFmpeg 源码（锁定 commit 的 GitHub archive zip，或对应 nX.Y 分支 tag 的源码 zip），随发布资产提供直接下载；或至少提供有效期 ≥3 年的书面要约（written offer）。首发直接随附下载链接，不做要约；
- [ ] **构建参数归档**：`ffmpeg -buildconf` 完整输出 + BtbN 构建脚本/commit 引用 + 版本串，写入 `dependencies.lock.toml` 的 ffmpeg 条目，并在源码包根附 `BUILD-CONFIG.txt`（满足"解释如何编译"要求）；
- [ ] 发布下载页归属声明："本软件使用了 FFmpeg（LGPL），源码见 <链接>"（legal.html 的 attribution 要求，收进 NOTICE）；
- [ ] 确认构建未引入 GPL 库（libx264/x265 等）与非自由组件（如 libfdk-aac）；`--disable-libfdk-aac` 已在实测 buildconf 中确认。

---

## 3. scrcpy 决策（scrcpy-server.jar）

- **许可**：Apache-2.0（https://github.com/Genymobile/scrcpy/blob/master/LICENSE ，实测取得）。仓库根**无 NOTICE 文件**（实测 contents API），因此 Apache-2.0 §4(d) 的"保留 NOTICE 文件内容"义务不触发；须履行的仅是：随包附 Apache-2.0 文本副本 + 保留版权/归属声明（写入我们的 NOTICE）。
- **发布方式**：客户端与 server jar 同 tag 同版本发布。实测 v3.3.3 tag 资产：`scrcpy-server-v3.3.3`（90,164 字节）、`scrcpy-win64-v3.3.3.zip`、`SHA256SUMS.txt` + `SHA256SUMS.txt.asc`（官方签名校验和）。官方下载页：https://github.com/Genymobile/scrcpy/releases/tag/v3.3.3 。
- **强绑定（计划 §2、DEP-004）**：客户端协议严格对齐 3.3.3，jar **不得**像 adb/ffmpeg 一样独立升级。发布门禁三者一致才放行：
  1. 代码常量 `SCRCPY_VERSION = "3.3.3"`（`server/src/device/scrcpy.rs:23`）；
  2. `server/assets/scrcpy-server.jar` 实际版本与 hash（实测本仓库 jar 90,164 字节，与官方 `scrcpy-server-v3.3.3` 资产字节数完全一致；本仓库 jar sha256 `7e70323ba7f259649dd4acce97ac4fefbae8102b2c6d91e2e7be613fd5354be0`）；
  3. Release manifest `resources.scrcpy_server`（version=`3.3.3`、`binding: "application"`、sha256）。
  ⚠️ DEP-001 锁定时须从官方 `SHA256SUMS.txt`（可用 `.asc` 验签）复核上述 hash——本次调研中 GitHub 资产直连下载多次超时，未能逐字节比对官方 hash，字节数一致为强佐证但**不足以替代锁定时复核**。
- jar 命名：包内路径 `versions/<version>/assets/scrcpy-server.jar`；lock 记录官方资产名 `scrcpy-server-v3.3.3` 以便溯源。

---

## 4. 第三方声明策略（DEP-005 输入契约）

### 4.1 full 包 `licenses/` 目录结构

```text
GameBot/
└─ licenses/
   ├─ NOTICE.md                 # 本应用第三方声明（见 4.2）
   ├─ adb/
   │  ├─ LICENSE.txt            # Apache-2.0 全文
   │  └─ SOURCE.txt             # 来源 URL + Pkg.Revision + zip sha256（从 lock 生成）
   ├─ ffmpeg/
   │  ├─ COPYING.LGPL-3.0.txt   # LGPL-3.0 全文（随实际许可版本调整）
   │  ├─ BUILD-CONFIG.txt       # -buildconf 输出 + 版本串 + 源码对应关系
   │  └─ SOURCE.txt             # 源码 offer 直链（pinned commit archive）
   └─ scrcpy/
      ├─ LICENSE.txt            # Apache-2.0 全文
      └─ SOURCE.txt             # 官方 release URL + SHA256SUMS 引用
```

要求：**声明内容必须与实际打进包的二进制一致**（计划 §7.4 DEP-005 验收），由打包脚本从 `dependencies.lock.toml` 生成 SOURCE.txt，禁止手抄漂移。

### 4.2 NOTICE.md（本应用 NOTICE）

固定包含：产品名与版权行；逐组件归属行，至少：

- adb：Android Platform-Tools（`<PLATFORM_TOOLS_VERSION>`），Copyright The Android Open Source Project，Apache License 2.0，来源 <dl.google.com URL>；
- ffmpeg：FFmpeg（`<版本串>`），licensed under the LGPL v3（或按实际），来源与源码 <链接>；
- scrcpy：scrcpy（v3.3.3），Copyright (C) Genymobile，Apache License 2.0，来源 <release URL>。

### 4.3 SBOM 输入选型

- **格式：CycloneDX JSON（spec 1.5）**，由打包脚本作为输入生成（`tooling`/打包流水线产物，不手写）。
- 运行依赖部分（adb/ffmpeg/scrcpy-server）的 SBOM 条目直接从 `dependencies.lock.toml` 生成：name、version、purl（如 `pkg:generic/adb@37.0.1`、`pkg:generic/ffmpeg@N-...-win64-lgpl`）、hashes（sha256）、license 表达式、外部引用（官方下载/源码 URL）。
- Rust crate 依赖部分：用 **cargo-cyclonedx**（https://github.com/CycloneDX/cargo-cyclonedx ）从 `Cargo.lock` 生成 CycloneDX JSON 后合并。
- 发布资产附带 SBOM 文件（计划 §17.7 "Release 资产包含 SBOM、NOTICE"）。

### 4.4 Rust 依赖 license 扫描

- 工具：**cargo-deny**（https://github.com/EmbarkStudios/cargo-deny ），`cargo deny check licenses` 进入 CI 常规门禁。
- `deny.toml [licenses]` 建议基线 allow：`Apache-2.0`、`MIT`、`BSD-2-Clause`、`BSD-3-Clause`、`ISC`、`Zlib`、`Unicode-3.0`、`CC0-1.0`、`MPL-2.0`；对 `ring`/`rustls`/`aws-lc-rs` 等带 OpenSSL 衍生条款的特殊 crate 按扫描结果显式放行并逐条记录理由（当前栈 axum/tokio/webrtc 0.13 的实际清单以 DEP-005 扫描输出为准）。
- 每次发版归档：`cargo deny check licenses` 输出 + Cargo.lock hash；出现未放行 license 时 fail closed，不得临时绕过。

---

## 5. 红线（未满足任一条禁止公开发布 full 包）

1. **发布门禁**：ARC-005 本决策未闭环、DEP-001 lock 未建立、§4 声明目录未生成前，**不得公开发布 full 包**（计划 §3 非目标、§8.2 批次 0 完成门）；期间只允许内部原型与 seed/lite 路线。
2. **禁止 GPL/nonfree 冒充**：ffmpeg 只允许 lgpl 构建路线；`-buildconf` 出现 `--enable-gpl`/`--enable-nonfree`、或 `-L` 输出 GPL，即判验收失败并阻断发布；严禁拿 GPL 构建当 LGPL 分发。
3. **禁止修改二进制**：adb 三件套、ffmpeg.exe、scrcpy-server.jar 必须按官方产物原字节再分发；重打包仅允许裁剪非必需文件（原字节提取），**逐文件 sha256 记入 `dependencies.lock.toml`** 并与 manifest `required_files` 一致；出现任何 hash 不一致即阻断发布（scrcpy 另受 DEP-004 三方强绑定门禁）。
4. **声明完整性**：`licenses/` 内容、NOTICE、SBOM 必须与包内实际二进制一一对应，来源 URL 与版本可溯源；缺失任一组件声明即禁止发布。
5. **许可版本漂移**：第三方许可条款变更（如 platform-tools 包内声明、ffmpeg 构建许可、scrcpy 许可）时，必须回到本文重做决策并重走 DEP-001 锁定，不得沿用旧结论。

---

## 6. 调研证据索引（2026-08-31 实测）

| 事实 | 来源 / 方式 |
|---|---|
| Android SDK 许可协议 §3.1/§3.4/§3.5 全文 | https://developer.android.com/studio/terms （全文抓取） |
| platform-tools 官方渠道、版本 37.0.1、包内 14 文件、NOTICE.txt=Apache-2.0 全文 | 实测下载 https://dl.google.com/android/repository/platform-tools-latest-windows.zip 并解包读取 |
| gyan.dev 全部构建为 GPLv3（无 LGPL 变体） | https://www.gyan.dev/ffmpeg/builds/ （页面明示 "All builds are 64-bit, static and licensed as GPLv3"） |
| BtbN win64-lgpl 构建存在且可用 | https://github.com/BtbN/FFmpeg-Builds/releases （`ffmpeg-master-latest-win64-lgpl.zip` 实测下载 148,537,845 字节，sha256 `c55a9c34...d29c920e`） |
| 该构建无 --enable-gpl/--enable-nonfree、-L 为 LGPL、H.264 pipe→PNG 冒烟通过 | 实测运行 `ffmpeg.exe -buildconf` / `-L` / stdin→stdout 管道（见 §2.2） |
| ffmpeg LGPL 合规清单（无 gpl/nonfree、随附对应源码、buildconf 说明、归属） | https://ffmpeg.org/legal.html |
| scrcpy v3.3.3 发布资产（scrcpy-server-v3.3.3 = 90,164 字节；SHA256SUMS.txt/.asc） | GitHub API `repos/Genymobile/scrcpy/releases/tags/v3.3.3` |
| scrcpy win64 包内捆绑 adb.exe/AdbWinApi.dll/AdbWinUsbApi.dll（再分发先例） | 实测下载并列出 `scrcpy-win64-v3.3.3.zip` 内容 |
| scrcpy LICENSE=Apache-2.0、仓库无 NOTICE 文件 | https://raw.githubusercontent.com/Genymobile/scrcpy/master/LICENSE 、GitHub contents API |
| 本仓库 jar 与常量 | `server/assets/scrcpy-server.jar`（90,164 字节，sha256 `7e70323b...fd5354be0`）、`server/src/device/scrcpy.rs:23` |
| SBOM/license 工具 | https://cyclonedx.org/ 、https://github.com/CycloneDX/cargo-cyclonedx 、https://github.com/EmbarkStudios/cargo-deny |
