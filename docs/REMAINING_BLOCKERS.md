# 未完成项与阻塞原因清单

> 维护日期：2026-09-01
> 范围：[docs/AUTO_UPDATE_DEVELOPMENT_PLAN.md](AUTO_UPDATE_DEVELOPMENT_PLAN.md)（下称「更新计划」）与
> [docs/CLEAN_BASELINE_PARALLEL_PLAN.md](CLEAN_BASELINE_PARALLEL_PLAN.md)（下称「基线计划」）中**全部未勾选 checklist 项**。
> 口径：已完成项不在此列出，勾选依据见各计划内注记与 `docs/UPDATE_*_EVIDENCE.md`、
> `docs/CLEAN_BASELINE_FUNC_EVIDENCE.md` 证据文档。某项解除阻塞后：先在原计划勾选并回填证据，再从本文档移除该条。
> 当前基线：本地 main 领先 origin/main 17 个提交（含全部修复与证据），**未推送**。

## 总览

| 阻塞类别 | 未勾项数 | 解除方式 |
|---|---:|---|
| 一、GitHub 仓库配置与凭据（REL/QA-008/发布演练/发布资产/批次 5 合流门） | 11 | 仓库管理员配置 secrets/environments + 本机凭据登录 + 推送后复演练 |
| 二、物理环境（clean VM、真实重启/注销、真实杀毒、容器内 UDP 媒体面） | 5 | 提供物理/虚拟环境后按 §11 矩阵执行 |
| 三、历史过程审计（合并顺序） | 3 | 不可追溯，维持注记（见 §三） |
| 四、发布后观察（更新计划 §17.8） | 6 | stable 正式发布并度过首个观察周期 |
| 五、仓库内可自行完成的收尾项（无外部阻塞，未列入合流门必需） | 1（另 3 条观察项） | 按第五节接线点直接开发 |

---

## 一、GitHub 仓库配置与凭据阻塞

这一组共享同一批前置条件，配齐后可一次跑通。workflow 侧缺口已全部修复并提交
（`4dd1469` draft-first 与门禁收紧、`a7e5ef1` 校验脚本 refspec 修复），本地双 PS 版本
harness（`release/packaging/test-release-workflow.ps1`）已 PASS。

### 共同前置（缺一不可）

1. **推送本地提交**：本地 17 个提交未推送（推送属仓库所有者决定）；此前演练失败的原因之一
   就是远端 HEAD 缺少这些修复（见下文演练记录）。
2. **两个签名 secrets 未配置**（`.github/workflows/release.yml` build-windows/sign job 真实消费）：
   - `RELEASE_MANIFEST_KEY_ID`（如 `prod-ed25519-1`）
   - `RELEASE_MANIFEST_PRIVATE_KEY`（生产 Ed25519 私钥；公钥需同步进仓库信任库/发行包）
   - 缺失时 build/sign job 必然失败——这是 REL-004 的第一硬阻塞。私钥只允许存在于
     GitHub Release environment，禁止进仓库（现有 `release/keys/` 仅 dev 公钥与 dev 轮换钥）。
3. **environment 配置**：workflow 使用两个 environment——`release-sign`（签名 job）与
   `release`（smoke 后发布的手动批准门）。`release` 需要配置评审人，否则「smoke 通过后
   人工批准发布」环节无人可批，REL-004 第二条（先 draft、全部 smoke 后才发布）无法完成闭环。
4. **本机 API 凭据缺失**：`gh` CLI 已安装（2.97.0）但未登录、环境无 `GITHUB_TOKEN`——
   QA-008 的 Release 重新下载、draft 清理等均无法执行（`tools/verify-external-release.ps1`
   已就绪且 fail closed，无凭据时明确非零退出，不会误报通过）。
5. **本机 docker 未登录 ghcr.io**：QA-008 的 GHCR 重新拉取与 digest/OCI label 校验无法在
   本机执行（若发布包设为 public，匿名拉取可行，但 attestation 校验仍建议登录）。

### 受阻条目明细

