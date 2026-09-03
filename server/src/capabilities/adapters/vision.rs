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

    fn options(query: &TemplateQuery) -> crate::matcher::DecodedMatchRequest {
        let options = query.options();
        crate::matcher::DecodedMatchRequest {
            template_png: Vec::new(),
            threshold: options.threshold,
            region: options
                .region
                .map(|region| [region.x, region.y, region.width, region.height]),
            color: options.color_check,
        }
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
        let mut request = Self::options(query);
        self.resources.open(query.template()).await?;
        request.template_png = self.resources.read(query.template())?;
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
            let mut decoded = Self::options(query);
            self.resources.open(query.template()).await?;
            decoded.template_png = self.resources.read(query.template())?;
            queries.push(decoded);
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
