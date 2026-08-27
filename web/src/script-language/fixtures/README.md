# 脚本语言 fixtures —— Rust 与前端共用的以例清单

本目录是脚本校验 / 行映射测试的**稳定副本**：由 `server/data/com.miHoYo.hkrpg/yaml/` 下
真实业务 YAML 改编而来，**模板引用全部替换为虚拟稳定名**，使测试永不依赖 `server/data`
的存续与中文模板改名。

## 布局

```
fixtures/
├─ templates.txt      # 虚拟模板清单（规范来源）：每行一个"磁盘文件名"
└─ scripts/           # 改编后的业务脚本副本
   ├─ lib_utils.yaml  ← server/data/com.miHoYo.hkrpg/yaml/utils.yml（函数库，列表形式 func）
   ├─ flow_daily.yaml ← 日常遗器.yml（跨文件调用 utils:xxx → lib_utils:xxx）
   ├─ common_account.yaml ← 通用日常.yml（$1 实参 + call 子脚本）
   ├─ multi_account.yaml  ← 三账号日常.yml
   ├─ mail_only.yaml  ← 领取邮件.yml（原版邮件按钮.png 已在真实数据中消失，改编为存在引用）
   ├─ color_probe.yaml ← 测试.yml（color 取色分支）
   ├─ cn_names.yaml   # （新增）中文模板名：精确全名 / 短名引用各一处
   ├─ fn_lib_short.yaml # （新增）省略 func: 的纯函数库映射简写
   └─ misc.yml        # （新增）缺扩展名自动补全用例的调用目标
```

## 规范约定

- **templates.txt 是权威清单**：每行一个文件名；`#` 后缀 = 区域元数据（半区码
  `#a/u/d/l/r/ul/ur/dl/dr` 或 ×1000 坐标 `#x1_y1_x2_y2`），与引擎
  `engine::tpl_region_from_name`、前端 `parseTplRegion` 同格式。无后缀 = 全屏。
- **短名引用**：脚本写 `tpl_mail_icon.png` 即可唯一匹配 `tpl_mail_icon#*.png`
  （与引擎 `resolve_template_file` 一致）；`tpl_dup#l.png` / `tpl_dup#r.png`
  是专门的**短名歧义对**，供"请用完整文件名"报错用例使用，不被任何合法 fixture 引用。
- 中文名条目（`签到按钮#u.png`、`每日签到#d.png`、`普通界面.png`）是**稳定命名**的一部分，
  覆盖 Unicode 处理路径；它们不随真实数据改名而变。

## 共用契约（Rust 侧待接入）

- 前端当前消费方：`web/src/script-language/*.test.js`（Vitest，node 环境直接读本目录，
  不需要设备 / ffmpeg）。Rust 侧消费将在后续阶段接上：测试读取同一份 `templates.txt`
  与 `scripts/*`，按各自运行时的同名校验规则断言一致判定（合法脚本双端均零错误）。
- 每个合法 fixture 在**双端都必须通过**；错误提示文案允许两端措辞不同，但判定结论
  （通过 / 报错）必须一致。
- 已知分歧（见 validate.test.js 的 TODO 用例，均以引擎为准、前端待修）：
  1. 省略 `func:` 的顶层映射含**多个函数键**时，引擎 `parse_funcs`（映射逐键拆分）
     接受，前端校验尚按"恰好一个函数名键"报错。fixture 采用单函数写法规避。
  2. **跨文件调用**省略 `func:` 的纯函数库时，引擎先对被引用脚本做
     normalize_top 再取 func 段（可调用）；前端直接读原文档 sdoc.func → 误报"未定义函数"。
