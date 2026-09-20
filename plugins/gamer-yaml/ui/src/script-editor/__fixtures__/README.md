# YAML v3 编辑器 fixture

本目录是前端脚本编辑器（YAML v3）的往返测试 fixture：
- `yaml/v3_*.yaml`：合法 v3 样例，**内容即 `codec.serialize` 的规范输出形态**
  （`fixtures.test.js` 断言 serialize(parse(fixture)) 逐字节一致 + 模型幂等）；
- `yaml/v2_rejected.yaml`：v2 样例，断言被明确拒绝（`yaml.v3.version` 诊断 + 空壳模型，
  不崩溃不误解析，见 ADR-YAML-01）。

语法契约：`docs/plans/phase12_v3_dsl_contract.md`；语义裁决：
`docs/reference/adr/ADR-YAML-01~04`。v2 契约 fixture（原 script_v2 只读副本）
已随 v2 codec 一并删除。
