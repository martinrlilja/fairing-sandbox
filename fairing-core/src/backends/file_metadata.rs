use anyhow::Result;
use std::{fmt::Debug, sync::Arc};

use crate::models;

pub type FileMetadata = Arc<dyn FileMetadataBackend>;

pub trait FileMetadataBackend: Debug + FileMetadataRepository {}

impl<T> FileMetadataBackend for T where T: Debug + FileMetadataRepository {}

#[async_trait::async_trait]
pub trait FileMetadataRepository: Send + Sync {
    async fn create_file_keyspace(
        &self,
        file_keyspace: &models::CreateFileKeyspace,
    ) -> Result<models::FileKeyspace>;

    async fn get_file_keyspace(
        &self,
        file_keyspace_id: &models::FileKeyspaceId,
    ) -> Result<Option<models::FileKeyspace>>;

    async fn create_blob(&self, blob: &models::CreateBlob) -> Result<()>;

    async fn create_file(&self, file: &models::CreateFile) -> Result<models::File>;

    async fn finalize_file(
        &self,
        file_id: &models::FileId,
        file: &models::FinalizeFile,
    ) -> Result<()>;

    async fn create_file_chunk(&self, file_chunk: &models::CreateFileChunk) -> Result<()>;

    async fn create_layer_member(&self, layer_member: &models::CreateLayerMember) -> Result<()>;

    async fn get_layer_member_file(
        &self,
        layer_set_id: models::LayerSetId,
        layer_id: models::LayerId,
        path: &str,
    ) -> Result<Option<models::File>>;

    async fn get_file_chunks(&self, file_id: &models::FileId) -> Result<Vec<Vec<u8>>>;
}
