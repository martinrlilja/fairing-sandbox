use anyhow::Result;
use std::{fmt::Debug, sync::Arc};

use crate::models;

pub type FileStorage = Arc<dyn FileStorageBackend>;

#[async_trait::async_trait]
pub trait FileStorageBackend: Debug + Send + Sync {
    async fn store_blob(&self, blob_checksum: &models::BlobChecksum, data: &[u8]) -> Result<()>;

    async fn load_blob(&self, blob_checksum: &models::BlobChecksum) -> Result<Vec<u8>>;
}
