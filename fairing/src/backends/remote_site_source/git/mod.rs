use anyhow::{anyhow, Result};
use std::path::PathBuf;

use fairing_core::models::{self, prelude::*};

use git_pack_file_reader::GitPackFileReader;
use parsers::PackFileObjectType;
use pkt_line_reader::{GitPktLineOutput, GitPktLineReader};
use ssh::{SshClient, SshClientConfig, SshReader};

mod git_pack_file_reader;
mod local_pack_file_reader;
mod parsers;
mod pkt_line_reader;
mod ssh;

const REVISION_LIMIT: usize = 4096;

#[derive(Copy, Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct IndexObject {
    type_: PackFileObjectType,
    offset: u64,
    length: u64,
}

pub struct GitRemoteSiteSource;

impl GitRemoteSiteSource {
    pub async fn list_tree_revisions(
        &self,
        site_source: &models::SiteSource,
        git_source: &models::GitSource,
    ) -> Result<Vec<models::CreateTreeRevision<'static>>> {
        let repository = git_source.repository_url.parts()?;
        let command = format!("git-upload-pack '{}'", repository.path);

        let config = SshClientConfig {
            addr: (repository.host, repository.port),
            user: &repository.user,
            command: &command,
            key_pair: git_source.id_ed25519.key_pair(),
        };

        let mut client = SshClient::connect(config).await?;
        let mut reader = GitPktLineReader::new(&site_source.name);

        let mut revisions = vec![];

        while let Some(output) = client.read(&mut reader).await? {
            if revisions.len() > REVISION_LIMIT {
                tracing::debug!("too many revisions ({})", site_source.name.name());
                break;
            }

            match output {
                Some(GitPktLineOutput::RefPkt(tree_revision)) => revisions.push(tree_revision),
                Some(GitPktLineOutput::Flush) => {
                    client.data(&b"0000"[..]).await?;
                    break;
                }
                None => (),
            }
        }

        client.disconnect().await?;

        Ok(revisions)
    }

    pub async fn fetch<'n>(
        &self,
        site_source: &models::SiteSource,
        git_source: &models::GitSource,
        fetch_tree_revision: &models::TreeRevisionName<'n>,
        work_directory: PathBuf,
    ) -> Result<PathBuf> {
        let repository = git_source.repository_url.parts()?;
        let command = format!("git-upload-pack '{}'", repository.path);

        let config = SshClientConfig {
            addr: (repository.host, repository.port),
            user: &repository.user,
            command: &command,
            key_pair: git_source.id_ed25519.key_pair(),
        };

        let mut client = SshClient::connect(config).await?;
        let mut reader = GitPktLineReader::new(&site_source.name);

        let tree_name = fetch_tree_revision.parent();
        let mut found_hash: Option<String> = None;

        while let Some(output) = client.read(&mut reader).await? {
            match output {
                Some(GitPktLineOutput::RefPkt(tree_revision)) => {
                    if tree_revision.parent.resource() == tree_name.resource() {
                        found_hash = Some(tree_revision.resource_id.into());
                    }
                }
                Some(GitPktLineOutput::Flush) => {
                    tracing::info!("caps: {:?}", reader.capabilities());

                    if let Some(found_hash) = found_hash {
                        // Request the commit we want if it was part of the ref pkts above,
                        // otherwise request the closest commit.
                        // TODO: look for allowReachableSHA1InWant or allowAnySHA1InWant in capabilities.
                        let line = if found_hash == fetch_tree_revision.resource() {
                            format!("want {} deepen 1\n", found_hash)
                        } else {
                            format!("want {}\n", found_hash)
                        };

                        let line = format!("{:04x}{}", line.len() + 4, line);
                        client.data(line.as_bytes()).await?;
                        client.data(&b"0000"[..]).await?;
                        client.data(&b"0009done\n"[..]).await?;
                        break;
                    } else {
                        client.data(&b"0000"[..]).await?;
                        client.disconnect().await?;
                        return Err(anyhow!("remote ref no longer available"));
                    }
                }
                None => (),
            }
        }

        let mut reader = GitPackFileReader::open(&work_directory).await?;

        while let Some(Some(())) = client.read(&mut reader).await? {}

        reader.flush().await?;

        client.disconnect().await?;

        let commit_key = {
            let mut key = [0u8; 20];
            hex::decode_to_slice(fetch_tree_revision.resource(), &mut key)?;
            key
        };

        let source_directory = local_pack_file_reader::extract(commit_key, &work_directory).await?;

        Ok(source_directory)
    }
}
