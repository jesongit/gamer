//! saphyr-parser 事件 → 节点树（保留标量样式）。
//!
//! 阶段 0 测试支持模块：serde_yaml 0.9 反序列化后丢失标量书写样式，无法校验
//! params「整条单引号」契约，因此用 saphyr-parser 的事件级 API 自建带样式的树。
//! 阶段 2 会把真实严格 AST 装载器迁入 `server/src`（见
//! docs/SCRIPT_EDITOR_CONTRACT.md 第 2 节选型结论），届时删除本文件。

use saphyr_parser::{Event, Parser, ScalarStyle};
use std::fmt;

/// 带标量样式信息的 YAML 节点树。映射保持键序（Vec 承载）。
#[derive(Debug, Clone, PartialEq)]
pub enum Node {
    Scalar { raw: String, style: ScalarStyle },
    Seq(Vec<Node>),
    Map(Vec<(String, Node)>),
}

impl Node {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Node::Scalar { raw, .. } => Some(raw),
            _ => None,
        }
    }

    pub fn as_seq(&self) -> Option<&[Node]> {
        match self {
            Node::Seq(items) => Some(items),
            _ => None,
        }
    }

    pub fn as_map(&self) -> Option<&[(String, Node)]> {
        match self {
            Node::Map(entries) => Some(entries),
            _ => None,
        }
    }

    pub fn scalar_style(&self) -> Option<ScalarStyle> {
        match self {
            Node::Scalar { style, .. } => Some(*style),
            _ => None,
        }
    }

    /// 取映射中指定键的值（首个同名键）。
    pub fn get(&self, key: &str) -> Option<&Node> {
        self.as_map()?
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v)
    }
}

#[derive(Debug)]
pub struct LoadError {
    pub message: String,
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "YAML 解析失败: {}", self.message)
    }
}

enum Frame {
    Seq(Vec<Node>),
    Map {
        entries: Vec<(String, Node)>,
        pending_key: Option<String>,
    },
}

/// 解析单文档 YAML 为节点树；标量样式（Plain/SingleQuoted/DoubleQuoted/Literal/Folded）随节点保留。
pub fn load(source: &str) -> Result<Node, LoadError> {
    let mut stack: Vec<Frame> = Vec::new();
    let mut root: Option<Node> = None;

    let attach = |stack: &mut Vec<Frame>, root: &mut Option<Node>, node: Node| -> Result<(), LoadError> {
        match stack.last_mut() {
            Some(Frame::Seq(items)) => {
                items.push(node);
                Ok(())
            }
            Some(Frame::Map { entries, pending_key }) => {
                let key = pending_key
                    .take()
                    .ok_or_else(|| LoadError { message: "映射值缺少键（复键或结构异常）".into() })?;
                entries.push((key, node));
                Ok(())
            }
            None => {
                if root.is_some() {
                    return Err(LoadError { message: "出现多余文档".into() });
                }
                *root = Some(node);
                Ok(())
            }
        }
    };

    for item in Parser::new_from_str(source) {
        let (ev, _span) = item.map_err(|e| LoadError { message: e.to_string() })?;
        match ev {
            Event::StreamStart | Event::StreamEnd | Event::DocumentStart(_) | Event::Nothing => {}
            Event::DocumentEnd => {}
            Event::Scalar(value, style, _anchor, _tag) => {
                let raw = value.into_owned();
                // 顶部是映射且尚无挂起键 → 该标量是键；否则是值（或根标量）。
                let is_key = matches!(
                    stack.last(),
                    Some(Frame::Map { pending_key: None, .. })
                );
                if is_key {
                    match stack.last_mut() {
                        Some(Frame::Map { pending_key, .. }) => *pending_key = Some(raw),
                        _ => unreachable!(),
                    }
                } else {
                    attach(&mut stack, &mut root, Node::Scalar { raw, style })?;
                }
            }
            Event::SequenceStart(_anchor, _tag) => {
                stack.push(Frame::Seq(Vec::new()));
            }
            Event::SequenceEnd => {
                let frame = stack.pop().ok_or_else(|| LoadError { message: "序列结束不匹配".into() })?;
                let items = match frame {
                    Frame::Seq(items) => items,
                    Frame::Map { .. } => return Err(LoadError { message: "序列结束遇到映射".into() }),
                };
                attach(&mut stack, &mut root, Node::Seq(items))?;
            }
            Event::MappingStart(_anchor, _tag) => {
                stack.push(Frame::Map { entries: Vec::new(), pending_key: None });
            }
            Event::MappingEnd => {
                let frame = stack.pop().ok_or_else(|| LoadError { message: "映射结束不匹配".into() })?;
                let entries = match frame {
                    Frame::Map { entries, pending_key } => {
                        if pending_key.is_some() {
                            return Err(LoadError { message: "映射以悬空键结束".into() });
                        }
                        entries
                    }
                    Frame::Seq(_) => return Err(LoadError { message: "映射结束遇到序列".into() }),
                };
                attach(&mut stack, &mut root, Node::Map(entries))?;
            }
            Event::Alias(_) => {
                return Err(LoadError { message: "锚点/别名不在 script_v2 契约内".into() });
            }
        }
    }

    root.ok_or_else(|| LoadError { message: "空文档".into() })
}
