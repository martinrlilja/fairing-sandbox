use anyhow::Result;
use std::{fmt::Debug, path::PathBuf, sync::Arc};

use crate::models;

pub type RemoteSiteSource = Arc<dyn RemoteSiteSourceBackend>;

pub trait RemoteSiteSourceBackend: Debug + RemoteSiteSourceRepository {}

impl<T> RemoteSiteSourceBackend for T where T: Debug + RemoteSiteSourceRepository {}

#[async_trait::async_trait]
pub trait RemoteSiteSourceRepository: Send + Sync {
    async fn list_tree_revisions(
        &self,
        site_source: &models::SiteSource,
    ) -> Result<Vec<models::CreateBuild>>;

    async fn fetch(
        &self,
        site_source: &models::SiteSource,
        build: &models::Build,
        work_directory: PathBuf,
    ) -> Result<PathBuf>;
}
