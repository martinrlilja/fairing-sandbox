use anyhow::{ensure, Result};
use futures_util::{pin_mut, Stream, StreamExt};
use std::{convert::TryInto, marker::Unpin};

use crate::models;

#[derive(Clone)]
pub struct Storage {
    files: crate::backends::file_storage::FileStorage,
    file_metadata: crate::backends::file_metadata::FileMetadata,
}

impl Storage {
    pub fn new(
        files: crate::backends::file_storage::FileStorage,
        file_metadata: crate::backends::file_metadata::FileMetadata,
    ) -> Storage {
        Storage {
            files,
            file_metadata,
        }
    }

    pub async fn store_file(
        &self,
        file_keyspace: &models::FileKeyspace,
        file_size: i64,
        file_stream: impl Stream<Item = Vec<u8>> + Unpin,
    ) -> Result<()> {
        use blake2::{Blake2b, Digest};
        use fastcdc::FastCDC;

        const CHUNK_AVG_SIZE: usize = 4_194_304;

        let file = models::CreateFile {
            file_namespace_id: file_keyspace.id,
            size: file_size,
        };

        let file = self.file_metadata.create_file(&file).await?;

        // Limit the number of file chunks.
        ensure!(file.size <= (CHUNK_AVG_SIZE as i64) * 2_000);
        ensure!(file.size >= 0);

        let mut hasher = Blake2b::with_params(&file_keyspace.key, &[], &[]);

        // Buffer with data that has not yet been chunked.
        let mut buffer = Vec::new();
        // Blob metadata to create.
        let mut blobs = Vec::new();
        // Chunk metadata to create.
        let mut chunks = Vec::new();

        // Number of bytes chunked so far.
        let mut chunked_bytes = 0_i64;

        pin_mut!(file_stream);

        while let Some(file_data) = file_stream.next().await {
            hasher.update(&file_data);

            ensure!(file_data.len() <= CHUNK_AVG_SIZE);
            ensure!(chunked_bytes <= file.size);

            let data = if !buffer.is_empty() {
                buffer.extend_from_slice(&file_data);
                buffer
            } else {
                file_data
            };

            let chunker = FastCDC::with_eof(
                &data,
                CHUNK_AVG_SIZE / 4,
                CHUNK_AVG_SIZE,
                CHUNK_AVG_SIZE * 8,
                false,
            );

            let mut data_read = 0;

            for chunk in chunker {
                let chunk_data = &data[chunk.offset..chunk.offset + chunk.length];

                let blob = self.store_blob(chunk_data).await?;

                let file_chunk = models::CreateFileChunk {
                    file_id: file.id.clone(),
                    start_byte_offset: chunked_bytes,
                    end_byte_offset: chunked_bytes + chunk.length as i64,
                    blob_checksum: blob.checksum.clone(),
                };

                blobs.push(blob);
                chunks.push(file_chunk);

                data_read += chunk.length;
                chunked_bytes += chunk.length as i64;
            }

            buffer = data;
            buffer.copy_within(data_read.., 0);
            buffer.truncate(buffer.len() - data_read);
        }

        // If there is still data in the buffer once the file_stream is empty, assume that the data
        // left would be the last chunk.
        if !buffer.is_empty() {
            let blob = self.store_blob(&buffer).await?;

            let file_chunk = models::CreateFileChunk {
                file_id: file.id.clone(),
                start_byte_offset: chunked_bytes,
                end_byte_offset: chunked_bytes + buffer.len() as i64,
                blob_checksum: blob.checksum.clone(),
            };

            blobs.push(blob);
            chunks.push(file_chunk);

            chunked_bytes += buffer.len() as i64;
        }

        ensure!(chunked_bytes == file.size);

        for blob in blobs {
            self.file_metadata.create_blob(&blob).await?;
        }

        for chunk in chunks {
            self.file_metadata.create_file_chunk(&chunk).await?;
        }

        let checksum = hasher.finalize();

        let file = self
            .file_metadata
            .finalize_file(
                &file.id,
                &models::FinalizeFile {
                    checksum: checksum.to_vec(),
                    is_valid_utf8: false,
                },
            )
            .await?;

        Ok(file)
    }

    async fn store_blob(&self, data: &[u8]) -> Result<models::CreateBlob> {
        use blake2::{Blake2b, Digest};

        const COMPRESSION_LEVEL: i16 = 14;

        let mut hasher = Blake2b::new();

        hasher.update(data);

        let checksum = hasher.finalize();
        let checksum = models::BlobChecksum(checksum.to_vec());

        let compressed = zstd::encode_all(data, COMPRESSION_LEVEL as i32)?;

        self.files.store_blob(&checksum, data).await?;

        Ok(models::CreateBlob {
            checksum,
            storage_id: 0,
            size: data.len().try_into()?,
            size_on_disk: compressed.len().try_into()?,
            compression_algorithm: 1,
            compression_level: COMPRESSION_LEVEL,
        })
    }
}