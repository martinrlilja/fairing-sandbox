use anyhow::Result;
use scylla::FromRow;
use uuid::Uuid;

use fairing_core2::{models, repositories::ProjectRepository};

use crate::ScyllaRepository;

#[derive(Debug, FromRow)]
struct Project {
    id: Uuid,
    acme_dns_challenge_label: String,
    file_encryption_key: Vec<u8>,
}

impl Into<models::Project> for Project {
    fn into(self) -> models::Project {
        models::Project {
            id: self.id.into(),
            acme_dns_challenge_label: self.acme_dns_challenge_label,
            file_encryption_key: self.file_encryption_key,
        }
    }
}

#[async_trait::async_trait]
impl ProjectRepository for ScyllaRepository {
    async fn get_project(&self, id: &models::ProjectId) -> Result<Option<models::Project>> {
        let project = self
            .session
            .query(
                r"
                SELECT id, acme_dns_challenge_label, file_encryption_key
                FROM projects
                WHERE id = ?;
                ",
                (id.into_uuid(),),
            )
            .await?
            .maybe_first_row_typed::<Project>()?
            .map(Into::into);

        Ok(project)
    }

    async fn create_or_update_project(&self, project: &models::Project) -> Result<()> {
        self.session
            .query(
                r"
                UPDATE projects
                SET acme_dns_challenge_label = ?,
                    file_encryption_key = ?
                WHERE id = ?;
                ",
                (
                    &project.acme_dns_challenge_label,
                    &project.file_encryption_key,
                    project.id.into_uuid(),
                ),
            )
            .await?;

        Ok(())
    }
}
