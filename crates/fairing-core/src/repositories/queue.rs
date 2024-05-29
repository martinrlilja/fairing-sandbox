use anyhow::Result;

use crate::models;

#[async_trait::async_trait]
pub trait QueueRepository: Send + Sync {
    async fn queue_build(&self, message: &models::BuildQueueMessage) -> Result<()>;

    async fn assign_build(
        &self,
        worker_id: models::WorkerId,
    ) -> Result<Option<models::BuildQueueMessage>>;
}
