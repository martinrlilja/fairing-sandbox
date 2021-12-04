use anyhow::{anyhow, Result};
use std::{path::PathBuf, sync::Arc};

use fairing_core::{
    backends::{remote_site_source, RemoteSiteSource},
    models,
};

mod git;

#[derive(Debug, Clone)]
pub struct GenericRemoteSiteSource;

impl GenericRemoteSiteSource {
    pub fn new() -> RemoteSiteSource {
        Arc::new(GenericRemoteSiteSource)
    }
}

#[async_trait::async_trait]
impl remote_site_source::RemoteSiteSourceRepository for GenericRemoteSiteSource {
    //#[tracing::instrument]
    async fn list_tree_revisions(
        &self,
        site_source: &models::SiteSource,
    ) -> Result<Vec<models::CreateBuild>> {
        match site_source.kind {
            Some(models::SiteSourceKind::GitSource(ref git_source)) => {
                let remote_site_source = git::GitRemoteSiteSource;
                remote_site_source
                    .list_tree_revisions(site_source, git_source)
                    .await
            }
            None => Err(anyhow!("unknown site source kind")),
        }
    }

    async fn fetch(
        &self,
        site_source: &models::SiteSource,
        build: &models::Build,
        work_directory: PathBuf,
    ) -> Result<PathBuf> {
        match site_source.kind {
            Some(models::SiteSourceKind::GitSource(ref git_source)) => {
                let remote_site_source = git::GitRemoteSiteSource;
                remote_site_source
                    .fetch(site_source, git_source, build, work_directory)
                    .await
            }
            None => Err(anyhow!("unknown site source kind")),
        }
    }
}
