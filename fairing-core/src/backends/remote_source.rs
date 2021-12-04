use anyhow::Result;
use std::{fmt::Debug, path::PathBuf, sync::Arc};

use crate::models;

pub type RemoteSource = Arc<dyn RemoteSourceBackend>;

pub trait RemoteSourceBackend: Debug + RemoteSourceRepository {}

impl<T> RemoteSourceBackend for T where T: Debug + RemoteSourceRepository {}

#[async_trait::async_trait]
pub trait RemoteSourceRepository: Send + Sync {
    async fn list_tree_revisions(
        &self,
        source: &models::Source,
    ) -> Result<Vec<models::CreateBuild>>;

    async fn fetch(
        &self,
        source: &models::Source,
        build: &models::Build,
        work_directory: PathBuf,
    ) -> Result<PathBuf>;
}
