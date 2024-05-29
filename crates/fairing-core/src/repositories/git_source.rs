use anyhow::Result;
use std::path::PathBuf;

use crate::models;

#[async_trait::async_trait]
pub trait GitSourceRepository: Send + Sync {
    async fn git_list_latest(
        &self,
        source: &models::SourceWithKind<models::GitSource>,
    ) -> Result<Vec<models::GitSourceRefAndCommit>>;

    async fn git_clone(
        &self,
        source: &models::SourceWithKind<models::GitSource>,
        ref_and_commit: &models::GitSourceRefAndCommit,
        work_directory: PathBuf,
    ) -> Result<PathBuf>;
}
