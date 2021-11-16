use crate::models;

#[derive(Clone, Debug)]
pub struct BlobChecksum(pub Vec<u8>);

impl BlobChecksum {
    pub fn hex_prefix(&self) -> String {
        hex::encode(&self.0[..1])
    }

    pub fn hex(&self) -> String {
        hex::encode(&self.0)
    }
}

#[derive(Clone, Debug, sqlx::Type)]
#[repr(i16)]
pub enum CompressionAlgorithm {
    None = 0,
    Zstd = 1,
}

#[derive(Clone, Debug)]
pub struct CreateBlob {
    pub checksum: BlobChecksum,
    pub storage_id: i16,

    pub size: i32,
    pub size_on_disk: i32,

    pub compression_algorithm: CompressionAlgorithm,
    pub compression_level: i16,
}

#[derive(Copy, Clone, Debug, sqlx::Type)]
#[sqlx(transparent)]
pub struct FileKeyspaceId(pub uuid::Uuid);

#[derive(Clone, Debug, sqlx::FromRow)]
pub struct FileKeyspace {
    pub id: FileKeyspaceId,
    pub key: Vec<u8>,
}

pub struct CreateFileKeyspace;

impl CreateFileKeyspace {
    pub fn create(&self) -> FileKeyspace {
        use rand::RngCore;

        let key = {
            let mut key = vec![0u8; 32];
            rand::thread_rng().fill_bytes(&mut key);
            key
        };

        FileKeyspace {
            id: FileKeyspaceId(uuid::Uuid::new_v4()),
            key,
        }
    }
}

#[derive(Clone, Debug)]
pub struct FileId(pub FileKeyspaceId, pub Vec<u8>);

#[derive(Clone, Debug)]
pub struct File {
    pub id: FileId,
    pub size: i64,
    pub is_valid_utf8: bool,
}

pub struct CreateFile {
    pub file_namespace_id: FileKeyspaceId,
    pub size: i64,
}

impl CreateFile {
    pub fn create(&self) -> File {
        let checksum = uuid::Uuid::new_v4().as_bytes().to_vec();

        File {
            id: FileId(self.file_namespace_id, checksum),
            size: self.size,
            is_valid_utf8: false,
        }
    }
}

pub struct FinalizeFile {
    pub checksum: Vec<u8>,
    pub is_valid_utf8: bool,
}

pub struct CreateFileChunk {
    pub file_id: FileId,

    pub start_byte_offset: i64,
    pub end_byte_offset: i64,

    pub blob_checksum: BlobChecksum,
}

pub struct CreateTreeLeaf {
    pub tree_id: models::TreeId,
    pub version: i64,
    pub path: String,
    pub file_id: Option<FileId>,
}
