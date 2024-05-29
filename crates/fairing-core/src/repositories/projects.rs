use anyhow::Result;

use crate::models;

#[async_trait::async_trait]
pub trait ProjectRepository: Send + Sync {
    async fn get_project(&self, id: &models::ProjectId) -> Result<Option<models::Project>>;

    async fn create_or_update_project(&self, project: &models::Project) -> Result<()>;
}
