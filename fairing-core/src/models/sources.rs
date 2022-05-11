use anyhow::{anyhow, Result};
use chrono::{DateTime, Utc};
use std::str::FromStr;

use crate::models::{
    self,
    prelude::*,
    resource_name::{validators, ParentedResourceName, ResourceNameInner},
};

crate::impl_resource_name! {
    pub struct SourceName<'n>;
}

impl<'n> ParentedResourceName<'n> for SourceName<'n> {
    const COLLECTION: &'static str = "sources";

    type Validator = validators::UnicodeIdentifierValidator;

    type Parent = crate::models::TeamName<'static>;
}

pub struct Source {
    pub name: SourceName<'static>,
    pub created_time: DateTime<Utc>,
    pub hook_token: String,
    pub kind: Option<SourceKind>,
}

pub enum SourceKind {
    GitSource(GitSource),
}

pub struct GitSource {
    pub repository_url: GitRepository,
    pub id_ed25519: Ed25519,
}

impl sqlx::FromRow<'_, sqlx::postgres::PgRow> for Source {
    fn from_row(row: &'_ sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        use sqlx::Row;

        let kind = if let Some(git_repository_url) = row.try_get("git_repository_url")? {
            let id_ed25519_secret_key = row.try_get("git_id_ed25519_secret_key")?;
            let id_ed25519 = Ed25519::from_row(id_ed25519_secret_key);

            let git_source = GitSource {
                repository_url: GitRepository(git_repository_url),
                id_ed25519,
            };

            Some(SourceKind::GitSource(git_source))
        } else {
            None
        };

        Ok(Source {
            name: row.try_get("name")?,
            created_time: row.try_get("created_time")?,
            hook_token: row.try_get("hook_token")?,
            kind,
        })
    }
}

impl Into<fairing_proto::sources::v1beta1::Source> for Source {
    fn into(self) -> fairing_proto::sources::v1beta1::Source {
        let kind = self.kind.map(Into::into);

        fairing_proto::sources::v1beta1::Source {
            name: self.name.name().into(),
            hook_url: self.hook_token,
            kind,
        }
    }
}

impl Into<fairing_proto::sources::v1beta1::source::Kind> for SourceKind {
    fn into(self) -> fairing_proto::sources::v1beta1::source::Kind {
        use fairing_proto::sources::v1beta1::source::Kind;

        match self {
            SourceKind::GitSource(git_source) => {
                let git_source = fairing_proto::sources::v1beta1::source::GitSource {
                    repository_url: git_source.repository_url.0,
                    id_ed25519_pub: git_source.id_ed25519.id_ed25519_pub(),
                };
                Kind::GitSource(git_source)
            }
        }
    }
}

pub struct GitRepository(String);

impl GitRepository {
    pub fn as_str(&'_ self) -> &'_ str {
        &self.0
    }

    pub fn parts(&self) -> Result<GitRepositoryParts> {
        self.0.parse()
    }
}

impl FromStr for GitRepository {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<GitRepository> {
        GitRepositoryParts::from_str(s).map(|_| GitRepository(s.to_owned()))
    }
}

#[derive(Clone, Debug)]
pub struct GitRepositoryParts {
    pub user: String,
    pub host: String,
    pub port: u16,
    pub path: String,
}

impl FromStr for GitRepositoryParts {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<GitRepositoryParts> {
        use regex::Regex;

        lazy_static::lazy_static! {
            static ref RE: Regex = Regex::new(r"^(?P<user>[^@]+)@(?P<host>[^:]+):(?P<path>[^'\\]+)$").unwrap();
        };

        if s.starts_with("ssh://") {
            let url = url::Url::parse(s)?;

            Ok(GitRepositoryParts {
                user: url.username().to_owned(),
                host: url.host_str().unwrap_or("").to_owned(),
                port: url.port().unwrap_or(22),
                // TODO: there is some special handling that must be done regarding ~
                // see more: https://git-scm.com/docs/pack-protocol/#_ssh_transport
                path: url.path().to_owned(),
            })
        } else {
            let captures = RE.captures(s);
            let captures = if let Some(captures) = captures {
                captures
            } else {
                return Err(anyhow!("invalid repository url"));
            };

            let user = captures.name("user").unwrap();
            let host = captures.name("host").unwrap();
            let path = captures.name("path").unwrap();

            Ok(GitRepositoryParts {
                user: user.as_str().to_owned(),
                host: host.as_str().to_owned(),
                port: 22,
                path: path.as_str().to_owned(),
            })
        }
    }
}

pub struct Ed25519 {
    secret_key: thrussh_keys::key::ed25519::SecretKey,
}

impl Ed25519 {
    pub fn from_row(secret_key: Vec<u8>) -> Ed25519 {
        use thrussh_keys::key::ed25519::SecretKey;

        let secret_key = {
            let mut key = SecretKey::new_zeroed();
            assert_eq!(key.key.len(), secret_key.len());
            key.key.copy_from_slice(&secret_key);
            key
        };

        Ed25519 { secret_key }
    }

    pub fn secret_key_to_slice(&self) -> &[u8] {
        &self.secret_key.key
    }

    pub fn generate() -> Ed25519 {
        let (_public_key, secret_key) = thrussh_keys::key::ed25519::keypair();
        Ed25519 { secret_key }
    }

    pub fn public_key(&self) -> thrussh_keys::key::PublicKey {
        let key_pair = self.key_pair();
        key_pair.clone_public_key()
    }

    pub fn key_pair(&self) -> thrussh_keys::key::KeyPair {
        let secret_key = thrussh_keys::key::ed25519::SecretKey {
            key: self.secret_key.key,
        };
        thrussh_keys::key::KeyPair::Ed25519(secret_key)
    }

    pub fn id_ed25519_pub(&self) -> String {
        let mut writer = Vec::new();
        let public_key = self.public_key();
        thrussh_keys::write_public_key_base64(&mut writer, &public_key).unwrap();

        String::from_utf8(writer).unwrap()
    }
}

pub struct CreateSource<'a> {
    pub resource_id: &'a str,
    pub parent: models::TeamName<'static>,
    pub kind: CreateSourceKind<'a>,
}

pub enum CreateSourceKind<'a> {
    GitSource { repository_url: &'a str },
}

impl<'a> CreateSource<'a> {
    pub fn create(&self) -> Result<Source> {
        use rand::distributions::Distribution;

        const HEX: &[u8; 16] = b"0123456789abcdef";

        let name = format!("{}/sources/{}", self.parent.name(), self.resource_id);
        let name = SourceName::parse(name)?;

        let hook_token = rand::distributions::Uniform::new_inclusive(0, HEX.len() - 1)
            .sample_iter(&mut rand::thread_rng())
            .take(32)
            .map(|i| char::from(HEX[i]))
            .collect();

        let kind = self.kind.create()?;

        Ok(Source {
            name,
            created_time: Utc::now(),
            hook_token,
            kind: Some(kind),
        })
    }
}

impl<'a> CreateSourceKind<'a> {
    pub fn create(&self) -> Result<SourceKind> {
        match self {
            &CreateSourceKind::GitSource { repository_url } => {
                let git_source = GitSource {
                    repository_url: repository_url.parse()?,
                    id_ed25519: Ed25519::generate(),
                };

                Ok(SourceKind::GitSource(git_source))
            }
        }
    }
}
