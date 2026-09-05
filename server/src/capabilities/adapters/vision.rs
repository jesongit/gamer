use std::sync::Arc;

use async_trait::async_trait;

use super::super::{
    CapabilityError, CapabilityResult, ColorSample, FrameHandle, FramePoint, MatchBox,
    MatchManyRequest, MatchManyResult, MatchOutcome, ResourceService, TemplateQuery, VisionService,
};
use super::{FrameStore, ResourceAdapter};

pub(crate) struct VisionAdapter {
    frames: Arc<FrameStore>,
    resources: Arc<ResourceAdapter>,
}

impl VisionAdapter {
    pub(crate) fn new(frames: Arc<FrameStore>, resources: Arc<ResourceAdapter>) -> Self {
        Self { frames, resources }
    }

    /// 搜索区域裁决：步骤显式 region 优先；未给时按模板实际文件名的 `#`
    /// 后缀推断（`xx#l` 半区 / `xx#0_0_500_500` 千分比矩形，与 v2 引擎和
    /// 匹配预览端点共用 `template_region_from_name`，区域语义不漂移）。
    /// 都没有 → 全屏（None）。
    fn effective_region(
        explicit: Option<crate::capabilities::SearchRegion>,
        file_name: Option<String>,
        frame_dims: (u32, u32),
    ) -> Option<[u32; 4]> {
        explicit
            .map(|region| [region.x, region.y, region.width, region.height])
            .or_else(|| {
                file_name.and_then(|name| {
                    crate::matcher::template_region_from_name(&name, frame_dims.0, frame_dims.1)
                })
            })
    }

    async fn request(
        &self,
        frame: FrameHandle,
        query: &TemplateQuery,
    ) -> CapabilityResult<(
        Arc<crate::matcher::DecodedFrame>,
        crate::matcher::DecodedMatchRequest,
    )> {
        let frame = self.frames.get(frame)?;
        self.resources.open(query.template()).await?;
        let template_png = self.resources.read(query.template())?;
        let region = Self::effective_region(
            query.options().region,
            self.resources.file_name(query.template()).ok(),
            frame.dimensions(),
        );
        let request = crate::matcher::DecodedMatchRequest {
            template_png,
            threshold: query.options().threshold,
            region,
            color: query.options().color_check,
        };
        Ok((frame, request))
    }

    fn map_match(result: Option<crate::matcher::MatchResult>) -> MatchOutcome {
        result
            .map(|result| {
                MatchOutcome::Found(MatchBox {
                    x: result.x,
                    y: result.y,
                    width: result.width,
                    height: result.height,
                    score: result.score,
                })
            })
            .unwrap_or(MatchOutcome::NotFound)
    }
}

#[async_trait]
impl VisionService for VisionAdapter {
    async fn match_template(
        &self,
        frame: FrameHandle,
        template: TemplateQuery,
    ) -> CapabilityResult<MatchOutcome> {
        let (frame, request) = self.request(frame, &template).await?;
        crate::matcher::compute::run(move || crate::matcher::match_decoded_frame(&frame, &request))
            .await
            .map_err(|error| CapabilityError::Failed(error.to_string()))?
            .map(Self::map_match)
            .map_err(|error| CapabilityError::Failed(error.to_string()))
    }

    async fn match_many(
        &self,
        request: &MatchManyRequest,
    ) -> CapabilityResult<Vec<MatchManyResult>> {
        let frame = self.frames.get(request.frame())?;
        let mut queries = Vec::with_capacity(request.templates().len());
        for query in request.templates() {
            self.resources.open(query.template()).await?;
            let template_png = self.resources.read(query.template())?;
            let region = Self::effective_region(
                query.options().region,
                self.resources.file_name(query.template()).ok(),
                frame.dimensions(),
            );
            queries.push(crate::matcher::DecodedMatchRequest {
                template_png,
                threshold: query.options().threshold,
                region,
                color: query.options().color_check,
            });
        }
        let results = crate::matcher::compute::run(move || {
            crate::matcher::match_decoded_many(&frame, &queries)
        })
        .await
        .map_err(|error| CapabilityError::Failed(error.to_string()))?
        .map_err(|error| CapabilityError::Failed(error.to_string()))?;
        Ok(request
            .templates()
            .iter()
            .zip(results)
            .map(|(query, result)| MatchManyResult {
                template: query.template(),
                outcome: Self::map_match(result),
            })
            .collect())
    }

    async fn sample_color(
        &self,
        frame: FrameHandle,
        point: FramePoint,
    ) -> CapabilityResult<ColorSample> {
        let frame = self.frames.get(frame)?;
        let [red, green, blue] = frame
            .pixel(point.x, point.y)
            .ok_or_else(|| CapabilityError::InvalidRequest("color point outside frame".into()))?;
        Ok(ColorSample { red, green, blue })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 模板 `#` 后缀区域兜底：显式 region 优先、千分比矩形换算、无后缀全屏。
    #[test]
    fn effective_region_prefers_explicit_then_template_name_suffix() {
        let explicit = Some(crate::capabilities::SearchRegion::new(1, 2, 3, 4));
        assert_eq!(
            VisionAdapter::effective_region(explicit, Some("probe#0_500_500_999.png".into()), (1000, 1000)),
            Some([1, 2, 3, 4])
        );
        // 千分比矩形：0.5~0.75 × 0.25~0.5 → 像素 500,250,250,250
        assert_eq!(
            VisionAdapter::effective_region(None, Some("probe#500_250_750_500.png".into()), (1000, 1000)),
            Some([500, 250, 250, 250])
        );
        // 半区字母与 `#1` 彩色标记
        assert_eq!(
            VisionAdapter::effective_region(None, Some("probe#d#1.png".into()), (1000, 800)),
            Some([0, 400, 1000, 400])
        );
        // 无后缀 / `#a` = 全屏
        assert_eq!(VisionAdapter::effective_region(None, Some("probe.png".into()), (1000, 1000)), None);
        assert_eq!(VisionAdapter::effective_region(None, Some("probe#a.png".into()), (1000, 1000)), None);
        // 文件名拿不到（不可能路径兜底）→ 全屏
        assert_eq!(VisionAdapter::effective_region(None, None, (1000, 1000)), None);
    }
}
