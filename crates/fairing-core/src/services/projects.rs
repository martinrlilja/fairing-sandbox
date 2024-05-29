use anyhow::Result;
use getrandom::getrandom;
use uuid::Uuid;

use super::auth::{Authentication, ProjectPermissions};
use crate::{models, repositories::ProjectRepository};

pub struct ProjectService {
    repository: &'static dyn ProjectRepository,
}

impl ProjectService {
    pub fn new(repository: &'static dyn ProjectRepository) -> ProjectService {
        ProjectService { repository }
    }

    pub async fn get_project(&self, auth: &Authentication) -> Result<Option<models::Project>> {
        auth.can(ProjectPermissions::Get)?;
        let project_id = auth.project_id()?;

        let project = self.repository.get_project(&project_id).await?;

        Ok(project)
    }

    pub async fn create_project(
        &self,
        auth: &Authentication,
        _project: &models::CreateProject,
    ) -> Result<models::Project> {
        auth.can(ProjectPermissions::Create)?;

        let acme_dns_challenge_label = {
            let mut label = [0u8; 12];
            getrandom(&mut label)?;
            hex::encode(label)
        };

        let file_encryption_key = {
            let mut key = [0u8; 32];
            getrandom(&mut key)?;
            key.to_vec()
        };

        let project = models::Project {
            id: Uuid::new_v4().into(),
            acme_dns_challenge_label,
            file_encryption_key,
        };

        self.repository.create_or_update_project(&project).await?;

        Ok(project)
    }
}
