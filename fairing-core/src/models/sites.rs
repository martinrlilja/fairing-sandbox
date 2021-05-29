use anyhow::Result;
use chrono::{DateTime, Utc};

use crate::models::{
    self,
    prelude::*,
    resource_name::{validators, ParentedResourceName, ResourceNameInner},
};

crate::impl_resource_name! {
    pub struct SiteName<'n>;
    pub struct SiteSourceName<'n>;
}

impl<'n> ParentedResourceName<'n> for SiteName<'n> {
    const COLLECTION: &'static str = "sites";

    type Validator = validators::DomainLabelValidator;

    type Parent = crate::models::TeamName<'static>;
}

impl<'n> ParentedResourceName<'n> for SiteSourceName<'n> {
    const COLLECTION: &'static str = "sources";

    type Validator = validators::UnicodeIdentifierValidator;

    type Parent = crate::models::SiteName<'static>;
}

#[derive(sqlx::FromRow)]
pub struct Site {
    pub name: SiteName<'static>,
    pub created_time: DateTime<Utc>,
}

pub struct CreateSite<'a> {
    pub resource_id: &'a str,
    pub parent: models::TeamName<'static>,
}

impl<'a> CreateSite<'a> {
    pub fn create(&self) -> Result<Site> {
        let name = format!("{}/sites/{}", self.parent.name(), self.resource_id);
        let name = SiteName::parse(name)?;

        Ok(Site {
            name,
            created_time: Utc::now(),
        })
    }
}

pub struct SiteSource {
    pub name: SiteSourceName<'static>,
    pub created_time: DateTime<Utc>,
    pub hook_token: String,
    pub kind: Option<SiteSourceKind>,
}

pub enum SiteSourceKind {
    GitSource(GitSource),
}

pub struct GitSource {
    pub repository_url: String,
    pub id_ed25519: Ed25519,
}

impl sqlx::FromRow<'_, sqlx::postgres::PgRow> for SiteSource {
    fn from_row(row: &'_ sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        use sqlx::Row;

        let kind = if let Some(git_repository_url) = row.try_get("git_repository_url")? {
            let id_ed25519_public_key = row.try_get("git_id_ed25519_public_key")?;
            let id_ed25519_secret_key = row.try_get("git_id_ed25519_secret_key")?;
            let id_ed25519 = Ed25519::from_row(id_ed25519_public_key, id_ed25519_secret_key);

            let git_source = GitSource {
                repository_url: git_repository_url,
                id_ed25519,
            };

            Some(SiteSourceKind::GitSource(git_source))
        } else {
            None
        };

        Ok(SiteSource {
            name: row.try_get("name")?,
            created_time: row.try_get("created_time")?,
            hook_token: row.try_get("hook_token")?,
            kind,
        })
    }
}

impl Into<fairing_proto::sites::v1beta1::SiteSource> for SiteSource {
    fn into(self) -> fairing_proto::sites::v1beta1::SiteSource {
        let kind = self.kind.map(Into::into);

        fairing_proto::sites::v1beta1::SiteSource {
            name: self.name.name().into(),
            hook_url: self.hook_token,
            kind,
        }
    }
}

impl Into<fairing_proto::sites::v1beta1::site_source::Kind> for SiteSourceKind {
    fn into(self) -> fairing_proto::sites::v1beta1::site_source::Kind {
        use fairing_proto::sites::v1beta1::site_source::{GitSource, Kind};

        match self {
            SiteSourceKind::GitSource(git_source) => {
                let git_source = GitSource {
                    repository_url: git_source.repository_url,
                    id_ed25519_pub: git_source.id_ed25519.id_ed25519_pub(),
                };
                Kind::GitSource(git_source)
            }
        }
    }
}

pub struct Ed25519 {
    public_key: thrussh_keys::key::ed25519::PublicKey,
    secret_key: thrussh_keys::key::ed25519::SecretKey,
}

impl Ed25519 {
    pub fn from_row(public_key: Vec<u8>, secret_key: Vec<u8>) -> Ed25519 {
        use thrussh_keys::key::ed25519::{PublicKey, SecretKey};

        let public_key = {
            let mut key = PublicKey::new_zeroed();
            assert_eq!(key.key.len(), public_key.len());
            key.key.copy_from_slice(&public_key);
            key
        };

        let secret_key = {
            let mut key = SecretKey::new_zeroed();
            assert_eq!(key.key.len(), secret_key.len());
            key.key.copy_from_slice(&secret_key);
            key
        };

        Ed25519 {
            public_key,
            secret_key,
        }
    }

    pub fn public_key_to_slice(&self) -> &[u8] {
        &self.public_key.key
    }

    pub fn secret_key_to_slice(&self) -> &[u8] {
        &self.secret_key.key
    }

    pub fn generate() -> Ed25519 {
        let (public_key, secret_key) = thrussh_keys::key::ed25519::keypair();
        Ed25519 {
            public_key,
            secret_key,
        }
    }

    pub fn public_key(&self) -> thrussh_keys::key::PublicKey {
        use thrussh_keys::key::ed25519::PublicKey;
        thrussh_keys::key::PublicKey::Ed25519(PublicKey {
            key: self.public_key.key,
        })
    }

    pub fn id_ed25519_pub(&self) -> String {
        let mut writer = Vec::new();
        let public_key = self.public_key();
        thrussh_keys::write_public_key_base64(&mut writer, &public_key).unwrap();

        String::from_utf8(writer).unwrap()
    }
}

pub struct CreateSiteSource<'a> {
    pub resource_id: &'a str,
    pub parent: models::SiteName<'static>,
    pub kind: CreateSiteSourceKind<'a>,
}

pub enum CreateSiteSourceKind<'a> {
    GitSource { repository_url: &'a str },
}

impl<'a> CreateSiteSource<'a> {
    pub fn create(&self) -> Result<SiteSource> {
        use rand::distributions::Distribution;

        const HEX: &[u8; 16] = b"abcdef0123456789";

        let name = format!("{}/sources/{}", self.parent.name(), self.resource_id);
        let name = SiteSourceName::parse(name)?;

        let hook_token = rand::distributions::Uniform::new_inclusive(0, HEX.len() - 1)
            .sample_iter(&mut rand::thread_rng())
            .take(32)
            .map(|i| char::from(HEX[i]))
            .collect();

        let kind = self.kind.create()?;

        Ok(SiteSource {
            name,
            created_time: Utc::now(),
            hook_token,
            kind: Some(kind),
        })
    }
}

impl<'a> CreateSiteSourceKind<'a> {
    pub fn create(&self) -> Result<SiteSourceKind> {
        match self {
            &CreateSiteSourceKind::GitSource { repository_url } => {
                let git_source = GitSource {
                    repository_url: repository_url.into(),
                    id_ed25519: Ed25519::generate(),
                };

                Ok(SiteSourceKind::GitSource(git_source))
            }
        }
    }
}
