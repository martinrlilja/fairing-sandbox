use anyhow::{ensure, Result};
use chrono::{DateTime, Utc};
use scylla::{
    frame::value::Timestamp,
    prepared_statement::PreparedStatement,
    statement::{Consistency, SerialConsistency},
    FromRow, Session,
};
use uuid::Uuid;

use fairing_core2::{models, repositories::DomainRepository};

use crate::{
    time::{from_timestamp, to_timestamp},
    ScyllaRepository,
};

#[derive(Debug, FromRow)]
struct Certificate {
    project_id: Uuid,
    name: String,
    domain_names: Vec<String>,
}

impl Into<models::Certificate> for Certificate {
    fn into(self) -> models::Certificate {
        let domain_names = self
            .domain_names
            .into_iter()
            .map(|domain_name| domain_name.parse().unwrap())
            .collect();

        models::Certificate {
            project_id: self.project_id.into(),
            name: self.name,
            domain_names,
        }
    }
}

#[derive(Debug, FromRow)]
struct QueuedCertificate {
    project_id: Uuid,
    name: String,
}

impl Into<models::QueuedCertificate> for QueuedCertificate {
    fn into(self) -> models::QueuedCertificate {
        models::QueuedCertificate {
            project_id: self.project_id.into(),
            name: self.name,
        }
    }
}

#[derive(Debug, FromRow)]
struct ValidatedDomain {
    fqdn: String,
    data: Vec<u8>,
}

impl Into<models::ValidatedDomain> for ValidatedDomain {
    fn into(self) -> models::ValidatedDomain {
        let (data, _) =
            bincode::decode_from_slice(&self.data, bincode::config::standard()).unwrap();

        models::ValidatedDomain {
            fqdn: self.fqdn,
            data,
        }
    }
}

pub(crate) struct Statements {
    create_certificate: PreparedStatement,
    get_certificate: PreparedStatement,
    update_certificate: PreparedStatement,
    process_certificate: PreparedStatement,
    get_queued_certificates: PreparedStatement,
    create_validated_domain: PreparedStatement,
    get_validated_domain: PreparedStatement,
    create_acme_challenge: PreparedStatement,
    get_acme_dns_01_challenges: PreparedStatement,
}

impl Statements {
    pub(crate) async fn prepare(session: &Session) -> Result<Statements> {
        let mut create_certificate = session
            .prepare(
                r"
                INSERT INTO certificates (
                    project_id, bucket, name, domains, next_processing_time
                )
                VALUES (?, ?, ?, ?, ?);
                ",
            )
            .await?;
        create_certificate.set_consistency(Consistency::EachQuorum);

        let mut get_certificate = session
            .prepare(
                r"
                SELECT project_id, name, domains
                FROM certificates
                WHERE project_id = ? AND bucket = ? AND name = ?;
                ",
            )
            .await?;
        get_certificate.set_consistency(Consistency::LocalQuorum);

        let mut update_certificate = session
            .prepare(
                r"
                UPDATE certificates
                SET keys = ?
                WHERE project_id = ? AND bucket = ? AND name = ?;
                ",
            )
            .await?;
        update_certificate.set_consistency(Consistency::LocalQuorum);

        let mut process_certificate = session
            .prepare(
                r"
                UPDATE certificates
                SET next_processing_time = ?
                WHERE project_id = ? AND bucket = ? AND name = ?
                IF next_processing_time = ?;
                ",
            )
            .await?;
        process_certificate.set_serial_consistency(Some(SerialConsistency::Serial));

        let mut get_queued_certificates = session
            .prepare(
                r"
                SELECT project_id, name, domains
                FROM certificate_queue
                WHERE next_processing_time IN ?;
                ",
            )
            .await?;
        get_queued_certificates.set_consistency(Consistency::LocalQuorum);

        let mut create_validated_domain = session
            .prepare(
                r"
                INSERT INTO validated_domains (
                    fqdn, bucket, data
                )
                VALUES (?, ?, ?);
                ",
            )
            .await?;
        create_validated_domain.set_consistency(Consistency::EachQuorum);

        let mut get_validated_domain = session
            .prepare(
                r"
                SELECT fqdn, data
                FROM validated_domains
                WHERE fqdn = ?;
                ",
            )
            .await?;
        get_validated_domain.set_consistency(Consistency::LocalQuorum);

        let create_acme_challenge = session
            .prepare(
                r"
                INSERT INTO acme_challenges (
                    acme_dns_challenge_label, project_id, certificate_name, dns_01_token
                )
                VALUES (?, ?, ?)
                USING TTL ?;
                ",
            )
            .await?;

        let get_acme_dns_01_challenges = session
            .prepare(
                r"
                SELECT dns_01_token
                FROM acme_challenges
                WHERE acme_dns_challenge_label = ?;
                ",
            )
            .await?;

        Ok(Statements {
            create_certificate,
            get_certificate,
            update_certificate,
            process_certificate,
            get_queued_certificates,
            create_validated_domain,
            get_validated_domain,
            create_acme_challenge,
            get_acme_dns_01_challenges,
        })
    }
}

