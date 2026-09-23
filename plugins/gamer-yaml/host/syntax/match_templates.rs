//! 模板分支的 surface 契约；设备匹配由原生 find_any 执行。
use super::*;

#[derive(Clone, Debug, PartialEq)]
pub struct TemplateCase {
    pub template: SurfaceExpr,
    pub save_as: Option<String>,
    pub body: Vec<SurfaceStep>,
}

fn fields<'a>(
    value: &'a YamlValue,
    allowed: &[&str],
    path: &str,
) -> Result<&'a Mapping, Vec<Diagnostic>> {
    let map = value
        .as_mapping()
        .ok_or_else(|| one_diagnostic("yaml.match_templates.shape", path, "必须是映射"))?;
    if map
        .keys()
        .any(|key| !key.as_str().is_some_and(|key| allowed.contains(&key)))
    {
        return Err(one_diagnostic(
            "yaml.match_templates.shape",
            path,
            format!("仅支持字段 {}", allowed.join("/")),
        ));
    }
    Ok(map)
}

pub(super) fn parse(value: &YamlValue, path: &str) -> Result<SurfaceStep, Vec<Diagnostic>> {
    let map = fields(value, &["cases", "else", "threshold"], path)?;
    let cases = map
        .get("cases")
        .and_then(YamlValue::as_sequence)
        .filter(|v| !v.is_empty() && v.len() <= 64)
        .ok_or_else(|| {
            one_diagnostic(
                "yaml.match_templates.cases",
                path,
                "cases 必须是 1..64 项的分支列表",
            )
        })?;
    let mut parsed = Vec::new();
    for (index, value) in cases.iter().enumerate() {
        let case_path = format!("{path}.cases[{index}]");
        let case = fields(value, &["template", "as", "do"], &case_path)?;
        let template = case.get("template").ok_or_else(|| {
            one_diagnostic("yaml.match_templates.template", path, "分支缺少 template")
        })?;
        let template = expr_from_yaml(template, &format!("{case_path}.template"))?;
        if !matches!(
            &template,
            SurfaceExpr::Ref(_) | SurfaceExpr::Lit(Value::String(_))
        ) || matches!(&template, SurfaceExpr::Lit(Value::String(v)) if v.trim().is_empty())
        {
            return Err(one_diagnostic(
                "yaml.match_templates.template",
                path,
                "template 必须是非空模板名或引用",
            ));
        }
        let save_as = match case.get("as") {
            None => None,
            Some(value) => Some(
                value
                    .as_str()
                    .filter(|name| is_identifier(name))
                    .ok_or_else(|| one_diagnostic("yaml.as.invalid", path, "分支 as 必须是变量名"))?
                    .to_string(),
            ),
        };
        let body = case
            .get("do")
            .ok_or_else(|| one_diagnostic("yaml.match_templates.do", path, "分支缺少 do"))?;
        parsed.push(TemplateCase {
            template,
            save_as,
            body: parse_steps(body, &format!("{case_path}.do"))?,
        });
    }
    let threshold = match map.get("threshold") {
        Some(value) => expr_from_yaml(value, &format!("{path}.threshold"))?,
        None => SurfaceExpr::Lit(json!(0.8)),
    };
    if !matches!(&threshold, SurfaceExpr::Ref(_))
        && !matches!(&threshold, SurfaceExpr::Lit(v) if v.as_f64().is_some_and(|v| (0.0..=1.0).contains(&v)))
    {
        return Err(one_diagnostic(
            "yaml.match_templates.threshold",
            path,
            "threshold 必须为 0..1 数字或引用",
        ));
    }
    let else_steps = match map.get("else") {
        Some(value) => parse_steps(value, &format!("{path}.else"))?,
        None => Vec::new(),
    };
    Ok(SurfaceStep::MatchTemplates {
        cases: parsed,
        threshold,
        else_steps,
    })
}

