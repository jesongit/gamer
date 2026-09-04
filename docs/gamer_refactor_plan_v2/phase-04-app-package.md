# Phase 4：App Package、ResourceResolver 与零业务资源发行

## 目标

让 Gamer 默认发行包不再附带任何具体应用模板、脚本、Keymap YAML。所有应用相关内容通过 App Package 按需安装。

---

## 1. App Package 定义

建议扩展名：

```text
.gamerpkg
```

本质为安全校验后的归档文件：

```text
manifest.toml
templates/
scripts/
keymaps/
presets/
resources/
```

---

## 2. AppPackageId 与 Android package 分离

不要：

```text
AppPackageId == com.xxx.game
```

建议：

```text
AppPackageId = official.xxx
```

manifest：

```toml
[android]
packages = [
  "com.xxx.cn",
  "com.xxx.global"
]
```

第一版限制：

> 一个 Android package 只允许一个 active primary App Package。

---

## 3. App Package immutable

安装后视为只读：

```text
app-packages/
└── official.xxx/
    └── 1.2.0/
```

用户修改单独存：

```text
user-overrides/
└── <android-package>/
```

资源解析顺序：

```text
User Override
    ↓
Installed App Package
```

---

## 4. ResourceResolver

统一禁止：

```text
scripts 模块自己拼目录
keymap 模块自己拼目录
matcher 自己找模板路径
```

所有资源访问：

```text
AppContext
   ↓
ResourceResolver
   ↓
ResourceId
```

---

## 5. Resource revision

ResourceId 应至少与：

- AppPackageId
- package version / revision
- resource logical path

绑定。

目标：

```text
immutable package
→ stable cache key
```

简化模板缓存失效。

---

## 6. Package 安装流程

```text
download/upload
    ↓
size limit
    ↓
archive traversal validation
    ↓
manifest validation
    ↓
hash/signature
    ↓
staging
    ↓
atomic install
```

---

## 7. Package 更新

更新：

- 不修改 user override
- 不修改用户 Task
- 不删除历史日志
- 保留旧版本直到新版本验证通过

可选保留最近一个旧版本用于 rollback。

---

## 8. App version compatibility

manifest 建议支持：

```toml
[[android.targets]]
package = "com.xxx.game"
tested_version_code = 1234
min_version_code = 1200
max_version_code = 1299
```

超出验证范围时 UI 提示风险。

---

## 9. Registry 第一版

不需要独立服务。

可以使用：

```text
GitHub Pages / Release
+
registry.json
```

客户端按 Android package name 查询可用支持包。

---

## 10. 未安装 App Package 时

仍必须可用：

- 投屏
- 基础点击
- 原始键盘输入
- 启动 / 停止 App
- Screenshot
- 日志

不可用或为空：

- 应用模板
- App-specific script
- App-specific keymap
- task preset

---

## 验收标准

全新安装：

```text
templates = 0
scripts = 0
keymaps = 0
```

连接已支持应用：

```text
识别 package
→ 提示可安装支持包
→ 安装
→ 资源出现
```

卸载支持包：

```text
基础投屏与设备能力不受影响
```

用户 Override 不被更新覆盖。

---

## Gate A

Phase 4 完成后进行一次架构与性能验收。

如果：

- ResourceId 模型不稳定
- AppContext 仍然混乱
- 默认零资源无法成立
- 性能明显退化

先修正，不进入 WASM。

---

## V2 收口记录（2026-09-04）

- **Manifest V2**：`format_version = 2` 必填（旧归档安装明确拒绝）；新增 `functions/` 资源根（六目录统一为 `scripts/functions/templates/keymaps/presets/resources/`），Script/Function 索引彻底分离——包内 `scripts/` 只进脚本索引、`functions/` 只进函数索引，同名不混入。
- **EditableLocal 一等源**：资源解析优先级定为 **EditableLocal（本地分区目录 `data/<android>/`）> UserOverride > InstalledPackage**（`app_packages/composite.rs`），覆盖模板、按键映射与脚本/函数库运行快照（`RunSnapshot::capture` 三层合并）；分区目录不再是「兜底层」而是本地编辑区，engine 运行快照经 composite 缝取源。
- **导出/编辑 API**：Rust `PackageBuilder` 正式落地 `.gamerpkg` 导出（`POST /api/app-packages/export`：load_metadata → validate_source → collect → manifest → zip → verify，可复现打包：条目排序 + 固定 mtime + 无额外字段）；工作区元数据 `package.toml` 经 `GET|PUT /api/workspace/:android_package` 读写；已装包一键提取回编辑区（`POST /api/app-packages/:id/:version/edit`，staging + Preflight + 原子替换 + 失败回滚，preflight 以 staging 目录自身内容为最高优先引用视图，保证「提取到空工作区」可过校验）。
- **快照 ZIP 退役**：分区快照 ZIP 导入导出（前后端 `/api/scripts|keymaps/import|export` 与 `scripts.rs` zip 快照链路）已整体移除，打包/迁移统一走 App Package 导出/安装/编辑三入口。
- **PowerShell 打包删除**：`tools/export-app-package.ps1` 已删；打包能力收口在 Rust `PackageBuilder`（`app_packages/builder.rs`），不再依赖脚本侧工具。
- **验收测试指针**：全生命周期 E2E 见 `server/src/api/tests/app_packages_lifecycle.rs`（工作区初始化 → 脚本/函数/模板/keymap 创建 → 运行 → 导出 → 删本地 → 安装激活 → 包层解析 → 编辑提取 → 本地改脚本/函数 → EditableLocal 胜出 → 1.0.1 重发布 → 多版本共存/激活切换 → 同版本重装 409）；导出/工作区/edit/composite 专项见 `app_packages_export.rs` / `app_packages_edit.rs` 与 `app_packages/tests/{builder,workspace,edit,composite,resolver}.rs`。
- 已知边界：`POST /api/scripts/:id/run` 的脚本存在性前置校验只读本地编辑区（`ScriptStore::get`），纯包内脚本经该入口返回 404；包内资源的运行链路由引擎运行快照（composite）保证，后续如需放开可在 run 端点前置校验改走 composite。
