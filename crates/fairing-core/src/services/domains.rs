use anyhow::{Context as _, Result};
use chrono::{Duration, Timelike, Utc};
use tokio::sync::Mutex;
use trust_dns_proto::rr::RecordType;
use x509_parser::prelude::*;

use super::auth::Authentication;
use crate::{
    models,
    repositories::{DomainRepository, ProjectRepository},
};
use fairing_acme::{Order, OrderStatus};

pub struct DomainService<'a, AcmeBackend> {
    domain_repository: &'a dyn DomainRepository,
    project_repository: &'a dyn ProjectRepository,
    acme_client: Mutex<fairing_acme::AcmeClientWithAccount<AcmeBackend>>,
}

impl<'a, AcmeBackend: fairing_acme::AcmeClientBackend + Send> DomainService<'a, AcmeBackend> {
    pub fn new(
        domain_repository: &'a dyn DomainRepository,
        project_repository: &'a dyn ProjectRepository,
        acme_client: fairing_acme::AcmeClientWithAccount<AcmeBackend>,
    ) -> DomainService<'a, AcmeBackend> {
        DomainService {
            domain_repository,
            project_repository,
            acme_client: Mutex::new(acme_client),
        }
    }

    pub async fn get_acme_dns_01_challenges(
        &self,
        acme_dns_challenge_label: &str,
    ) -> Result<Vec<String>> {
        self.domain_repository
            .get_acme_dns_01_challenges(acme_dns_challenge_label)
            .await
    }

    pub async fn process_certificates(&self) -> Result<()> {
        let dns_resolver = trust_dns_resolver::AsyncResolver::tokio_from_system_conf()?;
        let mut acme_client = self.acme_client.lock().await;

        let now = Utc::now();
        let today = now.date();
        let time = now.time();

        let current_minute = today.and_hms(time.hour(), time.minute(), 0);

        let queue_lookback_minutes = 60;
        let timestamps = (-queue_lookback_minutes..0)
            .map(|offset| current_minute - Duration::minutes(offset))
            .collect::<Vec<_>>();

        let certificates = self
            .domain_repository
            .get_queued_certificates(&timestamps)
            .await?;

        for certificate in certificates {
            let fallback_process_time = current_minute + Duration::minutes(5);
            let renewal = self
                .domain_repository
                .process_certificate(
                    certificate.project_id,
                    &certificate.name,
                    current_minute,
                    fallback_process_time,
                )
                .await?;

            let project = self
                .project_repository
                .get_project(&certificate.project_id)
                .await?
                .unwrap();

            if let Some(renewal) = renewal {
                let order = acme_client.get_order(&renewal.acme_order_url).await?;

                match order {
                    Order {
                        status: OrderStatus::Pending,
                        ..
                    } => (),
                    Order {
                        status: OrderStatus::Invalid,
                        ..
                    } => {
                        tracing::error!("order is invalid: {:?}", order.error);

                        self.domain_repository
                            .update_certificate_renewal(
                                certificate.project_id,
                                &certificate.name,
                                None,
                                fallback_process_time,
                                current_minute + Duration::days(1),
                            )
                            .await?;
                    }
                    Order {
                        status: OrderStatus::Valid,
                        ..
                    } => {
                        acme_client
                            .finalize_order(
                                &order.finalize,
                                &fairing_acme::FinalizeOrder {
                                    csr: base64::encode_config(
                                        &renewal.csr,
                                        base64::URL_SAFE_NO_PAD,
                                    ),
                                },
                            )
                            .await
                            .context("finalize order")?;
                    }
                    Order {
                        status: OrderStatus::Processing,
                        ..
                    } => (),
                    Order {
                        status: OrderStatus::Ready,
                        certificate: Some(certificate_url),
                        ..
                    } => {
                        tracing::trace!("getting certificate");

                        let acme_certificate = acme_client
                            .download_certificate(&certificate_url)
                            .await
                            .context("download certificate")?;

                        let public_key_chain =
                            rustls_pemfile::read_all(&mut acme_certificate.as_bytes())?
                                .into_iter()
                                .flat_map(|item| match item {
                                    rustls_pemfile::Item::X509Certificate(public_key) => {
                                        Some(public_key)
                                    }
                                    _ => None,
                                })
                                .collect::<Vec<_>>();

                        let (_, public_key) =
                            X509Certificate::from_der(&public_key_chain.first().unwrap())?;
                        let validity = public_key.validity();

                        let time_to_expiration = validity.time_to_expiration().unwrap();
                        let time_to_expiration =
                            Duration::seconds(time_to_expiration.whole_seconds());
                        let renew_within = time_to_expiration - time_to_expiration / 3;
                        let renew_on = now + renew_within;

                        let keys = models::CertificateKeys {
                            private_key: renewal.csr_secret_key.clone(),
                            public_keys: public_key_chain,
                        };

                        self.domain_repository
                            .update_certificate(certificate.project_id, &certificate.name, &keys)
                            .await?;

                        let certificate = self
                            .domain_repository
                            .get_certificate(certificate.project_id, &certificate.name)
                            .await?
                            .unwrap();

                        for fqdn in certificate.domain_names {
                            self.domain_repository
                                .create_validated_domain(&models::ValidatedDomain {
                                    fqdn: fqdn.to_fqdn(),
                                    data: models::ValidatedDomainData {
                                        project_id: certificate.project_id,
                                        keys: keys.clone(),
                                    },
                                })
                                .await?;
                        }

                        self.domain_repository
                            .update_certificate_renewal(
                                certificate.project_id,
                                &certificate.name,
                                None,
                                fallback_process_time,
                                renew_on,
                            )
                            .await?;
                    }
                    Order {
                        status: OrderStatus::Ready,
                        certificate: None,
                        ..
                    } => {
                        tracing::error!("order couldn't create a certificate: {:?}", order.error);

                        self.domain_repository
                            .update_certificate_renewal(
                                certificate.project_id,
                                &certificate.name,
                                None,
                                fallback_process_time,
                                current_minute + Duration::days(1),
                            )
                            .await?;
                    }
                }
            } else {
                let certificate = self
                    .domain_repository
                    .get_certificate(certificate.project_id, &certificate.name)
                    .await?
                    .unwrap();

                // Check that the dns is correctly setup before creating acme order.
                for domain_name in certificate.domain_names.iter() {
                    let acme = format!("_acme_challenge.{}", domain_name.to_fqdn());
                    let lookup = dns_resolver.lookup(&acme, RecordType::TXT).await?;
                    for record in lookup.record_iter() {
                        tracing::info!("{record:?}");
                    }
                }

                // TODO: If not change renew_on to a time in the future.

                // Create acme order
                let order = acme_client
                    .create_order(&fairing_acme::CreateOrder {
                        identifiers: certificate
                            .domain_names
                            .iter()
                            .map(|name| fairing_acme::Identifier {
                                type_: fairing_acme::IdentifierType::Dns,
                                value: name.to_fqdn(),
                            })
                            .collect(),
                    })
                    .await?;

                for authorization_url in order.authorizations.iter() {
                    let authorization = acme_client
                        .get_authorization(&authorization_url)
                        .await
                        .context("get authorization")?;

                    if let fairing_acme::AuthorizationStatus::Valid = authorization.status {
                        continue;
                    }

                    let dns_challenge = authorization
                        .challenges
                        .iter()
                        .find(|challenge| challenge.type_ == "dns-01");

                    if let Some(dns_challenge) = dns_challenge {
                        self.domain_repository
                            .create_acme_challenge(models::AcmeChallenge {
                                acme_dns_challenge_label: project.acme_dns_challenge_label.clone(),
                                project_id: project.id,
                                certificate_name: certificate.name.clone(),
                                dns_01_token: dns_challenge.token.clone(),
                                ttl: chrono::Duration::minutes(60),
                            })
                            .await?;

                        acme_client.accept_challenge(&dns_challenge.url).await?;
                    }
                }

                let certificate_params = {
                    let subject_alt_names = certificate
                        .domain_names
                        .iter()
                        .map(models::DomainName::to_fqdn)
                        .collect::<Vec<_>>();
                    let first_name = certificate
                        .domain_names
                        .first()
                        .map(models::DomainName::to_fqdn);

                    let mut certificate_params = rcgen::CertificateParams::new(subject_alt_names);

                    let mut distinguished_name = rcgen::DistinguishedName::new();
                    distinguished_name.push(rcgen::DnType::CommonName, first_name.unwrap());

                    certificate_params.distinguished_name = distinguished_name;
                    certificate_params
                };

                let cert = rcgen::Certificate::from_params(certificate_params)?;

                let csr_secret_key = cert.serialize_private_key_der();

                let csr = cert.serialize_request_der()?;

                self.domain_repository
                    .update_certificate_renewal(
                        certificate.project_id,
                        &certificate.name,
                        Some(models::CertificateRenewal {
                            acme_order_url: order.url,
                            csr,
                            csr_secret_key,
                        }),
                        fallback_process_time,
                        current_minute + Duration::days(1),
                    )
                    .await?;
            }
        }

        Ok(())
    }
}
