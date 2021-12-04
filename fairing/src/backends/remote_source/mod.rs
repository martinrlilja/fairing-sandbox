use anyhow::{anyhow, Result};
use std::{path::PathBuf, sync::Arc};

use fairing_core::{
    backends::{remote_source, RemoteSource},
    models,
};

mod git;

#[derive(Debug, Clone)]
pub struct GenericRemoteSource;

impl GenericRemoteSource {
    pub fn new() -> RemoteSource {
        Arc::new(GenericRemoteSource)
    }
}

#[async_trait::async_trait]
impl remote_source::RemoteSourceRepository for GenericRemoteSource {
    //#[tracing::instrument]
    async fn list_tree_revisions(
        &self,
        source: &models::Source,
    ) -> Result<Vec<models::CreateBuild>> {
        match source.kind {
            Some(models::SourceKind::GitSource(ref git_source)) => {
                let remote_source = git::GitRemoteSource;
                remote_source.list_tree_revisions(source, git_source).await
            }
            None => Err(anyhow!("unknown site source kind")),
        }
    }

    async fn fetch(
        &self,
        source: &models::Source,
        build: &models::Build,
        work_directory: PathBuf,
    ) -> Result<PathBuf> {
        match source.kind {
            Some(models::SourceKind::GitSource(ref git_source)) => {
                let remote_source = git::GitRemoteSource;
                remote_source
                    .fetch(source, git_source, build, work_directory)
                    .await
            }
            None => Err(anyhow!("unknown site source kind")),
        }
    }
}
