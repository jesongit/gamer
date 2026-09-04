# script_v2 契约 fixture 前端副本

本目录是 `server/tests/fixtures/script_v2/` 的**只读副本**，供前端契约断言与
编辑器 codec 测试使用。契约正文见 `docs/reference/SCRIPT_EDITOR_CONTRACT.md`，
样例索引见 `server/tests/fixtures/script_v2/README.md`。

## 两目录映射

| 服务端源 | 本目录副本 | 说明 |
|---|---|---|
| `server/tests/fixtures/script_v2/<ID>.yaml` | `yaml/<ID>.yaml` | 逻辑 ID 相同；**内容逐字节一致**（漂移由 `fixtures.test.js` 的逐字节比对测试强制，改任一侧必须同步另一侧） |
| `server/tests/fixtures/script_v2/<ID>.golden.json` | `json/<ID>.golden.json` | 合法样例期望（当前前端 Model 字段名） |
| `server/tests/fixtures/script_v2/<ID>.expected.json` | `json/<ID>.expected.json` | 非法样例期望（code + step_path + field） |
| `server/tests/fixtures/script_v2/README.md`、`*.golden/expected.json` 之外的文件 | 不复制 | 服务端数据扫描测试只存在于服务端 |

多文件样例（v09 的 `v09_call_script.target.yaml`、v10 的 `v10_func_call_cross_file.common.yaml`）
与单文件样例同规则：文件名即逻辑 ID 体系的一部分，两目录同名同内容。

## 职责边界

- **服务端**（saphyr-parser 事件层）是权威解析与非法拒绝方，仓库数据示例也必须通过同一严格 loader；
- **前端**（js-yaml）对同一 YAML 断言同一模型形态 + psig1 签名双实现 + 副本漂移守护
  （`fixtures.test.js`）；
- 两目录逐字节一致是「同一份 YAML、前后端读出同一形态」的前提，任何契约改动必须
  同步：服务端 fixture、golden/expected JSON、本目录副本、`docs/reference/SCRIPT_EDITOR_CONTRACT.md`。
