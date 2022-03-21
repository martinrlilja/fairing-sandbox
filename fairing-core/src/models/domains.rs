use anyhow::{anyhow, Error, Result};
use chrono::{DateTime, Utc};

use crate::models::{
    self,
    prelude::*,
    resource_name::{validators, ParentedResourceName, ResourceNameInner},
};

crate::impl_resource_name! {
    pub struct DomainName<'n>;
}

impl<'n> ParentedResourceName<'n> for DomainName<'n> {
    const COLLECTION: &'static str = "domains";

    type Validator = validators::DomainNameValidator;

    type Parent = crate::models::TeamName<'static>;
}

#[derive(Clone, Debug, sqlx::FromRow)]
pub struct Domain {
    pub name: DomainName<'static>,
    pub created_time: DateTime<Utc>,
    pub acme_label: String,
    pub is_validated: bool,
}

pub struct CreateDomain<'a> {
    pub parent: models::TeamName<'static>,
    pub resource_id: &'a str,
}

impl<'a> CreateDomain<'a> {
    pub fn create(&self) -> Result<Domain> {
        let name = format!("{}/domains/{}", self.parent.name(), self.resource_id);
        let name = DomainName::parse(name)?;

        let acme_label = uuid::Uuid::new_v4().to_simple().to_string();

        Ok(Domain {
            name,
            created_time: Utc::now(),
            acme_label,
            is_validated: false,
        })
    }
}

#[derive(sqlx::FromRow)]
pub struct Certificate {
    pub created_time: DateTime<Utc>,
    pub expires_time: DateTime<Utc>,
    pub private_key: Vec<u8>,
    pub public_key_chain: Vec<Vec<u8>>,
}

pub struct CreateCertificate {
    pub parent: models::DomainName<'static>,
    pub expires_time: DateTime<Utc>,
    pub private_key: Vec<u8>,
    pub public_key_chain: Vec<Vec<u8>>,
}

impl CreateCertificate {
    pub fn create(&self) -> Result<(models::DomainName<'static>, Certificate)> {
        Ok((
            self.parent.clone(),
            Certificate {
                created_time: Utc::now(),
                expires_time: self.expires_time,
                private_key: self.private_key.clone(),
                public_key_chain: self.public_key_chain.clone(),
            },
        ))
    }
}

#[derive(Copy, Clone, Debug, sqlx::Type)]
#[sqlx(type_name = "acme_order_status")]
#[sqlx(rename_all = "snake_case")]
pub enum AcmeOrderStatus {
    Pending,
    Ready,
    Processing,
    Valid,
    Invalid,
}

impl From<fairing_acme::OrderStatus> for AcmeOrderStatus {
    fn from(order_status: fairing_acme::OrderStatus) -> AcmeOrderStatus {
        use fairing_acme::OrderStatus::*;
        match order_status {
            Pending => AcmeOrderStatus::Pending,
            Ready => AcmeOrderStatus::Ready,
            Processing => AcmeOrderStatus::Processing,
            Valid => AcmeOrderStatus::Valid,
            Invalid => AcmeOrderStatus::Invalid,
        }
    }
}

#[derive(sqlx::FromRow)]
pub struct AcmeOrder {
    pub status: AcmeOrderStatus,
    pub created_time: DateTime<Utc>,
    pub expires_time: DateTime<Utc>,
    pub url: String,
}

#[derive(sqlx::FromRow)]
pub struct AcmeChallenge {
    pub domain: models::DomainName<'static>,
    pub dns_01_token: String,
}

pub struct CreateAcmeOrder {
    pub parent: models::TeamName<'static>,
    pub order_url: String,
    pub order: fairing_acme::Order,
    pub authorizations: Vec<fairing_acme::Authorization>,
}

impl CreateAcmeOrder {
    pub fn create(&self) -> Result<(models::TeamName<'static>, AcmeOrder, Vec<AcmeChallenge>)> {
        let challenges = self
            .authorizations
            .iter()
            .map(|authorization| {
                let dns_challenge = authorization
                    .challenges
                    .iter()
                    .find(|&challenge| challenge.type_ == "dns-01")
                    .ok_or_else(|| anyhow!("authorization does not accept dns-01 validation"))?;

                let domain = format!(
                    "{}/domains/{}",
                    self.parent.name(),
                    authorization.identifier.value
                );
                let domain = DomainName::parse(domain)?;

                Ok::<_, Error>(AcmeChallenge {
                    domain,
                    dns_01_token: dns_challenge.token.clone(),
                })
            })
            .collect::<Result<Vec<_>, _>>()?;

        let expires_time = DateTime::parse_from_rfc3339(&self.order.expires)?.with_timezone(&Utc);

        Ok((
            self.parent.clone(),
            AcmeOrder {
                status: self.order.status.into(),
                created_time: Utc::now(),
                expires_time,
                url: self.order_url.clone(),
            },
            challenges,
        ))
    }
}
