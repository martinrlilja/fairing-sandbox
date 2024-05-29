use anyhow::Result;
use chrono::{DateTime, Utc};

use crate::models;

#[async_trait::async_trait]
pub trait DomainRepository: Send + Sync {
    async fn create_domain(&self, domain: models::Domain) -> Result<()>;

    async fn create_certificate(
        &self,
        certificate: &models::Certificate,
        queue_timestamp: DateTime<Utc>,
    ) -> Result<()>;

    async fn get_certificate(
        &self,
        project_id: models::ProjectId,
        name: &str,
    ) -> Result<Option<models::Certificate>>;

    async fn update_certificate(
        &self,
        project_id: models::ProjectId,
        name: &str,
        keys: &models::CertificateKeys,
    ) -> Result<()>;

    async fn process_certificate(
        &self,
        project_id: models::ProjectId,
        certificate_name: &str,
        current_timestamp: DateTime<Utc>,
        timestamp: DateTime<Utc>,
    ) -> Result<Option<models::CertificateRenewal>>;

    async fn update_certificate_renewal(
        &self,
        project_id: models::ProjectId,
        certificate_name: &str,
        certificate_renewal: Option<models::CertificateRenewal>,
        current_timestamp: DateTime<Utc>,
        timestamp: DateTime<Utc>,
    ) -> Result<()>;

    async fn get_queued_certificates(
        &self,
        timestamps: &[DateTime<Utc>],
    ) -> Result<Vec<models::QueuedCertificate>>;

    async fn create_acme_challenge(&self, challenge: models::AcmeChallenge) -> Result<()>;

    async fn get_acme_dns_01_challenges(
        &self,
        acme_dns_challenge_label: &str,
    ) -> Result<Vec<String>>;

    async fn create_validated_domain(
        &self,
        validated_domain: &models::ValidatedDomain,
    ) -> Result<()>;

    async fn get_validated_domain(&self, fqdn: &str) -> Result<Option<models::ValidatedDomain>>;
}
