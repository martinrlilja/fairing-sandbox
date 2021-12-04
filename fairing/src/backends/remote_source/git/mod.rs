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

pub struct GitRemoteSource;

impl GitRemoteSource {
    pub async fn list_tree_revisions(
        &self,
        source: &models::Source,
        git_source: &models::GitSource,
    ) -> Result<Vec<models::CreateBuild>> {
        let repository = git_source.repository_url.parts()?;
        let command = format!("git-upload-pack '{}'", repository.path);

        let config = SshClientConfig {
            addr: (repository.host, repository.port),
            user: &repository.user,
            command: &command,
            key_pair: git_source.id_ed25519.key_pair(),
        };

        let mut client = SshClient::connect(config).await?;
        let mut reader = GitPktLineReader::new(&source.name);

        let mut revisions = vec![];

        while let Some(output) = client.read(&mut reader).await? {
            if revisions.len() > REVISION_LIMIT {
                tracing::debug!("too many revisions ({})", source.name.name());
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
        source: &models::Source,
        git_source: &models::GitSource,
        build: &models::Build,
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
        let mut reader = GitPktLineReader::new(&source.name);

        let layer_set_name = build.name.parent();
        let mut found_hash: Option<String> = None;

        while let Some(output) = client.read(&mut reader).await? {
            match output {
                Some(GitPktLineOutput::RefPkt(tree_revision)) => {
                    if tree_revision.parent.resource() == layer_set_name.resource() {
                        found_hash = Some(build.source_reference.clone());
                    }
                }
                Some(GitPktLineOutput::Flush) => {
                    tracing::info!("caps: {:?}", reader.capabilities());

                    if let Some(found_hash) = found_hash {
                        // Request the commit we want if it was part of the ref pkts above,
                        // otherwise request the closest commit.
                        // TODO: look for allowReachableSHA1InWant or allowAnySHA1InWant in capabilities.
                        let line = if found_hash == build.source_reference {
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
            hex::decode_to_slice(&build.source_reference, &mut key)?;
            key
        };

        let source_directory = local_pack_file_reader::extract(commit_key, &work_directory).await?;

        Ok(source_directory)
    }
}