| 计划出处 | 条目 | 阻塞原因（具体） |
|---|---|---|
| 更新计划 §17.5 | REL-004：tag 触发 Release workflow，先创建 draft | 依赖共同前置 1/2/3。首支演练 tag `v0.2.0-rc-drill1` 已真实触发 run 33416440024 并在 verify 步骤失败（当时远端 HEAD 缺 `a7e5ef1` 修复），失败形态与清零残留均已留档（[UPDATE_EXTERNAL_QA_EVIDENCE.md](UPDATE_EXTERNAL_QA_EVIDENCE.md) §5），不是脚本未就绪 |
| 更新计划 §17.5 | REL-004：所有资产、验签和 smoke 通过后才发布 | 同上，另需 `release` environment 评审人执行手动批准 |
| 更新计划 §17.5 | REL-005：发布 GHCR semver tag 和 immutable digest | GHCR 推送用内置 `GITHUB_TOKEN`（无需额外凭据），但 job 在 workflow 内位于签名/构建之后——REL-004 通了它才通 |
| 更新计划 §17.5 | REL-006：生成 SBOM、provenance/attestation 并演练 key rotation | SBOM 与 key rotation 的**本地部分已全 PASS**（verify-sbom / augment-sbom / verify-key-rotation / check-immutable-release）；未勾只剩 provenance/SBOM attestation——它们在 workflow 构建阶段生成并绑定真实镜像 digest，依赖 REL-004/005 完成 |
| 更新计划 §17.7 | QA-008：从 GitHub Release 重新下载资产后验签和启动 | 前置 4（gh 凭据）+ 一个真实存在的 Release（依赖 REL-004） |
| 更新计划 §17.7 | QA-008：从 GHCR 重新拉取镜像校验版本/commit/digest/OCI label | 前置 4/5 + REL-005 完成后才有可回拉的 immutable digest |
| 更新计划 §17.7 | 使用临时 tag/draft 完成完整发布演练 | 首支演练已执行但**未走完**（失败于校验步），「完整」需共同前置配齐后复演练至 draft→smoke→发布批准闭环 |
| 更新计划 §17.7 | Release 资产包含 manifest/signature/SHA256SUMS/SBOM/NOTICE/发布说明 | 指真实 GitHub Release 的资产集合；本地 full ZIP 内同集合已验证（REL-002/003），等正式 Release |
| 更新计划 §17.7 | Release environment 私钥权限和 key rotation runbook 复核 | 需要仓库 environment/secrets 的管理员配置权限做复核；本地侧 dev 双钥（dev-ed25519-1/2）轮换演练已 PASS（`f389588`） |
| 更新计划 §17.7 | 批次 5 合流门（§16 DoD 逐条有证据 + 发布负责人签核 stable） | 聚合项：本节全部 + 第二节 clean VM + 人工签核动作 |

### 配齐后的一次性跑通顺序

1. 推送本地 main → 确认远端 CI 全绿。
2. 配置 `RELEASE_MANIFEST_KEY_ID` / `RELEASE_MANIFEST_PRIVATE_KEY`（release-sign）与 `release` environment 评审人。
3. 本机 `gh auth login`（或注入 `GITHUB_TOKEN`）、`docker login ghcr.io`。
4. 打 rc tag（如 `v0.2.0-rc2`）复演练：watch workflow → draft 生成 → `tools/verify-external-release.ps1` 全量 → 批准发布（演练发布物随后删除）。
5. 按结果勾选 REL-004/005/006、QA-008、演练、Release 资产各条。

---

## 二、物理环境阻塞

| 计划出处 | 条目 | 阻塞原因 |
|---|---|---|
| 更新计划 §17.7 | QA-005：Windows 10 x64 clean VM 完整测试 | 本机为 Win11 日常开发机；无可用的 Win10 clean VM（无虚拟化镜像/环境提供）。host-only 结果一律不冒充 clean VM |
| 更新计划 §17.7 | QA-005：Windows 11 x64 clean VM 完整测试 | 同上：本机虽是 Win11 x64 但非 clean VM（有开发工具链、杀软、既存数据） |
| 更新计划 §17.7 | Windows 重启、用户注销、launcher 强杀后 journal 恢复 | **强杀部分已实测 PASS**（snapshotting 注入 `taskkill /F` → journal 恢复 → 旧版本拉起，见 UPDATE_WINDOWS_QA_EVIDENCE.md 场景 3）；未勾只剩真实「重启/注销」——需要物理重启宿主机，会终止本会话，Agent 不可自主动作，需操作者在 clean VM/物理机上执行 |
| 更新计划 §17.7 | （杀毒相关已在 2026-09-01 勾选，此处无条目） | 真实杀毒引擎仍属可选增强：现有勾选基于 FileShare.None 独占句柄行为模拟，若需真实 AV（Defender/第三方）扫描时序验证，同样需要专用环境 |
| 更新计划 §17.6/基线计划 §10.5 | 容器内 WebRTC UDP 媒体面（基线计划「Docker readiness、时区和 WebRTC UDP 验证通过」仅剩此子项） | readiness/时区已实测通过（API+浏览器，UPDATE_DOCKER_E2E_EVIDENCE.md）；UDP 媒体面需要容器内 server 与真机端到端：USB 真机无法透传容器，adb 无线调试需在设备上人工配对（未擅自改动设备网络配置）。需提供 rootless 不解决的 USB 透传环境或人工配对无线调试 |

---

## 三、历史过程审计项（不可追溯，维持注记）

