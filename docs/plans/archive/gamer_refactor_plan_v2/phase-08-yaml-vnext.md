# Phase 8：YAML vNext 与 WASM Automation

## 目标

不是把当前大 Engine 原样编译成 WASM，而是重新收敛 YAML DSL，使 Core 只保留能力，YAML Extension 负责流程、规则和高级语法糖。

---

## 1. YAML 不属于 Core

最终：

```text
Core
不认识：
find
check
func
YAML AST
ScriptStore
```

Core 只认识 Capability。

---

## 2. DSL 分层

```text
Surface YAML
    ↓
Desugar
    ↓
Small AST
    ↓
Host API
```

---

## 3. 建议基础语法

动作：

```text
tap
swipe
key
text
```

流程：

```text
wait
if
loop
break
call
return
throw
```

数据：

```text
set
```

扩展：

```text
invoke
```

辅助：

```text
log
```

---

## 4. `invoke` 作为扩展逃生口

例如：

```yaml
- invoke:
    capability: vision.ocr
    with:
      region: [0.1, 0.2, 0.5, 0.3]
    save: result
```

未来增加能力时，不必每次扩充 AST enum。

---

## 5. 高级语法糖

保留用户友好语法：

```text
find
check
retry
wait_for
click_when
match_first
color_branch
```

但这些不应是 Core Capability。

它们在 YAML Extension 内部 lower 成 primitive。

---

## 6. `find` 的新定位

用户仍可：

```yaml
- find:
    template: login
    timeout: 10s
    click: true
```

内部：

```text
vision.match
→ if
→ device.tap
→ retry/sleep
```

---

## 7. `match_many` 继续属于 Core

原因：

```text
single frame decode
→ many template matches
```

这是性能能力，不是业务语法。

---

## 8. `func` 合并到 `call`

建议：

```yaml
- call:
    target: common/check_login
    save: result

- if:
    cond: $result
```

而不是维护特殊 `func -> bool -> then/else`。

---

## 9. Return Value 泛化

至少支持：

```text
null
bool
int
float
string
duration
coordinate
list
map/record
handle
```

否则 OCR / Vision / Plugin capability 很快受限。

---

## 10. 不把 YAML 变成通用编程语言

避免继续加入：

- 复杂数学 DSL
- lambda
- 大量字符串函数
- collection pipeline
- 自定义类型系统
- 通用表达式编程语言

复杂逻辑直接写 WASM Extension。

---

## 11. App 操作语义泛化

旧：

```text
str_app
cls_app
```

新建议：

```text
app.start
app.stop
```

默认使用当前 `AppContext.android_package`。

操作其他 package 需要额外权限。

---

## 12. 兼容旧 YAML

建议：

```text
YAML v2
YAML v3
```

并存一个迁移周期。

新脚本：

```yaml
version: 3
```

旧脚本通过 Compatibility Adapter 运行。

---

## 13. 前端自动化 Panel

YAML Extension 可以贡献：

```text
自动化
函数
```

等多个 Panel。

复杂编辑器使用 iframe。

Panel 通过 Bridge 请求：

- 当前 AppContext
- 左侧区域框选
- plugin.call
- toast/dialog

---

## 验收标准

- Core 中不存在 YAML-specific 类型
- 旧 YAML 兼容测试通过
- v3 DSL 明显小于现有复杂 AST
- find/check 等作为语法糖仍保持易用
- YAML Extension 可动态安装/卸载
- 卸载 YAML 后基础投屏不受影响
