use anyhow::Result;
use std::{path::PathBuf, sync::Arc};
use tokio::fs;

use fairing_core::{backends::file_storage, models};

#[derive(Clone, Debug)]
pub struct LocalFileStorage {
    location: PathBuf,
}

impl LocalFileStorage {
    pub async fn open(location: impl Into<PathBuf>) -> Result<file_storage::FileStorage> {
        let location = location.into();

        fs::create_dir_all(&location).await?;

        Ok(Arc::new(LocalFileStorage { location }))
    }
}

#[async_trait::async_trait]
impl file_storage::FileStorageBackend for LocalFileStorage {
    async fn store_blob(&self, blob_checksum: &models::BlobChecksum, data: &[u8]) -> Result<()> {
        use std::io::ErrorKind;

        let mut path = self.location.clone();
        path.push(blob_checksum.hex_prefix());

        fs::create_dir(&path).await?;

        let mut temp_path = path.clone();
        temp_path.push(uuid::Uuid::new_v4().to_string());

        path.push(blob_checksum.hex());

        match fs::metadata(&path).await {
            Err(err) if err.kind() == ErrorKind::NotFound => {
                fs::write(&temp_path, data).await?;
                fs::rename(&temp_path, &path).await?;
            }
            _ => (),
        }

        Ok(())
    }

    async fn load_blob(&self, blob_checksum: &models::BlobChecksum) -> Result<Vec<u8>> {
        let mut path = self.location.clone();
        path.push(blob_checksum.hex_prefix());
        path.push(blob_checksum.hex());

        let data = fs::read(&path).await?;

        Ok(data)
    }
}
