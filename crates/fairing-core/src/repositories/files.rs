use anyhow::Result;

use crate::models;

#[async_trait::async_trait]
pub trait FileRepository: Send + Sync {
    async fn get_file(
        &self,
        project_id: models::ProjectId,
        checksum: &models::FileChecksum,
    ) -> Result<Option<models::File>>;

    async fn create_chunk(
        &self,
        project_id: models::ProjectId,
        checksum: &models::FileChecksum,
        length: u64,
        offset: u64,
        data: Vec<u8>,
    ) -> Result<()>;

    async fn finish_file(
        &self,
        project_id: models::ProjectId,
        checksum: &models::FileChecksum,
        length: u64,
    ) -> Result<()>;

    async fn get_file_chunks(
        &self,
        project_id: models::ProjectId,
        checksum: models::FileChecksum,
        range: (u64, u64),
    ) -> Result<Vec<models::FileChunk>>;
}
