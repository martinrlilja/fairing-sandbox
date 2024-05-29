use anyhow::Result;
use scylla::{
    frame::value::MaybeUnset, prepared_statement::PreparedStatement, statement::Consistency,
    FromRow, Session,
};
use uuid::Uuid;

use fairing_core2::{models, repositories::FileRepository};

use crate::ScyllaRepository;

#[derive(Debug, FromRow)]
struct File {
    project_id: Uuid,
    checksum: Vec<u8>,
    length: Option<i64>,
}

impl Into<models::File> for File {
    fn into(self) -> models::File {
        models::File {
            project_id: self.project_id.into(),
            checksum: models::FileChecksum::decode(&self.checksum).unwrap(),
            length: self.length.unwrap() as u64,
        }
    }
}

#[derive(Debug, FromRow)]
struct FileChunk {
    length: i64,
    offset: i64,
    data: Vec<u8>,
}

impl Into<models::FileChunk> for FileChunk {
    fn into(self) -> models::FileChunk {
        models::FileChunk {
            total_length: self.length as u64,
            offset: self.offset as u64,
            data: self.data,
        }
    }
}

pub(crate) struct Statements {
    get_file: PreparedStatement,
    get_file_chunks: PreparedStatement,
    create_chunk: PreparedStatement,
    finish_file: PreparedStatement,
}

impl Statements {
    pub(crate) async fn prepare(session: &Session) -> Result<Statements> {
        let mut get_file = session
            .prepare(
                r"
                    SELECT project_id, checksum, length
                    FROM files
                    WHERE project_id = ? AND checksum = ? AND bucket = ?;
                    ",
            )
            .await?;
        get_file.set_consistency(Consistency::LocalQuorum);

        let mut get_file_chunks = session
            .prepare(
                r"
                SELECT length, offset, data
                FROM files
                WHERE project_id = ? AND checksum = ? AND bucket = ?
                    AND offset >= ? AND offset < ?;
                ",
            )
            .await?;
        get_file_chunks.set_consistency(Consistency::LocalQuorum);

        let mut create_chunk = session
            .prepare(
                r"
                INSERT INTO files (project_id, checksum, bucket, length, offset, data)
                VALUES (?, ?, ?, ?, ?, ?);
                ",
            )
            .await?;
        create_chunk.set_consistency(Consistency::EachQuorum);

        let mut finish_file = session
            .prepare(
                r"
                UPDATE files
                SET length = ?
                WHERE project_id = ? AND checksum = ? AND bucket = 0;
                ",
            )
            .await?;
        finish_file.set_consistency(Consistency::EachQuorum);

        Ok(Statements {
            get_file,
            get_file_chunks,
            create_chunk,
            finish_file,
        })
    }
}

#[async_trait::async_trait]
impl FileRepository for ScyllaRepository {
    async fn get_file(
        &self,
        project_id: models::ProjectId,
        checksum: &models::FileChecksum,
    ) -> Result<Option<models::File>> {
        let file = self
            .session
            .execute(
                &self.file_statements.get_file,
                (project_id.into_uuid(), checksum.encode(), 0i64),
            )
            .await?
            .maybe_first_row_typed::<File>()?
            .and_then(|file| file.length.map(|_| file))
            .map(Into::into);

        Ok(file)
    }

    async fn get_file_chunks(
        &self,
        project_id: models::ProjectId,
        checksum: models::FileChecksum,
        (range_start, range_end): (u64, u64),
    ) -> Result<Vec<models::FileChunk>> {
        let chunks = self
            .session
            .execute(
                &self.file_statements.get_file_chunks,
                (
                    project_id.into_uuid(),
                    checksum.encode(),
                    0i64,
                    range_start as i64,
                    range_end as i64,
                ),
            )
            .await?
            .rows_typed()?
            .map(|row| {
                let chunk: FileChunk = row?;
                Ok(chunk.into())
            })
            .collect::<Result<Vec<_>>>()?;

        Ok(chunks)
    }

    async fn create_chunk(
        &self,
        project_id: models::ProjectId,
        checksum: &models::FileChecksum,
        length: u64,
        offset: u64,
        data: Vec<u8>,
    ) -> Result<()> {
        let bucket = (offset + data.len() as u64) as i64 / 67_108_864;
        let length = if bucket == 0 {
            MaybeUnset::Unset
        } else {
            MaybeUnset::Set(length as i64)
        };

        self.session
            .execute(
                &self.file_statements.create_chunk,
                (
                    project_id.into_uuid(),
                    checksum.encode(),
                    bucket,
                    length,
                    offset as i64,
                    data,
                ),
            )
            .await?;

        Ok(())
    }

    async fn finish_file(
        &self,
        project_id: models::ProjectId,
        checksum: &models::FileChecksum,
        length: u64,
    ) -> Result<()> {
        self.session
            .execute(
                &self.file_statements.finish_file,
                (
                    length as i64,
                    project_id.into_uuid(),
                    checksum.encode().to_vec(),
                ),
            )
            .await?;

        Ok(())
    }
}
