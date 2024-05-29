use anyhow::{anyhow, Result};
use scylla::FromRow;
use uuid::Uuid;

use fairing_core2::{models, repositories::SourceRepository};

use crate::ScyllaRepository;

#[derive(Debug, FromRow)]
struct Source {
    project_id: Uuid,
    name: String,
    git_repository_url: Option<String>,
    git_ed25519_secret_key: Option<Vec<u8>>,
}

impl Into<models::Source> for Source {
    fn into(self) -> models::Source {
        let kind = match self {
            Source {
                git_repository_url: Some(repository_url),
                git_ed25519_secret_key: Some(ed25519_secret_key),
                ..
            } => models::SourceKind::Git {
                repository_url: repository_url.parse().unwrap(),
                id_ed25519: models::Ed25519::from_row(ed25519_secret_key),
            },
            _ => unreachable!("unknown source kind"),
        };

        models::Source {
            project_id: self.project_id.into(),
            name: self.name.parse().unwrap(),
            kind,
        }
    }
}

#[async_trait::async_trait]
impl SourceRepository for ScyllaRepository {
    async fn get_source(
        &self,
        project_id: &models::ProjectId,
        name: &models::SourceName,
    ) -> Result<Option<models::Source>> {
        let source = self
            .session
            .query(
                r"
                SELECT project_id, name, git_repository_url, git_ed25519_secret_key
                FROM sources
                WHERE project_id = ? AND bucket = ? AND name = ?;
                ",
                (project_id.into_uuid(), 0i64, name.as_str()),
            )
            .await?
            .maybe_first_row_typed::<Source>()?
            .map(Into::into);

        Ok(source)
    }

    async fn list_sources(&self, project_id: &models::ProjectId) -> Result<Vec<models::Source>> {
        let sources = self
            .session
            .query(
                r"
                SELECT project_id, name, git_repository_url, git_ed25519_secret_key
                FROM sources
                WHERE project_id = ? AND bucket = ?;
                ",
                (project_id.into_uuid(), 0i64),
            )
            .await?
            .rows_typed()?
            .map(|row| row.map(Source::into).map_err(|err| anyhow!("{:?}", err)))
            .collect::<Result<Vec<_>>>()?;

        Ok(sources)
    }

    async fn create_or_update_source(&self, source: &models::Source) -> Result<()> {
        match source.kind {
            models::SourceKind::Git {
                ref repository_url,
                ref id_ed25519,
            } => {
                self.session
                    .query(
                        r"
                        UPDATE sources
                        SET git_repository_url = ?,
                            git_ed25519_secret_key = ?
                        WHERE project_id = ? AND bucket = ? AND name = ?;
                        ",
                        (
                            repository_url.as_str(),
                            id_ed25519.secret_key_to_slice().to_vec(),
                            source.project_id.into_uuid(),
                            0_i64,
                            source.name.as_str(),
                        ),
                    )
                    .await?;
            }
        }

        Ok(())
    }
}
