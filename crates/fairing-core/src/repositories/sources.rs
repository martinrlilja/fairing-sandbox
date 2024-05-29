use anyhow::Result;

use crate::models;

#[async_trait::async_trait]
pub trait SourceRepository: Send + Sync {
    async fn get_source(
        &self,
        project_id: &models::ProjectId,
        name: &models::SourceName,
    ) -> Result<Option<models::Source>>;

    async fn list_sources(&self, project_id: &models::ProjectId) -> Result<Vec<models::Source>>;

    async fn create_or_update_source(&self, source: &models::Source) -> Result<()>;
}
