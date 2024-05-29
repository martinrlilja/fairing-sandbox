use anyhow::{anyhow, Result};
use std::{fmt, str::FromStr};

use super::ProjectId;

#[derive(Clone, Debug)]
pub struct SourceName(String);

impl SourceName {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for SourceName {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<SourceName> {
        Ok(SourceName(s.into()))
    }
}

#[derive(Clone, Debug)]
pub struct Source {
    pub project_id: ProjectId,
    pub name: SourceName,
    pub kind: SourceKind,
}

impl Source {
    pub fn try_with_kind<Kind: SourceKindData>(self) -> Result<SourceWithKind<Kind>> {
        Ok(SourceWithKind {
            project_id: self.project_id,
            name: self.name,
            with: Kind::try_from(self.kind)?,
        })
    }
}

#[derive(Clone, Debug)]
pub enum SourceKind {
    Git {
        repository_url: GitRepository,
        id_ed25519: Ed25519,
    },
}

pub struct CreateSource {
    pub name: SourceName,
    pub kind: CreateSourceKind,
}

pub enum CreateSourceKind {
    Git { repository_url: GitRepository },
}

#[derive(Clone, Debug)]
pub struct SourceWithKind<Kind> {
    pub project_id: ProjectId,
    pub name: SourceName,
    pub with: Kind,
}

#[derive(Clone, Debug)]
pub struct GitSource {
    pub repository_url: GitRepository,
    pub id_ed25519: Ed25519,
}

impl SourceKindData for GitSource {}

impl TryFrom<SourceKind> for GitSource {
    type Error = anyhow::Error;

    fn try_from(source_kind: SourceKind) -> Result<Self> {
        match source_kind {
            SourceKind::Git {
                repository_url,
                id_ed25519,
            } => Ok(GitSource {
                repository_url,
                id_ed25519,
            }),
        }
    }
}

pub trait SourceKindData: TryFrom<SourceKind, Error = anyhow::Error> {}

#[derive(Clone, Debug)]
pub struct GitSourceRefAndCommit {
    pub ref_: String,
    pub commit: String,
}

#[derive(Clone, Debug)]
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

#[derive(Clone)]
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

impl fmt::Debug for Ed25519 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Ed25519")
            .field("id_ed25519_pub", &self.id_ed25519_pub())
            .finish_non_exhaustive()
    }
}