#[async_trait::async_trait]
impl DomainRepository for ScyllaRepository {
    async fn create_domain(&self, domain: models::Domain) -> Result<()> {
        todo!();
    }

    async fn create_certificate(
        &self,
        certificate: &models::Certificate,
        queue_timestamp: DateTime<Utc>,
    ) -> Result<()> {
        let domain_names = certificate
            .domain_names
            .iter()
            .map(|domain_name| domain_name.to_fqdn())
            .collect::<Vec<_>>();

        self.session
            .execute(
                &self.domain_statements.create_certificate,
                (
                    certificate.project_id.into_uuid(),
                    0_i64,
                    &certificate.name,
                    domain_names,
                    to_timestamp(&queue_timestamp),
                ),
            )
            .await?;

        Ok(())
    }

    async fn get_certificate(
        &self,
        project_id: models::ProjectId,
        name: &str,
    ) -> Result<Option<models::Certificate>> {
        let certificate = self
            .session
            .execute(
                &self.domain_statements.get_certificate,
                (project_id.into_uuid(), 0_i64, name),
            )
            .await?
            .maybe_first_row_typed()?
            .map(Certificate::into);

        Ok(certificate)
    }

    async fn update_certificate(
        &self,
        project_id: models::ProjectId,
        name: &str,
        keys: &models::CertificateKeys,
    ) -> Result<()> {
        self.session
            .execute(
                &self.domain_statements.update_certificate,
                (
                    bincode::encode_to_vec(&keys, bincode::config::standard())?,
                    project_id.into_uuid(),
                    0_i64,
                    name,
                ),
            )
            .await?;

        Ok(())
    }

    async fn process_certificate(
        &self,
        project_id: models::ProjectId,
        certificate_name: &str,
        current_timestamp: DateTime<Utc>,
        timestamp: DateTime<Utc>,
    ) -> Result<Option<models::CertificateRenewal>> {
        let (applied, _next_processing_time): (bool, Timestamp) = self
            .session
            .execute(
                &self.domain_statements.process_certificate,
                (
                    to_timestamp(&timestamp),
                    project_id.into_uuid(),
                    0_i64,
                    certificate_name,
                    to_timestamp(&current_timestamp),
                ),
            )
            .await?
            .first_row_typed()?;

        ensure!(applied, "new layer id is older than previous layer id");

        Ok()
    }

    async fn update_certificate_renewal(
        &self,
        project_id: models::ProjectId,
        certificate_name: &str,
        certificate_renewal: Option<models::CertificateRenewal>,
        current_timestamp: DateTime<Utc>,
        timestamp: DateTime<Utc>,
    ) -> Result<()> {
        todo!();
    }

    async fn get_queued_certificates(
        &self,
        timestamps: &[DateTime<Utc>],
    ) -> Result<Vec<models::QueuedCertificate>> {
        let queued_certificates = self
            .session
            .execute(
                &self.domain_statements.get_queued_certificates,
                timestamps.iter().map(to_timestamp).collect::<Vec<_>>(),
            )
            .await?
            .rows_typed()?
            .map(|row| {
                let row: QueuedCertificate = row?;
                Ok(row.into())
            })
            .collect::<Result<Vec<_>>>()?;

        Ok(queued_certificates)
    }

    async fn create_acme_challenge(&self, challenge: models::AcmeChallenge) -> Result<()> {
        self.session
            .execute(
                &self.domain_statements.create_acme_challenge,
                (
                    challenge.acme_dns_challenge_label,
                    challenge.project_id.into_uuid(),
                    challenge.certificate_name,
                    challenge.dns_01_token,
                    challenge.ttl.num_seconds(),
                ),
            )
            .await?;

        Ok(())
    }

    async fn get_acme_dns_01_challenges(
        &self,
        acme_dns_challenge_label: &str,
    ) -> Result<Vec<String>> {
        let dns_01_tokens = self
            .session
            .execute(
                &self.domain_statements.get_acme_dns_01_challenges,
                (acme_dns_challenge_label,),
            )
            .await?
            .rows_typed()?
            .map(|row| {
                let (dns_01_token,) = row?;
                Ok(dns_01_token)
            })
            .collect::<Result<Vec<_>>>()?;

        Ok(dns_01_tokens)
    }

    async fn create_validated_domain(
        &self,
        validated_domain: &models::ValidatedDomain,
    ) -> Result<()> {
        self.session
            .execute(
                &self.domain_statements.create_validated_domain,
                (
                    &validated_domain.fqdn,
                    0_i64,
                    bincode::encode_to_vec(&validated_domain.data, bincode::config::standard())?,
                ),
            )
            .await?;

        Ok(())
    }

    async fn get_validated_domain(&self, fqdn: &str) -> Result<Option<models::ValidatedDomain>> {
        let validated_domain = self
            .session
            .execute(&self.domain_statements.get_validated_domain, (fqdn, 0_i64))
            .await?
            .maybe_first_row_typed::<ValidatedDomain>()?
            .map(Into::into);

        Ok(validated_domain)
    }
}
