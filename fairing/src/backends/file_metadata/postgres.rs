use anyhow::{Context as _, Result};
use fairing_core::{backends::file_metadata, models};

use crate::backends::PostgresDatabase;

#[async_trait::async_trait]
impl file_metadata::FileMetadataRepository for PostgresDatabase {
    async fn create_file_keyspace(
        &self,
        file_keyspace: &models::CreateFileKeyspace,
    ) -> Result<models::FileKeyspace> {
        let file_keyspace = file_keyspace.create();

        sqlx::query(
            r#"
            INSERT INTO file_keyspace (id, key)
            VALUES ($1, $2);
            "#,
        )
        .bind(&file_keyspace.id)
        .bind(&file_keyspace.key)
        .execute(&self.pool)
        .await
        .context("create file keyspace")?;

        Ok(file_keyspace)
    }

    async fn get_file_keyspace(
        &self,
        file_keyspace_id: &models::FileKeyspaceId,
    ) -> Result<Option<models::FileKeyspace>> {
        let file_keyspace = sqlx::query_as(
            r#"
            SELECT id, key
            FROM file_keyspace
            WHERE id = $1;
            "#,
        )
        .bind(&file_keyspace_id)
        .fetch_optional(&self.pool)
        .await
        .context("get file keyspace")?;

        Ok(file_keyspace)
    }

    async fn create_blob(&self, blob: &models::CreateBlob) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO blobs (checksum, storage_id, "size", size_on_disk, compression_algorithm, compression_level)
            VALUES ($1, $2, $3, $4, $5, $6)
            ON CONFLICT (checksum) DO NOTHING;
            "#,
        )
        .bind(&blob.checksum.0)
        .bind(&blob.storage_id)
        .bind(&blob.size)
        .bind(&blob.size_on_disk)
        .bind(&blob.compression_algorithm)
        .bind(&blob.compression_level)
        .execute(&self.pool)
        .await
        .context("create blob")?;

        Ok(())
    }

    async fn create_file(&self, file: &models::CreateFile) -> Result<models::File> {
        let file = file.create();

        sqlx::query(
            r#"
            INSERT INTO files (file_keyspace, checksum, size, is_valid_utf8)
            VALUES ($1, $2, $3, $4);
            "#,
        )
        .bind(&file.id.0 .0)
        .bind(&file.id.1)
        .bind(&file.size)
        .bind(&file.is_valid_utf8)
        .execute(&self.pool)
        .await
        .context("create file")?;

        Ok(file)
    }

    async fn finalize_file(
        &self,
        file_id: &models::FileId,
        file: &models::FinalizeFile,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await?;

        let result = sqlx::query(
            r#"
            INSERT INTO files (file_keyspace, checksum, size, is_valid_utf8)
            SELECT $1, $3, size, $4
            FROM files
            WHERE file_keyspace = $1 AND checksum = $2
            ON CONFLICT (file_keyspace, checksum) DO NOTHING;
            "#,
        )
        .bind(&file_id.0)
        .bind(&file_id.1)
        .bind(&file.checksum)
        .bind(&file.is_valid_utf8)
        .execute(&mut tx)
        .await
        .context("finalize file (create)")?;

        if result.rows_affected() == 1 {
            // If this file is new and unique, update all file chunks to the correct checksum.
            sqlx::query(
                r"
                UPDATE file_chunks
                SET file_checksum = $3
                WHERE file_keyspace = $1 AND file_checksum = $2;
                ",
            )
            .bind(&file_id.0)
            .bind(&file_id.1)
            .bind(&file.checksum)
            .execute(&mut tx)
            .await
            .context("finalize file (update)")?;
        }

        // Remove the temporary file.
        sqlx::query(
            r"
            DELETE FROM files
            WHERE file_keyspace = $1 AND checksum = $2;
            ",
        )
        .bind(&file_id.0)
        .bind(&file_id.1)
        .execute(&mut tx)
        .await
        .context("finalize file (clean up)")?;

        tx.commit().await?;

        Ok(())
    }

    async fn create_file_chunk(&self, file_chunk: &models::CreateFileChunk) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO file_chunks (file_keyspace, file_checksum, start_byte_offset, end_byte_offset, blob_checksum)
            VALUES ($1, $2, $3, $4, $5);
            "#,
        )
        .bind(&file_chunk.file_id.0)
        .bind(&file_chunk.file_id.1)
        .bind(&file_chunk.start_byte_offset)
        .bind(&file_chunk.end_byte_offset)
        .bind(&file_chunk.blob_checksum.0)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn create_layer_member(&self, layer_member: &models::CreateLayerMember) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO layer_members (layer_set_id, layer_id, path, file_keyspace, file_checksum)
            VALUES ($1, $2, $3, $4, $5);
            "#,
        )
        .bind(&layer_member.layer_set_id)
        .bind(&layer_member.layer_id)
        .bind(&layer_member.path)
        .bind(&layer_member.file_id.as_ref().map(|file_id| file_id.0))
        .bind(
            &layer_member
                .file_id
                .as_ref()
                .map(|file_id| file_id.1.as_slice()),
        )
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn get_layer_member_file(
        &self,
        layer_set_id: models::LayerSetId,
        layer_id: models::LayerId,
        path: &str,
    ) -> Result<Option<models::File>> {
        let file: Option<(models::FileKeyspaceId, Vec<u8>, i64, bool)> = sqlx::query_as(
            r"
            SELECT f.file_keyspace, f.checksum, f.size, f.is_valid_utf8
            FROM files f
            JOIN layer_members lm ON lm.file_keyspace = f.file_keyspace AND lm.file_checksum = f.checksum
            WHERE lm.layer_set_id = $1 AND lm.layer_id = $2 AND lm.path = $3;
            "
        )
        .bind(layer_set_id)
        .bind(layer_id)
        .bind(path)
        .fetch_optional(&self.pool)
        .await?;

        match file {
            Some((file_keyspace, file_id, size, is_valid_utf8)) => Ok(Some(models::File {
                id: models::FileId(file_keyspace, file_id),
                size,
                is_valid_utf8,
            })),
            None => Ok(None),
        }
    }

    async fn get_file_chunks(&self, file_id: &models::FileId) -> Result<Vec<Vec<u8>>> {
        let blob_checksums = sqlx::query_scalar(
            r"
            SELECT fc.blob_checksum
            FROM file_chunks fc
            WHERE fc.file_keyspace = $1 AND fc.file_checksum = $2;
            ",
        )
        .bind(file_id.0)
        .bind(&file_id.1)
        .fetch_all(&self.pool)
        .await?;

        Ok(blob_checksums)
    }
}
