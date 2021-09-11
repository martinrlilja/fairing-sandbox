use anyhow::Result;
use fairing_core::{backends::file_metadata, models};

use crate::backends::PostgresDatabase;

#[async_trait::async_trait]
impl file_metadata::FileMetadataRepository for PostgresDatabase {
    async fn create_blob(&self, blob: &models::CreateBlob) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO blobs (checksum, storage_id, "size", size_on_disk, compression_algorithm, compression_level)
            VALUES ($1, $2, $3, $4, $5, $6);
            "#,
        )
        .bind(&blob.checksum.0)
        .bind(&blob.storage_id)
        .bind(&blob.size)
        .bind(&blob.size_on_disk)
        .bind(&blob.compression_algorithm)
        .bind(&blob.compression_level)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

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
        .bind(&file_keyspace.id.0)
        .bind(&file_keyspace.key)
        .execute(&self.pool)
        .await?;

        Ok(file_keyspace)
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
        .await?;

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
            ON CONFLICT (checksum) DO NOTHING;
            "#,
        )
        .bind(&file_id.0 .0)
        .bind(&file_id.1)
        .bind(&file.checksum)
        .bind(&file.is_valid_utf8)
        .execute(&mut tx)
        .await?;

        if result.rows_affected() == 1 {
            // If this file is new and unique, update all file chunks to the correct checksum.
            sqlx::query(
                r"
                UPDATE file_chunks
                WHERE file_keyspace = $1 AND file_checksum = $2
                SET file_checksum = $3;
                ",
            )
            .bind(&file_id.0 .0)
            .bind(&file_id.1)
            .bind(&file.checksum)
            .execute(&mut tx)
            .await?;
        }

        // Remove the temporary file.
        sqlx::query(
            r"
            DELETE FROM files
            WHERE file_keyspace = $1 AND checksum = $2;
            ",
        )
        .bind(&file_id.0 .0)
        .bind(&file_id.1)
        .execute(&mut tx)
        .await?;

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
        .bind(&file_chunk.file_id.0.0)
        .bind(&file_chunk.file_id.1)
        .bind(&file_chunk.start_byte_offset)
        .bind(&file_chunk.end_byte_offset)
        .bind(&file_chunk.blob_checksum.0)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn create_tree_leaf(&self, tree_leaf: &models::CreateTreeLeaf) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO tree_leaves (tree_id, version, path, file_keyspace, file_checksum)
            VALUES ($1, $2, $3, $4, $5);
            "#,
        )
        .bind(&tree_leaf.tree_id.0)
        .bind(&tree_leaf.version)
        .bind(&tree_leaf.path)
        .bind(&tree_leaf.file_id.as_ref().map(|file_id| file_id.0 .0))
        .bind(
            &tree_leaf
                .file_id
                .as_ref()
                .map(|file_id| file_id.1.as_slice()),
        )
        .execute(&self.pool)
        .await?;

        Ok(())
    }
}
