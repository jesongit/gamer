use async_trait::async_trait;

use super::{CapabilityResult, FrameHandle, ResourceHandle};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FramePoint {
    pub x: u32,
    pub y: u32,
}

impl FramePoint {
    pub const fn new(x: u32, y: u32) -> Self {
        Self { x, y }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SearchRegion {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl SearchRegion {
    pub const fn new(x: u32, y: u32, width: u32, height: u32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MatchOptions {
    pub threshold: Option<f32>,
    pub region: Option<SearchRegion>,
    pub color_check: bool,
}

impl Default for MatchOptions {
    fn default() -> Self {
        Self {
            threshold: None,
            region: None,
            color_check: false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MatchBox {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
    pub score: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MatchOutcome {
    Found(MatchBox),
    NotFound,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ColorSample {
    pub red: u8,
    pub green: u8,
    pub blue: u8,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TemplateQuery {
    template: ResourceHandle,
    options: MatchOptions,
}

impl TemplateQuery {
    pub const fn new(template: ResourceHandle, options: MatchOptions) -> Self {
        Self { template, options }
    }

    pub fn template(self) -> ResourceHandle {
        self.template
    }

    pub fn options(self) -> MatchOptions {
        self.options
    }
}

/// A single-frame batch request. Implementations must decode `frame` once and
/// reuse that decoded representation for every template query in the list.
#[derive(Clone, Debug, PartialEq)]
pub struct MatchManyRequest {
    frame: FrameHandle,
    templates: Vec<TemplateQuery>,
}

impl MatchManyRequest {
    pub fn new(frame: FrameHandle) -> Self {
        Self {
            frame,
            templates: Vec::new(),
        }
    }

    pub fn with_template(mut self, template: TemplateQuery) -> Self {
        self.templates.push(template);
        self
    }

    pub fn frame(&self) -> FrameHandle {
        self.frame
    }

    pub fn templates(&self) -> &[TemplateQuery] {
        &self.templates
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MatchManyResult {
    pub template: ResourceHandle,
    pub outcome: MatchOutcome,
}

/// Vision boundary over decoded frame handles and logical template resources.
#[async_trait]
pub trait VisionService: Send + Sync {
    async fn match_template(
        &self,
        frame: FrameHandle,
        template: TemplateQuery,
    ) -> CapabilityResult<MatchOutcome>;

    async fn match_many(
        &self,
        request: &MatchManyRequest,
    ) -> CapabilityResult<Vec<MatchManyResult>>;

    async fn sample_color(
        &self,
        frame: FrameHandle,
        point: FramePoint,
    ) -> CapabilityResult<ColorSample>;
}
