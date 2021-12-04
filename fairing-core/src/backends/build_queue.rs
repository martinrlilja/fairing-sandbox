use anyhow::Result;
use futures_util::Stream;
use std::{fmt::Debug, marker::Unpin, sync::Arc};

use crate::models;

pub type BuildQueue = Arc<dyn BuildQueueBackend>;

#[async_trait::async_trait]
pub trait BuildQueueBackend: Debug + Send + Sync {
    async fn stream_builds(
        &self,
    ) -> Result<Box<dyn Stream<Item = Result<models::Build>> + Unpin + Send>>;
}
