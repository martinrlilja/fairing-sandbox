use anyhow::{anyhow, ensure, Result};

use super::auth::{Authentication, LayerPermissions, LayerSetPermissions};
use crate::{models, repositories::LayerRepository};

pub struct LayerService {
    repository: &'static dyn LayerRepository,
}

impl LayerService {
    pub fn new(repository: &'static dyn LayerRepository) -> LayerService {
        LayerService { repository }
    }

    pub async fn get_layer_set(
        &self,
        auth: &Authentication,
        layer_set_name: &models::LayerSetName,
    ) -> Result<Option<models::LayerSet>> {
        auth.can(LayerSetPermissions::Get)?;
        let project_id = auth.project_id()?;

        let layer_set = self
            .repository
            .get_layer_set(project_id, layer_set_name)
            .await?;

        Ok(layer_set)
    }

    pub async fn create_layer_set(
        &self,
        auth: &Authentication,
        layer_set: &models::CreateLayerSet,
    ) -> Result<models::LayerSet> {
        auth.can(LayerSetPermissions::Create)?;
        let project_id = auth.project_id()?;

        let source = match &layer_set.source {
            Some(models::CreateLayerSetSource { source, kind }) => {
                let kind = match (&source.kind, kind) {
                    (
                        &models::SourceKind::Git { .. },
                        models::CreateLayerSetSourceKind::Git { ref_ },
                    ) => models::LayerSetSourceKind::Git { ref_: ref_.clone() },
                };

                Some(models::LayerSetSource {
                    name: source.name.clone(),
                    kind,
                })
            }
            None => None,
        };

        let layer_set = models::LayerSet {
            project_id,
            name: layer_set.name.clone(),
            visibility: layer_set.visibility,
            source,
            build_status: models::LayerSetBuildStatus {
                current_layer_id: None,
                last_layer_id: None,
            },
        };

        self.repository.create_layer_set(&layer_set).await?;

        Ok(layer_set)
    }

    pub async fn create_layer(
        &self,
        auth: &Authentication,
        layer_set: &models::LayerSet,
        layer: &models::CreateLayer,
    ) -> Result<models::Layer> {
        auth.can(LayerPermissions::Create)?;
        let project_id = auth.project_id()?;

        ensure!(project_id == layer_set.project_id);

        let layer_id = models::LayerId::new()?;

        self.repository
            .set_last_layer_id(project_id, &layer_set.name, layer_id)
            .await?;

        let source = match (&layer_set.source, &layer.source) {
            (
                Some(models::LayerSetSource {
                    kind: models::LayerSetSourceKind::Git { .. },
                    ..
                }),
                Some(models::CreateLayerSource::Git { commit }),
            ) => Some(models::LayerSource::Git {
                commit: commit.clone(),
            }),
            (None, Some(_)) => {
                return Err(anyhow!(
                    "no source configured on layer set, but layer specified source data"
                ))
            }
            (Some(_), None) => {
                return Err(anyhow!(
                    "no source data configured on layer, but layer set specified a source"
                ))
            }
            (None, None) => None,
        };

        let layer = models::Layer {
            project_id,
            layer_set_name: layer_set.name.clone(),
            id: layer_id,
            status: models::LayerStatus::Building,
            source,
        };

        self.repository.create_layer(&layer).await?;

        Ok(layer)
    }
}