| 计划出处 | 条目 | 不处理原因 |
|---|---|---|
| 基线计划 §10.3 | 已按 C → D → B → A 顺序合并 | 实际父链 A→D→C→B，是已发生的合并历史（2026-08-31 审计注记在案）。四条支线最终集成都通过门禁，重写合并历史（rebase/reorder 已合并的破坏性提交）风险远大于收益，且会推翻既有 BREAKING CHANGE 提交链 |
| 基线计划 §10.3 | 每合并一条支线均单独运行了对应测试 | 同一历史事实：当时未留下逐支线单独测试记录，仅有集成门禁记录；无法事后补造证据，如实保持未勾 |
| 基线计划 §10.5 | 已按 E → F → G → H 顺序合并 | 实际父链 E→H→G→F→集成（2026-08-31 审计注记在案），理由同上 |

这三项是**过程合规审计**，不影响「仓库只存在一套当前契约」的完成定义；对应能力均已由后续全量门禁与实测覆盖。

---

## 四、发布后观察项（更新计划 §17.8，全部 6 条）

阻塞原因单一：**stable 尚未发布**。这些条目按定义只能在正式发布后执行——干净环境重装正式
full ZIP、真实更新闭环检查、失败率/回滚率观察、保留策略复核、lite/delta/新平台决策、
计划顶部状态收口。前置即第一节全部完成。

---

## 五、仓库内可自行完成的收尾项（无外部阻塞）

### 5.1 未勾 checklist 项

| 计划出处 | 条目 | 现状与缺口 |
|---|---|---|
| 更新计划 §17.2（批次 2） | LCH-008：用 version、boot id、schema 和 readiness 验证目标进程 | launcher 对受管/候选进程的身份校验**逻辑与场景测试（mock）已具备**（LCH-012 已勾），但真实链路的身份探针未携带 `X-Admin-Token`：`/api/system/info` 匿名 401 后回退 `/health/ready` body（无 app_version/boot_id 字段），版本/boot_id/schema 比对被跳过直接 commit（UPDATE_M2_EVIDENCE.md §E-6 #5）。接线点：launcher 候选/受管身份校验的 HTTP 探测统一附带 `state/admin-token` 派生的 `X-Admin-Token`（令牌注入链路已在 `24039a1` 前后的修复中就绪）。接线后补一条真实进程回归即可勾选 |

### 5.2 已留档的非阻塞观察项（不在 checklist，属设计取舍/低影响）

1. **install API 一键接管**（UPDATE_M2_EVIDENCE.md §E-6 #4）：server `POST /api/system/update/install`
   202 后经 IPC 只到 `prepare_install`（驻留 staged）；drain→snapshot→switch→commit 编排仅有
   launcher CLI 入口，且安装锁与 `start` 互斥。属批次 3 的设计取舍（CLI=手动语义、API=「可安装」），
   若要 API 一键升级需先解决 IPC 线程执行 `run_full` 的锁与所有权设计。
2. **CLI 接管候选不继承 GAMER_ADMIN_PASSWORD**（UPDATE_REALDEVICE_EVIDENCE.md 缺陷 #2）：CLI 环境
   通常无该变量 → 升级后候选认证 fail closed，需受管重启恢复管理面。生产部署以 `config.toml`
   的 Argon2id `password_hash` 认证，不受影响；仅影响依赖环境变量密码的 E2E 台架便捷性。
3. **cron 冻结窗口前置判断**（更新计划 §6.5）：CLI 手动升级路径的实际语义是「drain 期触发点拒绝 +
   新版启动后按窗口内最近触发点补跑」（真机实测触发点零丢失）；「距下次 cron 大于冻结窗口才安装」
   的前置策略判断由服务端 auto 策略（SYS-005，已勾）承担。两套语义边界已留档，无缺陷。

---

## 附：本清单生成依据

- 更新计划 checklist 现存未勾项核对（2026-09-01）：§17.2 LCH-008×1、§17.5 REL×4、
  §17.7 QA-005×2/重启注销×1/QA-008×2/演练×1/资产×1/environment×1/合流门×1、§17.8×6。
- 基线计划现存未勾项：§10.3×2、§10.5×2。
- 凭据/secrets 名称取自 `.github/workflows/release.yml`（`RELEASE_MANIFEST_KEY_ID`、
  `RELEASE_MANIFEST_PRIVATE_KEY`、environments `release-sign`/`release`）。
- 演练与外部链路证据：[UPDATE_EXTERNAL_QA_EVIDENCE.md](UPDATE_EXTERNAL_QA_EVIDENCE.md)；
  Docker/真机/Windows QA 证据：`UPDATE_DOCKER_E2E_EVIDENCE.md`、`UPDATE_REALDEVICE_EVIDENCE.md`、
  `UPDATE_WINDOWS_QA_EVIDENCE.md`、`UPDATE_M2_EVIDENCE.md`、`CLEAN_BASELINE_FUNC_EVIDENCE.md`。
