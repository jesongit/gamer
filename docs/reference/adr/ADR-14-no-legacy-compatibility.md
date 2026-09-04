# ADR-14：不提供 Legacy 兼容

> 位置说明：Phase 11 计划原文路径为 `docs/gamer_refactor_plan_v2/adr/`；2026-09-05 docs 已重组为 reference/guides/plans/evidence 四子目录，ADR 属长期有效的架构决策记录，故定位于 `docs/reference/adr/`。

状态：ACCEPTED（2026-09-05）

## 背景

项目仍处于开发阶段，允许破坏性修改。此前为"渐进式重构"保留的旧 API、旧格式、迁移逻辑与回退路径与最终架构并存，是当前最大的复杂度来源。V2 正式架构不提供 legacy compatibility。

## 决策

**不保留**：

- 旧 Task API（含旧 `/api/tasks` adapter、`script_id` 任务）与旧 `/user-tasks` 路由名
- 旧 YAML v2 格式（`script_v2` / legacy parser / legacy lowering / version fallback 整体删除）
- 旧数据目录布局
- native keymap runtime 及其 legacy fallback（Core 侧 mapping parser 一并删除）
- 旧 ScriptStore API
- 旧 App Package format（`format_version = 1` 直接不支持，无 layout v1 migration）
- 旧 Snapshot UI / legacy page / fallback panel

**不做**：

- 配置自动迁移（`legacy` / `compat` / `fallback` / `migration` 类开关与配置一律不引入，已存在且只为旧架构服务的直接删除）
- 旧 App Package 自动升级
- 为旧版本写 adapter

**附带裁决**：

- PowerShell 不是正式打包链路：不得成为产品主路径，推荐删除避免双入口（`tools/` 下的开发脚本可暂留，文档不再描述其为打包方案）。
- 只测试旧行为的测试一并删除，最终只测试正式行为。

本地开发数据因此失效时：**直接删除并重建**，不做迁移。

## 后果

- 升级到 Phase 11 后版本的用户需按新架构重建本地数据 / 重新打包资产；官方提供的资产迁移路径只有 App Package 导出 / 编辑提取（安装即用，无自动升级）。
- 文档收口（P11.10）后，架构文档只描述"现在是什么"，不再保留"回退 / 兼容 / 临时方案"说明。
- 换取的收益：P11.7 之后不再存在双入口、双格式与版本分支逻辑，Core 代码量与心智负担显著下降。
