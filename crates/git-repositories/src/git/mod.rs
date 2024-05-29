use anyhow::{anyhow, Result};
use std::path::PathBuf;

use fairing_core2::{models, repositories::GitSourceRepository};

use git_pack_file_reader::GitPackFileReader;
use parsers::PackFileObjectType;
use pkt_line_reader::{GitPktLineOutput, GitPktLineReader};
use ssh::{SshClient, SshClientConfig, SshReader};

mod git_pack_file_reader;
mod lfs;
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

pub struct ThrusshGitSource;

#[async_trait::async_trait]
impl GitSourceRepository for ThrusshGitSource {
    async fn git_list_latest(
        &self,
        source: &models::SourceWithKind<models::GitSource>,
    ) -> Result<Vec<models::GitSourceRefAndCommit>> {
        let repository = source.with.repository_url.parts()?;

        let config = SshClientConfig {
            addr: (repository.host.as_str(), repository.port),
            user: &repository.user,
            key_pair: source.with.id_ed25519.key_pair(),
        };

        let mut client = SshClient::connect(config).await?;
        let mut reader = GitPktLineReader::new();

        let mut revisions = vec![];

        let command = format!("git-upload-pack '{}'", repository.path);
        client.exec(&command).await?;

        while let Some(output) = client.read(&mut reader).await? {
            if revisions.len() > REVISION_LIMIT {
                tracing::debug!("too many revisions");
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

    async fn git_clone(
        &self,
        source: &models::SourceWithKind<models::GitSource>,
        ref_and_commit: &models::GitSourceRefAndCommit,
        work_directory: PathBuf,
    ) -> Result<PathBuf> {
        let repository = source.with.repository_url.parts()?;

        let config = SshClientConfig {
            addr: (repository.host.as_str(), repository.port),
            user: &repository.user,
            key_pair: source.with.id_ed25519.key_pair(),
        };

        let mut client = SshClient::connect(config).await?;
        let mut reader = GitPktLineReader::new();

        let mut found_hash: Option<String> = None;

        let command = format!("git-upload-pack '{}'", repository.path);
        client.exec(&command).await?;

        while let Some(output) = client.read(&mut reader).await? {
            match output {
                Some(GitPktLineOutput::RefPkt(pkt)) => {
                    if pkt.ref_ == ref_and_commit.ref_ {
                        found_hash = Some(pkt.commit);
                    }
                }
                Some(GitPktLineOutput::Flush) => {
                    tracing::info!("caps: {:?}", reader.capabilities());

                    if let Some(found_hash) = found_hash {
                        // Request the commit we want if it was part of the ref pkts above,
                        // otherwise request the closest commit.
                        // TODO: look for allowReachableSHA1InWant or allowAnySHA1InWant in capabilities.
                        let line = if found_hash == ref_and_commit.commit {
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

        let index = reader.flush().await?;

        //client.disconnect().await?;

        let commit_key = {
            let mut key = [0u8; 20];
            hex::decode_to_slice(&ref_and_commit.commit, &mut key)?;
            key
        };

        let source_directory =
            local_pack_file_reader::extract(commit_key, &work_directory, index).await?;

        if lfs::detect(&source_directory).await? {
            let config = SshClientConfig {
                addr: (repository.host.as_str(), repository.port),
                user: &repository.user,
                key_pair: source.with.id_ed25519.key_pair(),
            };

            //let mut client = SshClient::connect(config).await?;

            lfs::download(&mut client, &repository, &source_directory).await?;
        }

        client.disconnect().await?;

        Ok(source_directory)
    }
}
