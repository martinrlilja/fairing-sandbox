use anyhow::{anyhow, Result};
use std::sync::Arc;

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
    ) -> Result<Vec<models::CreateTreeRevision>> {
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
}
