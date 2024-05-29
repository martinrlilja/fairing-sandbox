use anyhow::Result;
use blake2::{digest::Mac, Blake2bMac};

use super::ProjectId;

#[derive(Copy, Clone, Debug, PartialEq, bincode::Encode, bincode::Decode)]
pub enum FileChecksum {
    Deleted,
    Blake2b(FileEncoding, [u8; 32]),
}

#[derive(Copy, Clone, Debug, PartialEq, bincode::Encode, bincode::Decode)]
pub enum FileEncoding {
    Identity,
    Gzip,
    Zstd,
    Brotli,
}

impl FileChecksum {
    pub fn encode(&self) -> Vec<u8> {
        let config = bincode::config::standard().skip_fixed_array_length();
        bincode::encode_to_vec(self, config).unwrap()
    }

    pub fn decode(bytes: &[u8]) -> Result<FileChecksum> {
        let config = bincode::config::standard().skip_fixed_array_length();
        let (checksum, _) = bincode::decode_from_slice(bytes, config)?;
        Ok(checksum)
    }

    pub fn blake2b_hasher(project_id: ProjectId) -> Blake2bHasher {
        let hasher = blake2::Blake2bMac::new_with_salt_and_personal(
            project_id.into_uuid().as_bytes(),
            &[],
            b"file",
        )
        .unwrap();
        Blake2bHasher(hasher)
    }

    pub fn with_encoding(self, encoding: FileEncoding) -> FileChecksum {
        match self {
            FileChecksum::Deleted => FileChecksum::Deleted,
            FileChecksum::Blake2b(_, checksum) => FileChecksum::Blake2b(encoding, checksum),
        }
    }
}

pub struct Blake2bHasher(Blake2bMac<blake2::digest::consts::U32>);

impl Blake2bHasher {
    pub fn update(&mut self, data: &[u8]) {
        self.0.update(data)
    }

    pub fn finalize(self) -> FileChecksum {
        let mut bytes = [0u8; 32];
        let result = self.0.finalize();
        bytes.copy_from_slice(&result.into_bytes());
        FileChecksum::Blake2b(FileEncoding::Identity, bytes)
    }
}

#[derive(Clone, Debug)]
pub struct File {
    pub project_id: ProjectId,
    pub checksum: FileChecksum,
    pub length: u64,
}

#[derive(Clone, Debug)]
pub struct FileChunk {
    pub total_length: u64,
    pub offset: u64,
    pub data: Vec<u8>,
}
