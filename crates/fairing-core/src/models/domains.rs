use anyhow::{ensure, Result};
use std::str::FromStr;

use super::{LayerId, LayerSetName, ProjectId};

pub struct DomainName(trust_dns_proto::rr::Name);

impl DomainName {
    pub fn to_fqdn(&self) -> String {
        self.0.to_ascii()
    }

    pub fn to_fqdn_without_trailing_dot(&self) -> String {
        let mut s = self.to_fqdn();
        s.pop();
        s
    }
}

impl FromStr for DomainName {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<DomainName> {
        let mut name = trust_dns_proto::rr::Name::from_utf8(s)?;
        name.set_fqdn(true);
        name.to_lowercase();

        ensure!(name.num_labels() >= 1);
        ensure!(!name.is_root());
        ensure!(!name.is_localhost());

        Ok(DomainName(name))
    }
}

#[derive(Clone, Debug)]
pub struct Domain {
    pub project_id: ProjectId,
    pub fqdn: String,
    pub kind: DomainKind,
}

#[derive(Clone, Debug)]
pub enum DomainKind {
    Layer,
    WildCard { kind: WildCardKind },
}

#[derive(Clone, Debug)]
pub struct ValidatedDomain {
    pub fqdn: String,
    pub data: ValidatedDomainData,
}

#[derive(Clone, Debug, bincode::Encode, bincode::Decode)]
pub struct ValidatedDomainData {
    pub project_id: ProjectId,
    pub keys: CertificateKeys,
}

#[derive(Clone, Debug)]
pub struct QueuedCertificate {
    pub project_id: ProjectId,
    pub name: String,
}

pub struct Certificate {
    pub project_id: ProjectId,
    pub name: String,

    pub domain_names: Vec<DomainName>,
}

pub struct CertificateRenewal {
    pub acme_order_url: String,
    pub csr: Vec<u8>,
    pub csr_secret_key: Vec<u8>,
}

#[derive(Clone, Debug)]
pub struct CompactCertificateKeys(Vec<u8>);

#[derive(Clone, Debug, bincode::Encode, bincode::Decode)]
pub struct CertificateKeys {
    pub private_key: Vec<u8>,
    pub public_keys: Vec<Vec<u8>>,
}

#[derive(Clone, Debug, bincode::Encode, bincode::Decode)]
pub enum CertificateSigningRequestStatus {
    Pending,
    Ready,
    Processing,
    Valid,
    Invalid,
}

#[derive(Clone, Debug)]
pub struct AcmeChallenge {
    pub acme_dns_challenge_label: String,
    pub project_id: ProjectId,
    pub certificate_name: String,
    pub dns_01_token: String,
    pub ttl: chrono::Duration,
}

// TODO: delete
#[derive(Clone, Debug)]
pub enum ValidatedDomainKind {
    Parked,
    Layer {
        layer_set_name: LayerSetName,
        layer_id: LayerId,
    },
    WildCard {
        kind: WildCardKind,
    },
}

#[derive(Clone, Debug, bincode::Encode, bincode::Decode)]
pub enum WildCardKind {
    Private,
    Public,
}

#[derive(Clone, Debug)]
pub struct ValidatedDomainCertificate {
    pub private_key: Vec<u8>,
    pub public_key_chain: Vec<Vec<u8>>,
}
