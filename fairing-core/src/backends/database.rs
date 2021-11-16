use anyhow::Result;
use std::{fmt::Debug, sync::Arc};

use crate::models;

pub type Database = Arc<dyn DatabaseBackend>;

pub trait DatabaseBackend:
    Debug + UserRepository + TeamRepository + SiteRepository + TreeRepository
{
}

impl<T> DatabaseBackend for T where
    T: Debug + UserRepository + TeamRepository + SiteRepository + TreeRepository
{
}

#[async_trait::async_trait]
pub trait UserRepository: Send + Sync {
    async fn get_user(&self, user_name: &models::UserName) -> Result<Option<models::User>>;

    async fn create_user(&self, user: &models::CreateUser) -> Result<models::User>;

    async fn verify_user_password(
        &self,
        user_name: &models::UserName,
        password: models::Password,
    ) -> Result<()>;
}

#[async_trait::async_trait]
pub trait TeamRepository: Send + Sync {
    async fn list_teams(&self, user_name: &models::UserName) -> Result<Vec<models::Team>>;

    async fn get_team(&self, team_name: &models::TeamName) -> Result<Option<models::Team>>;

    async fn create_team(&self, team: &models::CreateTeam) -> Result<models::Team>;

    async fn delete_team(&self, team_name: &models::TeamName) -> Result<()>;

    async fn list_team_members(
        &self,
        team_name: &models::TeamName,
    ) -> Result<Vec<models::TeamMember>>;

    async fn create_team_member(
        &self,
        team_member: &models::CreateTeamMember,
    ) -> Result<models::TeamMember>;

    async fn delete_team_member(&self, team_member_name: &models::TeamMemberName) -> Result<()>;
}

#[async_trait::async_trait]
pub trait SiteRepository: Send + Sync {
    async fn list_sites(&self, team_name: &models::TeamName) -> Result<Vec<models::Site>>;

    async fn get_site(&self, site_name: &models::SiteName) -> Result<Option<models::Site>>;

    async fn create_site(&self, site: &models::CreateSite) -> Result<models::Site>;

    async fn delete_site(&self, site_name: &models::SiteName) -> Result<()>;

    async fn list_site_sources(
        &self,
        site_name: &models::SiteName,
    ) -> Result<Vec<models::SiteSource>>;

    async fn get_site_source(
        &self,
        site_source_name: &models::SiteSourceName,
    ) -> Result<Option<models::SiteSource>>;

    async fn create_site_source(
        &self,
        site_source: &models::CreateSiteSource,
    ) -> Result<models::SiteSource>;
}

#[async_trait::async_trait]
pub trait TreeRepository: Send + Sync {
    async fn list_trees(
        &self,
        site_source_name: &models::SiteSourceName,
    ) -> Result<Vec<models::Tree>>;

    async fn get_tree(&self, tree_name: &models::TreeName) -> Result<Option<models::Tree>>;

    async fn create_tree_revision(
        &self,
        tree_revision: &models::CreateTreeRevision,
    ) -> Result<Option<models::TreeRevision>>;
}
