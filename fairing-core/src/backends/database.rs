use anyhow::Result;
use std::{fmt::Debug, sync::Arc};

use crate::models;

pub type Database = Arc<dyn DatabaseBackend>;

pub trait DatabaseBackend:
    Debug
    + UserRepository
    + TeamRepository
    + SourceRepository
    + SiteRepository
    + DeploymentRepository
    + DomainRepository
    + LayerRepository
{
}

impl<T> DatabaseBackend for T where
    T: Debug
        + UserRepository
        + TeamRepository
        + SourceRepository
        + SiteRepository
        + DeploymentRepository
        + DomainRepository
        + LayerRepository
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
pub trait SourceRepository: Send + Sync {
    async fn list_sources(&self, team_name: &models::TeamName) -> Result<Vec<models::Source>>;

    async fn get_source(&self, source_name: &models::SourceName) -> Result<Option<models::Source>>;

    async fn create_source(&self, source: &models::CreateSource) -> Result<models::Source>;
}

#[async_trait::async_trait]
pub trait SiteRepository: Send + Sync {
    async fn list_sites(&self, team_name: &models::TeamName) -> Result<Vec<models::Site>>;

    async fn list_sites_with_base_source(
        &self,
        source_name: &models::SourceName,
    ) -> Result<Vec<models::Site>>;

    async fn get_site(&self, site_name: &models::SiteName) -> Result<Option<models::Site>>;

    async fn create_site(&self, site: &models::CreateSite) -> Result<models::Site>;

    async fn delete_site(&self, site_name: &models::SiteName) -> Result<()>;

    async fn update_current_deployment(
        &self,
        deployment_name: &models::DeploymentName,
    ) -> Result<()>;
}

#[async_trait::async_trait]
pub trait DeploymentRepository: Send + Sync {
    async fn get_deployment(
        &self,
        deployment_name: &models::DeploymentName,
    ) -> Result<Option<models::Deployment>>;

    async fn create_deployment(
        &self,
        deployment: &models::CreateDeployment,
    ) -> Result<models::Deployment>;

    async fn get_deployment_by_host(
        &self,
        lookup: &models::DeploymentHostLookup,
    ) -> Result<Option<Vec<models::DeploymentProjectionAsdf>>>;
}

#[async_trait::async_trait]
pub trait DomainRepository: Send + Sync {
    async fn create_domain(&self, domain: &models::CreateDomain) -> Result<models::Domain>;

    async fn set_domain_site(
        &self,
        domain: &models::DomainName,
        site: &models::SiteName,
    ) -> Result<()>;

    async fn create_certificate(
        &self,
        certificate: &models::CreateCertificate,
    ) -> Result<models::Certificate>;

    async fn get_certificate(&self, domain: &str) -> Result<Option<models::Certificate>>;

    async fn create_acme_order(&self, acme_order: &models::CreateAcmeOrder) -> Result<()>;

    async fn get_domain_acme_challenge(
        &self,
        acme_label: &str,
    ) -> Result<Option<models::AcmeChallenge>>;

    async fn get_domain_needing_new_certificate(&self) -> Result<Option<models::Domain>>;
}

#[async_trait::async_trait]
pub trait LayerRepository: Send + Sync {
    async fn list_layer_sets(
        &self,
        source_name: &models::SourceName,
    ) -> Result<Vec<models::LayerSet>>;

    async fn get_layer_set(
        &self,
        layer_set_name: &models::LayerSetName,
    ) -> Result<Option<models::LayerSet>>;

    async fn create_build(&self, build: &models::CreateBuild) -> Result<models::Build>;
}
