//! Pure YAML value helpers used by the script engine.
//!
//! These helpers deliberately do not know about devices, files, or WebRTC.
//! Keeping substitution and scalar parsing here makes the semantic rules easy
//! to test without constructing a `Runner` or an Android session.

use serde_yaml::Value;

/// Split space-separated arguments while preserving spaces inside `[...]`.
pub(super) fn split_args(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut depth = 0usize;
    for ch in line.chars() {
        match ch {
            '[' => {
                depth += 1;
                cur.push(ch);
            }
            ']' => {
                depth = depth.saturating_sub(1);
                cur.push(ch);
            }
            c if c.is_whitespace() && depth == 0 => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Replace `$N` recursively in YAML strings, including mapping keys.
pub(super) fn substitute_args(v: &mut Value, args: &[String]) -> anyhow::Result<()> {
    match v {
        Value::String(s) => *s = substitute_str(s, args)?,
        Value::Sequence(seq) => {
            for item in seq.iter_mut() {
                substitute_args(item, args)?;
            }
        }
        Value::Mapping(m) => {
            // Mapping keys are immutable during iteration, so rebuild the map.
            let old = std::mem::take(m);
            for (mut k, mut val) in old {
                substitute_args(&mut k, args)?;
                substitute_args(&mut val, args)?;
                m.insert(k, val);
            }
        }
        _ => {}
    }
    Ok(())
}

/// Replace `$N` in one string without recursively expanding replacement text.
pub(super) fn substitute_str(s: &str, args: &[String]) -> anyhow::Result<String> {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(pos) = rest.find('$') {
        out.push_str(&rest[..pos]);
        let after = &rest[pos + 1..];
        let digits: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
        if digits.is_empty() {
            out.push('$');
            rest = after;
            continue;
        }
        let n: usize = digits.parse().unwrap();
        let Some(arg) = args.get(n.checked_sub(1).unwrap_or(usize::MAX)) else {
            anyhow::bail!(
                "参数引用 ${} 超出实参数量（{} 个）：含 $N 占位的脚本需经 call/函数调用传参运行（参数从 $1 开始）",
                digits,
                args.len()
            );
        };
        out.push_str(arg);
        rest = &after[digits.len()..];
    }
    out.push_str(rest);
    Ok(out)
}

/// Replace `^N` recursively while leaving mapping-valued sequence items intact.
pub(super) fn substitute_refs(v: &Value, refs: &[String]) -> anyhow::Result<Value> {
    match v {
        Value::String(s) => Ok(Value::String(substitute_ref_str(s, refs)?)),
        Value::Sequence(seq) => {
            let mut out = Vec::with_capacity(seq.len());
            for item in seq {
                out.push(match item {
                    Value::String(s) => Value::String(substitute_ref_str(s, refs)?),
                    other => other.clone(),
                });
            }
            Ok(Value::Sequence(out))
        }
        Value::Mapping(m) => {
            let mut out = serde_yaml::Mapping::new();
            for (k, val) in m {
                let nk = match k {
                    Value::String(s) => Value::String(substitute_ref_str(s, refs)?),
                    other => other.clone(),
                };
                let nv = match val {
                    Value::String(s) => Value::String(substitute_ref_str(s, refs)?),
                    Value::Sequence(seq) => {
                        let mut ns = Vec::with_capacity(seq.len());
                        for item in seq {
                            ns.push(match item {
                                Value::String(s) => Value::String(substitute_ref_str(s, refs)?),
                                other => other.clone(),
                            });
                        }
                        Value::Sequence(ns)
                    }
                    Value::Mapping(_) => substitute_refs(val, refs)?,
                    other => other.clone(),
                };
                out.insert(nk, nv);
            }
            Ok(Value::Mapping(out))
        }
        other => Ok(other.clone()),
    }
}

/// Replace `^N` in one string; non-numeric carets remain literal.
pub(super) fn substitute_ref_str(s: &str, refs: &[String]) -> anyhow::Result<String> {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(pos) = rest.find('^') {
        out.push_str(&rest[..pos]);
        let after = &rest[pos + 1..];
        let digits: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
        if digits.is_empty() {
            out.push('^');
            rest = after;
            continue;
        }
        let n: usize = digits.parse().unwrap();
        let Some(reference) = refs.get(n.checked_sub(1).unwrap_or(usize::MAX)) else {
            anyhow::bail!(
                "上下文引用 ^{} 超出数量（{} 个：^1 主模板/坐标，^2.. 障碍模板/颜色）",
                digits,
                refs.len()
            );
        };
        out.push_str(reference);
        rest = &after[digits.len()..];
    }
    out.push_str(rest);
    Ok(out)
}

/// Parse a duration with an explicit unit: ms, s, min/m, h, or d.
pub(super) fn parse_duration(v: &Value, opt: &str) -> anyhow::Result<u64> {
    let Some(s) = v.as_str() else {
        anyhow::bail!(
            "{} 需要带单位时长（如 500ms / 2s / 1m / 30min / 1h / 1d）；裸数字不再接受，收到: {:?}",
            opt,
            v
        );
    };
    let t = s.trim().to_ascii_lowercase();
    for (suffix, mult) in [
        ("ms", 1.0f64),
        ("min", 60_000.0),
        ("m", 60_000.0),
        ("s", 1_000.0),
        ("h", 3_600_000.0),
        ("d", 86_400_000.0),
    ] {
        if let Some(num) = t.strip_suffix(suffix) {
            if let Ok(val) = num.trim().parse::<f64>() {
                if val >= 0.0 {
                    return Ok((val * mult).round() as u64);
                }
            }
        }
    }
    anyhow::bail!(
        "{} 需要带单位时长（如 500ms / 2s / 1m / 30min / 1h / 1d），收到: {}",
        opt,
        s
    )
}

pub(super) fn opt_duration(step: &Value, opt: &str) -> anyhow::Result<Option<u64>> {
    match step.get(opt) {
        Some(v) => parse_duration(v, opt).map(Some),
        None => Ok(None),
    }
}

pub(super) fn steps_value(v: &Value) -> anyhow::Result<Vec<Value>> {
    match v {
        Value::Null => Ok(Vec::new()),
        Value::Sequence(seq) => Ok(seq.clone()),
        _ => anyhow::bail!("需要步骤列表（- 键: 换行缩进步骤）或留空"),
    }
}

/// Parse a color as RRGGBB text, integer, or a three-component array.
pub(super) fn parse_color(v: &Value) -> anyhow::Result<(u8, u8, u8)> {
    if let Some(s) = v.as_str() {
        let t = s
            .trim()
            .trim_start_matches('#')
            .trim_start_matches("0x")
            .to_ascii_lowercase();
        if t.len() == 6 && t.chars().all(|c| c.is_ascii_hexdigit()) {
            return Ok((
                u8::from_str_radix(&t[0..2], 16).unwrap(),
                u8::from_str_radix(&t[2..4], 16).unwrap(),
                u8::from_str_radix(&t[4..6], 16).unwrap(),
            ));
        }
        anyhow::bail!(
            "色值需要 6 位十六进制（如 ff8800 或 \"#ff8800\"）或 [r, g, b]，收到: {}",
            s
        );
    }
    if let Some(n) = v.as_u64() {
        if n <= 0xFF_FFFF {
            return Ok(((n >> 16) as u8, (n >> 8) as u8, n as u8));
        }
    }
    if let Some(seq) = v.as_sequence() {
        if seq.len() == 3 {
            let c = seq
                .iter()
                .map(|x| {
                    let n = x
                        .as_u64()
                        .or_else(|| x.as_str().and_then(|s| s.trim().parse::<u64>().ok()))
                        .ok_or_else(|| anyhow::anyhow!("色值数组需要 [r, g, b] 数字（0~255）"))?;
                    if n > 255 {
                        anyhow::bail!("色值分量必须在 0~255，收到: {}", n);
                    }
                    Ok(n as u8)
                })
                .collect::<anyhow::Result<Vec<u8>>>()?;
            return Ok((c[0], c[1], c[2]));
        }
    }
    anyhow::bail!(
        "色值只支持 6 位十六进制（ff8800）或 [r, g, b] 数组，收到: {:?}",
        v
    )
}

pub(super) fn parse_rel_coord(v: &Value) -> anyhow::Result<(f64, f64)> {
    let seq = v
        .as_sequence()
        .ok_or_else(|| anyhow::anyhow!("需要 [x, y] 数组（相对坐标 0~1）"))?;
    if seq.len() != 2 {
        anyhow::bail!("需要 [x, y] 2 个相对坐标");
    }
    let x = seq[0]
        .as_f64()
        .ok_or_else(|| anyhow::anyhow!("坐标必须是数字"))?;
    let y = seq[1]
        .as_f64()
        .ok_or_else(|| anyhow::anyhow!("坐标必须是数字"))?;
    if !(0.0..=1.0).contains(&x) || !(0.0..=1.0).contains(&y) {
        anyhow::bail!("相对坐标必须在 0~1 之间");
    }
    Ok((x, y))
}

pub(super) fn relative_pair(v: &Value) -> anyhow::Result<(f32, f32)> {
    if let Some(seq) = v.as_sequence() {
        let x = seq.first().and_then(|x| x.as_f64()).unwrap_or(0.0) as f32;
        let y = seq.get(1).and_then(|x| x.as_f64()).unwrap_or(0.0) as f32;
        return Ok((x.clamp(0.0, 1.0), y.clamp(0.0, 1.0)));
    }
    if v.is_mapping() {
        let x = v.get("x").and_then(|x| x.as_f64()).unwrap_or(0.0) as f32;
        let y = v.get("y").and_then(|x| x.as_f64()).unwrap_or(0.0) as f32;
        return Ok((x.clamp(0.0, 1.0), y.clamp(0.0, 1.0)));
    }
    anyhow::bail!("相对坐标需要 [x, y] 或 {{x, y}}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn substitutions_cover_nested_values_and_mapping_keys() {
        let mut value: Value =
            serde_yaml::from_str("steps:\n  - color: [$1, 0.5]\n    $2:\n      - log: ^1/$1\n")
                .unwrap();
        substitute_args(&mut value, &["0.25".into(), "ff8800".into()]).unwrap();
        assert_eq!(value["steps"][0]["color"][0], "0.25");
        assert_eq!(value["steps"][0]["ff8800"][0]["log"], "^1/0.25");

        let refs = vec!["main.png".into(), "block.png".into()];
        assert_eq!(
            substitute_ref_str("^1 ^2 ^x", &refs).unwrap(),
            "main.png block.png ^x"
        );
    }

    #[test]
    fn scalar_helpers_keep_units_and_argument_boundaries() {
        assert_eq!(
            split_args("f1 [0.5, 0.6] ff8800"),
            ["f1", "[0.5, 0.6]", "ff8800"]
        );
        assert_eq!(parse_duration(&Value::from("1.5s"), "wait").unwrap(), 1500);
        assert!(parse_duration(&Value::from(500), "wait").is_err());
        assert_eq!(parse_color(&Value::from("#ff8800")).unwrap(), (255, 136, 0));
        assert_eq!(
            parse_rel_coord(&serde_yaml::from_str("[0.25, 0.5]").unwrap()).unwrap(),
            (0.25, 0.5)
        );
    }
}
