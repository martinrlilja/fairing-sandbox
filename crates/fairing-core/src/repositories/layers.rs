use anyhow::Result;

use crate::models;

#[derive(Copy, Clone, Debug, PartialEq)]
pub enum LayerPendingLayersFilter {
    Building,
    Finalizing,
}

#[async_trait::async_trait]
pub trait LayerRepository: Send + Sync {
    async fn get_layer_set(
        &self,
        project_id: models::ProjectId,
        name: &models::LayerSetName,
    ) -> Result<Option<models::LayerSet>>;

    async fn list_layer_sets(&self, project_id: models::ProjectId)
        -> Result<Vec<models::LayerSet>>;

    async fn list_layer_sets_for_source(
        &self,
        project_id: models::ProjectId,
        name: &models::SourceName,
    ) -> Result<Vec<models::LayerSet>>;

    async fn create_layer_set(&self, layer_set: &models::LayerSet) -> Result<()>;

    async fn set_last_layer_id(
        &self,
        project_id: models::ProjectId,
        layer_set_name: &models::LayerSetName,
        layer_id: models::LayerId,
    ) -> Result<()>;

    async fn get_last_layer(
        &self,
        project_id: models::ProjectId,
        layer_set_name: &models::LayerSetName,
    ) -> Result<Option<models::Layer>>;

    async fn create_layer(&self, layer: &models::Layer) -> Result<()>;

    async fn get_pending_layers(
        &self,
        filter: LayerPendingLayersFilter,
    ) -> Result<Vec<models::Layer>>;

    async fn try_set_current_build(
        &self,
        project_id: models::ProjectId,
        layer_set_name: &models::LayerSetName,
        layer_id: models::LayerId,
    ) -> Result<()>;

    async fn build_layer(
        &self,
        project_id: models::ProjectId,
        layer_set_name: &models::LayerSetName,
        layer_id: models::LayerId,
        worker_id: models::WorkerId,
    ) -> Result<()>;

    async fn finish_build(
        &self,
        project_id: models::ProjectId,
        layer_set_name: &models::LayerSetName,
        layer_id: models::LayerId,
        worker_id: models::WorkerId,
    ) -> Result<()>;

    async fn finalize_layer(
        &self,
        project_id: models::ProjectId,
        layer_set_name: &models::LayerSetName,
        layer_id: models::LayerId,
        worker_id: models::WorkerId,
    ) -> Result<()>;

    async fn finish_finalizing(
        &self,
        project_id: models::ProjectId,
        layer_set_name: &models::LayerSetName,
        layer_id: models::LayerId,
        worker_id: models::WorkerId,
    ) -> Result<()>;

    async fn cancel_layer(
        &self,
        project_id: models::ProjectId,
        layer_set_name: &models::LayerSetName,
        layer_id: models::LayerId,
    ) -> Result<()>;

    async fn create_layer_changes(&self, layer_changes: &[models::LayerChange]) -> Result<()>;

    async fn list_layer_changes(
        &self,
        project_id: models::ProjectId,
        layer_set_name: &models::LayerSetName,
        layer_id: models::LayerId,
        worker_id: models::WorkerId,
    ) -> Result<Vec<models::LayerChange>>;

    async fn create_layer_members(&self, layer_members: &[models::LayerMember]) -> Result<()>;

    async fn get_layer_member_summary(
        &self,
        project_id: models::ProjectId,
        layer_set_name: &models::LayerSetName,
        layer_id: models::LayerId,
        paths: &[&str],
    ) -> Result<Vec<models::LayerMemberSummary>>;
}
