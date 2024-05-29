use anyhow::{anyhow, Context as _, Result};

use super::auth::{Authentication, SourcePermissions};
use crate::{
    models,
    repositories::{GitSourceRepository, LayerRepository, SourceRepository},
};

pub struct SourceService {
    repository: &'static dyn SourceRepository,
    git_repository: &'static dyn GitSourceRepository,
    layer_repository: &'static dyn LayerRepository,
}

impl SourceService {
    pub fn new(
        repository: &'static dyn SourceRepository,
        git_repository: &'static dyn GitSourceRepository,
        layer_repository: &'static dyn LayerRepository,
    ) -> SourceService {
        SourceService {
            repository,
            git_repository,
            layer_repository,
        }
    }

    pub async fn get_source(
        &self,
        auth: &Authentication,
        source_name: &models::SourceName,
    ) -> Result<Option<models::Source>> {
        auth.can(SourcePermissions::Get)?;
        let project_id = auth.project_id()?;

        let source = self
            .repository
            .get_source(&project_id, source_name)
            .await
            .context("get source")?;

        Ok(source)
    }

    pub async fn create_source(
        &self,
        auth: &Authentication,
        source: &models::CreateSource,
    ) -> Result<models::Source> {
        auth.can(SourcePermissions::Create)?;
        let project_id = auth.project_id()?;

        let kind = match source.kind {
            models::CreateSourceKind::Git { ref repository_url } => {
                let id_ed25519 = models::Ed25519::generate();
                models::SourceKind::Git {
                    repository_url: repository_url.clone(),
                    id_ed25519,
                }
            }
        };

        let source = models::Source {
            project_id,
            name: source.name.clone(),
            kind,
        };

        self.repository
            .create_or_update_source(&source)
            .await
            .context("create source")?;

        Ok(source)
    }

    pub async fn refresh_source(
        &self,
        auth: &Authentication,
        source_name: &models::SourceName,
    ) -> Result<()> {
        auth.can(SourcePermissions::Refresh)?;
        let project_id = auth.project_id()?;

        let source = self
            .repository
            .get_source(&project_id, source_name)
            .await
            .context("get source")?
            .ok_or_else(|| anyhow!("source not found"))?;

        let layer_sets = self
            .layer_repository
            .list_layer_sets_for_source(project_id, &source.name)
            .await
            .context("list layer sets for source")?;

        match source.kind {
            models::SourceKind::Git { .. } => {
                let source = source.try_with_kind()?;
                let refs_and_commits = self
                    .git_repository
                    .git_list_latest(&source)
                    .await
                    .context("git list latest")?;

                for ref_and_commit in refs_and_commits {
                    let layer_sets =
                        layer_sets
                            .iter()
                            .filter(|layer_set| match &layer_set.source {
                                Some(models::LayerSetSource {
                                    kind: models::LayerSetSourceKind::Git { ref_ },
                                    ..
                                }) if ref_ == &ref_and_commit.ref_ => true,
                                _ => false,
                            });

                    for layer_set in layer_sets {
                        let last_layer = self
                            .layer_repository
                            .get_last_layer(project_id, &layer_set.name)
                            .await
                            .context("get last layer")?;

                        match last_layer {
                            Some(models::Layer {
                                source: Some(models::LayerSource::Git { ref commit }),
                                ..
                            }) if commit == &ref_and_commit.commit => (),
                            _ => {
                                let layer_id = models::LayerId::new()?;

                                self.layer_repository
                                    .set_last_layer_id(project_id, &layer_set.name, layer_id)
                                    .await?;

                                let layer = models::Layer {
                                    project_id,
                                    layer_set_name: layer_set.name.clone(),
                                    id: layer_id,
                                    status: models::LayerStatus::Building,
                                    source: Some(models::LayerSource::Git {
                                        commit: ref_and_commit.commit.clone(),
                                    }),
                                };

                                self.layer_repository
                                    .create_layer(&layer)
                                    .await
                                    .context("create layer")?;
                            }
                        }
                    }
                }
            }
        };

        Ok(())
    }
}
