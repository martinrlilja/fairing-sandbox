use anyhow::Result;
use std::{fmt::Debug, sync::Arc};

use crate::models;

pub type FileMetadata = Arc<dyn FileMetadataBackend>;

pub trait FileMetadataBackend: Debug + FileMetadataRepository {}

impl<T> FileMetadataBackend for T where T: Debug + FileMetadataRepository {}

#[async_trait::async_trait]
pub trait FileMetadataRepository: Send + Sync {
    async fn create_blob(&self, blob: &models::CreateBlob) -> Result<()>;

    async fn create_file_keyspace(
        &self,
        file_keyspace: &models::CreateFileKeyspace,
    ) -> Result<models::FileKeyspace>;

    async fn create_file(&self, file: &models::CreateFile) -> Result<models::File>;

    async fn finalize_file(
        &self,
        file_id: &models::FileId,
        file: &models::FinalizeFile,
    ) -> Result<()>;

    async fn create_file_chunk(&self, file_chunk: &models::CreateFileChunk) -> Result<()>;

    async fn create_tree_leaf(&self, tree_leaf: &models::CreateTreeLeaf) -> Result<()>;
}
