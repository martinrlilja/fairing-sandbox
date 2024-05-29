use anyhow::{anyhow, Result};
use std::path::PathBuf;
use tokio::fs;

use fairing_core2::{models, repositories::GitSourceRepository};

pub struct LocalGitSource;

#[async_trait::async_trait]
impl GitSourceRepository for LocalGitSource {
    async fn git_list_latest(
        &self,
        _source: &models::SourceWithKind<models::GitSource>,
    ) -> Result<Vec<models::GitSourceRefAndCommit>> {
        Ok(vec![models::GitSourceRefAndCommit {
            ref_: "refs/heads/master".into(),
            commit: "46720c277c549b0b59a1d80c0128ff69f42a13b5".into(),
        }])
    }

    async fn git_clone(
        &self,
        _source: &models::SourceWithKind<models::GitSource>,
        _ref_and_commit: &models::GitSourceRefAndCommit,
        work_directory: PathBuf,
    ) -> Result<PathBuf> {
        let mut source_directory = work_directory.clone();
        source_directory.push("source");

        fs::create_dir(&source_directory).await?;

        let mut index_path = source_directory.clone();
        index_path.push("index.html");
        fs::write(&index_path, include_bytes!("../web-test/index.html")).await?;

        Ok(source_directory)
    }
}
