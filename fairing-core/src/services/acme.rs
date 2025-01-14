use anyhow::{anyhow, Context as _, Result};
use chrono::{TimeZone, Utc};
use std::time::Duration;
use x509_parser::prelude::*;

use crate::{
    backends::Database,
    models::{self, prelude::*},
};
use fairing_acme::{
    AcmeClientBackend, AcmeClientWithAccount, AuthorizationStatus, CreateOrder, FinalizeOrder,
    Identifier, IdentifierType, OrderStatus,
};

pub struct AcmeService<Backend> {
    database: Database,
    client: AcmeClientWithAccount<Backend>,
}

impl<Backend: AcmeClientBackend + Send> AcmeService<Backend> {
    pub fn new(database: Database, client: AcmeClientWithAccount<Backend>) -> AcmeService<Backend> {
        AcmeService { database, client }
    }

    pub async fn run(mut self) -> Result<()> {
        loop {
            let domain = self.database.get_domain_needing_new_certificate().await?;

            if let Some(domain) = domain {
                if let Err(err) = self.process_domain(domain).await {
                    tracing::error!("failed to process domain: {:?}", err);
                }
            }

            tokio::time::sleep(Duration::from_secs(30)).await;
        }
    }

    async fn process_domain(&mut self, domain: models::Domain) -> Result<()> {
        // TODO: validate the dns records before creating an order.
        tracing::trace!("processing domain {}", domain.name.name());

        let order = self
            .client
            .create_order(&CreateOrder {
                identifiers: vec![Identifier {
                    type_: IdentifierType::Dns,
                    value: domain.name.resource().into(),
                }],
            })
            .await
            .context("create order")?;

        let mut authorizations = vec![];

        for authorization_url in order.authorizations.iter() {
            let authorization = self
                .client
                .get_authorization(&authorization_url)
                .await
                .context("get authorization")?;
            authorizations.push(authorization);
        }

        self.database
            .create_acme_order(&models::CreateAcmeOrder {
                parent: domain.name.parent(),
                order: order.clone(),
                authorizations: authorizations.clone(),
            })
            .await?;

        for authorization in authorizations {
            if let AuthorizationStatus::Valid = authorization.status {
                continue;
            }

            let dns_challenge = authorization
                .challenges
                .iter()
                .find(|challenge| challenge.type_ == "dns-01");

            if let Some(dns_challenge) = dns_challenge {
                tracing::trace!("responding to dns-01 challenge");
                self.client
                    .accept_challenge(&dns_challenge.url)
                    .await
                    .context("accept challenge")?;
            }
        }

        let certificate_params = {
            let mut certificate_params =
                rcgen::CertificateParams::new(vec![domain.name.resource().into()]);
            certificate_params.distinguished_name = rcgen::DistinguishedName::new();
            certificate_params
                .distinguished_name
                .push(rcgen::DnType::CommonName, domain.name.resource());

            certificate_params
        };

        let certificate = rcgen::Certificate::from_params(certificate_params)?;

        let csr = certificate.serialize_request_der()?;

        tracing::trace!("waiting for order to not be pending");

        let mut order = self
            .client
            .get_order(&order.url)
            .await
            .context("get order")?;

        while let OrderStatus::Pending = order.status {
            tokio::time::sleep(Duration::from_secs(10)).await;

            order = self
                .client
                .get_order(&order.url)
                .await
                .context("get order")?;
        }

        if let OrderStatus::Invalid = order.status {
            return Err(anyhow!("order is invalid: {:?}", order.error));
        }

        tracing::trace!("finalizing order");

        let mut order = self
            .client
            .finalize_order(
                &order.finalize,
                &FinalizeOrder {
                    csr: base64::encode_config(&csr, base64::URL_SAFE_NO_PAD),
                },
            )
            .await
            .context("finalize order")?;

        tracing::trace!("waiting for certificate to be ready");

        while let OrderStatus::Processing = order.status {
            tokio::time::sleep(Duration::from_secs(10)).await;

            order = self
                .client
                .get_order(&order.url)
                .await
                .context("get order")
                .context("get order")?;
        }

        if let Some(certificate_url) = order.certificate {
            tracing::trace!("getting certificate");

            let acme_certificate = self
                .client
                .download_certificate(&certificate_url)
                .await
                .context("download certificate")?;

            let public_key_chain = rustls_pemfile::read_all(&mut acme_certificate.as_bytes())?
                .into_iter()
                .flat_map(|item| match item {
                    rustls_pemfile::Item::X509Certificate(public_key) => Some(public_key),
                    _ => None,
                })
                .collect::<Vec<_>>();

            let (_, public_key) = X509Certificate::from_der(&public_key_chain.first().unwrap())?;
            let validity = public_key.validity();

            let certificate = models::CreateCertificate {
                parent: domain.name,
                expires_time: Utc.timestamp(validity.not_after.timestamp(), 0),
                private_key: certificate.serialize_private_key_der(),
                public_key_chain,
            };

            self.database.create_certificate(&certificate).await?;

            tracing::trace!("certificate created");
        } else {
            return Err(anyhow!("order is invalid: {:?}", order.error));
        }

        Ok(())
    }
}
