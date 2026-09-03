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
