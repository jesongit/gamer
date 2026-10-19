use anyhow::{ensure, Context, Result};
use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct TextBox {
    pub text: String,
    pub confidence: f64,
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Serialize)]
pub struct Recognition {
    pub text: String,
    pub confidence: f64,
    pub lines: Vec<TextBox>,
    pub words: Vec<TextBox>,
    pub width: u32,
    pub height: u32,
}

fn joins_without_space(left: &str, right: &str) -> bool {
    fn cjk(c: char) -> bool {
        matches!(c as u32, 0x2E80..=0x9FFF | 0xF900..=0xFAFF | 0x20000..=0x3134F)
    }
    left.chars().last().is_some_and(cjk) && right.chars().next().is_some_and(cjk)
}

pub(super) fn parse_tsv(
    bytes: &[u8],
    width: u32,
    height: u32,
    scaled_width: u32,
    scaled_height: u32,
) -> Result<Recognition> {
    ensure!(
        width > 0 && height > 0 && scaled_width > 0 && scaled_height > 0,
        "OCR 图像尺寸无效"
    );
    let source = std::str::from_utf8(bytes).context("OCR 输出不是 UTF-8")?;
    let mut rows = source.lines();
    let header = rows
        .next()
        .unwrap_or_default()
        .trim_start_matches('\u{feff}');
    ensure!(header == "level\tpage_num\tblock_num\tpar_num\tline_num\tword_num\tleft\ttop\twidth\theight\tconf\ttext", "OCR 输出格式无效");
    let mut words = Vec::new();
    let mut groups: BTreeMap<(u32, u32, u32, u32), Vec<usize>> = BTreeMap::new();
    for row in rows {
        if row.trim().is_empty() {
            continue;
        }
        let columns: Vec<_> = row.splitn(12, '\t').collect();
        ensure!(columns.len() == 12, "OCR 输出行格式无效");
        let number = |i: usize| columns[i].parse::<u32>().context("OCR 位置无效");
        if number(0)? != 5 {
            continue;
        }
        let text = columns[11].trim();
        if text.is_empty() {
            continue;
        }
        ensure!(
            words.len() < 20_000 && text.len() <= 4096,
            "OCR 文字数量过多"
        );
        let confidence = columns[10].parse::<f64>().context("OCR 置信度无效")?;
        ensure!(
            confidence.is_finite() && (-1.0..=100.0).contains(&confidence),
            "OCR 置信度无效"
        );
        if confidence < 0.0 {
            continue;
        }
        let sx = number(6)?;
        let sy = number(7)?;
        let sw = number(8)?;
        let sh = number(9)?;
        ensure!(
            sx <= scaled_width && sy <= scaled_height && sw <= scaled_width && sh <= scaled_height,
            "OCR 位置超出图像"
        );
        let x = ((sx as f64 * width as f64 / scaled_width as f64).floor() as u32).min(width);
        let y = ((sy as f64 * height as f64 / scaled_height as f64).floor() as u32).min(height);
        let right = (((sx as u64 + sw as u64) as f64 * width as f64 / scaled_width as f64).ceil()
            as u32)
            .min(width);
        let bottom =
            (((sy as u64 + sh as u64) as f64 * height as f64 / scaled_height as f64).ceil() as u32)
                .min(height);
        if right <= x || bottom <= y {
            continue;
        }
        groups
            .entry((number(1)?, number(2)?, number(3)?, number(4)?))
            .or_default()
            .push(words.len());
        words.push(TextBox {
            text: text.into(),
            confidence: confidence / 100.0,
            x,
            y,
            width: right - x,
            height: bottom - y,
        });
    }
    let mut lines = Vec::new();
    for indexes in groups.into_values() {
        let mut line = words[indexes[0]].clone();
        let mut weight = line.text.chars().count().max(1) as f64;
        let mut confidence_sum = line.confidence * weight;
        for index in indexes.into_iter().skip(1) {
            let word = &words[index];
            if !joins_without_space(&line.text, &word.text) {
                line.text.push(' ');
            }
            line.text.push_str(&word.text);
            let right = (line.x + line.width).max(word.x + word.width);
            let bottom = (line.y + line.height).max(word.y + word.height);
            line.x = line.x.min(word.x);
            line.y = line.y.min(word.y);
            line.width = right - line.x;
            line.height = bottom - line.y;
            let word_weight = word.text.chars().count().max(1) as f64;
            confidence_sum += word.confidence * word_weight;
            weight += word_weight;
        }
        line.confidence = confidence_sum / weight;
        lines.push(line);
    }
    let text = lines
        .iter()
        .map(|line| line.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    let weight: usize = words
        .iter()
        .map(|word| word.text.chars().count().max(1))
        .sum();
    let confidence = if weight == 0 {
        0.0
    } else {
        words
            .iter()
            .map(|word| word.confidence * word.text.chars().count().max(1) as f64)
            .sum::<f64>()
            / weight as f64
    };
    Ok(Recognition {
        text,
        confidence,
        lines,
        words,
        width,
        height,
    })
}