pub(super) fn wire(
    cases: &[TemplateCase],
    threshold: &SurfaceExpr,
    otherwise: &[SurfaceStep],
    path: &str,
    functions: &FunctionLibrary,
) -> Value {
    json!({
        "op":"match_templates", "path":path, "desc":"模板分支",
        "args": SurfaceExpr::Map(vec![
            ("templates".into(), SurfaceExpr::List(cases.iter().map(|c| c.template.clone()).collect())),
            ("threshold".into(), threshold.clone()),
        ]).to_wire(),
        "cases":cases.iter().enumerate().map(|(i,c)| json!({
            "as":c.save_as, "do":wire_steps(&c.body, &format!("{path}.cases[{i}].do"), functions)
        })).collect::<Vec<_>>(),
        "else":wire_steps(otherwise, &format!("{path}.else"), functions),
    })
}

pub(super) fn yaml_lines(
    cases: &[TemplateCase],
    threshold: &SurfaceExpr,
    otherwise: &[SurfaceStep],
    indent: usize,
    out: &mut Vec<String>,
) {
    let pad = "  ".repeat(indent);
    out.push(format!("{pad}match_templates:"));
    out.push(format!("{pad}  threshold:"));
    expr_yaml_lines(threshold, indent + 2, out);
    out.push(format!("{pad}  cases:"));
    for case in cases {
        out.push(format!("{pad}    - template:"));
        expr_yaml_lines(&case.template, indent + 4, out);
        if let Some(name) = &case.save_as {
            out.push(format!("{pad}      as: {name}"));
        }
        out.push(format!(
            "{pad}      do:{}",
            if case.body.is_empty() { " []" } else { "" }
        ));
        for step in &case.body {
            push_dash_item(step, indent + 4, out);
        }
    }
    if !otherwise.is_empty() {
        out.push(format!("{pad}  else:"));
        for step in otherwise {
            push_dash_item(step, indent + 2, out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SOURCE: &str = "run:\n  - match_templates:\n      cases:\n        - template: old.png\n          as: hit\n          do:\n            - tap: $hit.center\n        - template: $other\n          do: []\n      else:\n        - log: missing\n";

    #[test]
    fn match_templates_roundtrip_wire_calls_and_rename() {
        let script = parse_script(SOURCE).unwrap();
        let serialized = serialize_script(&script);
        assert_eq!(parse_script(&serialized).unwrap().run, script.run);
        let wire = build_program(&script, &Vec::new(), JsonMap::new(), 0);
        assert_eq!(wire["run"][0]["op"], "match_templates");
        assert_eq!(
            wire["run"][0]["cases"][0]["do"][0]["path"],
            "run[0].cases[0].do[0]"
        );
        let mut calls = BTreeSet::new();
        script.run[0].collect(&mut calls, &mut BTreeSet::new());
        assert_eq!(
            calls,
            BTreeSet::from(["find_any".into(), "tap".into(), "log".into()])
        );
        let renamed = rename_template_source(SOURCE, "old.png", "old.png", "new.png", "new.png")
            .unwrap()
            .unwrap()
            .0;
        assert!(renamed.contains("new.png") && renamed.contains("$other"));
        assert!(parse_script(&renamed).is_ok());
        let library = format!(
            "functions:\n  handler:\n{}",
            SOURCE
                .lines()
                .map(|line| format!("    {line}\n"))
                .collect::<String>()
        );
        let library = parse_function_library(&library).unwrap();
        assert!(parse_function_library(&serialize_function_library(&library)).is_ok());
    }

    #[test]
    fn match_templates_rejects_invalid_contracts() {
        for body in [
            "cases: []",
            "cases: [{template: '', do: []}]",
            "cases: [{template: 1, do: []}]",
            "cases: [{template: a}]",
            "cases: [{template: a, as: 12, do: []}]",
            "cases: [{template: a, do: [], extra: 1}]",
            "cases: [{template: a, do: []}]\n      threshold: 2",
        ] {
            assert!(
                parse_script(&format!("run:\n  - match_templates:\n      {body}\n")).is_err(),
                "{body}"
            );
        }
        assert!(parse_function_library("functions:\n  match_templates:\n    run: []\n").is_err());
    }
}
