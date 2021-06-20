use anyhow::{anyhow, Context, Result};
use std::{borrow::Cow, sync::Arc};

use fairing_core::models::{self, prelude::*};

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

        while let Some(read_revisions) = client.read(&mut reader).await? {
            revisions.extend_from_slice(&read_revisions);
            /*
            for revision in revisions {
                tracing::info!("pkt-line: {:?} {:?} {:?}", revision.parent.name(), revision.resource_id, revision.status);
            }
            */
        }

        client.disconnect().await?;

        Ok(revisions)
    }
}

struct GitPktLineReader<'n> {
    site_source_name: &'n models::SiteSourceName<'n>,
    head_hash: Option<String>,
}

impl<'n> GitPktLineReader<'n> {
    fn new(site_source_name: &'n models::SiteSourceName<'n>) -> GitPktLineReader<'n> {
        GitPktLineReader {
            site_source_name,
            head_hash: None,
        }
    }
}

#[async_trait::async_trait]
impl<'n> SshReader for GitPktLineReader<'n> {
    type Output = Vec<models::CreateTreeRevision<'static>>;

    async fn read<'a>(
        &mut self,
        client: &mut SshClient,
        input: &'a [u8],
    ) -> nom::IResult<&'a [u8], Self::Output> {
        let (input, pkt_line) = ref_pkt_line(input)?;

        match pkt_line {
            PktLine::Data(RefPkt {
                hash,
                ref_name: "HEAD",
            }) => {
                self.head_hash = Some(hash.to_owned());
                Ok((input, vec![]))
            }
            PktLine::Data(RefPkt { hash, ref_name }) if ref_name.starts_with("refs/heads/") => {
                let ref_name = ref_name.replace('/', ":");
                let hash = hash.to_owned();

                let status = match self.head_hash {
                    Some(ref head_hash) if head_hash == &hash => models::TreeRevisionStatus::Fetch,
                    _ => models::TreeRevisionStatus::Ignore,
                };

                let tree_name = format!("{}/trees/{}", self.site_source_name.name(), ref_name);
                let revision = models::CreateTreeRevision {
                    resource_id: Cow::Owned(hash),
                    parent: models::TreeName::parse(tree_name).unwrap(),
                    status,
                };

                Ok((input, vec![revision]))
            }
            PktLine::Data(RefPkt { .. }) => {
                // Ignore anything that is not a branch.
                Ok((input, vec![]))
            }
            PktLine::Flush => {
                client.data(&b"0000"[..]).await.unwrap();
                Ok((input, vec![]))
            }
        }
    }
}

/// Git pkt-line: https://git-scm.com/docs/protocol-common/en#_pkt_line_format
#[derive(Copy, Clone, Debug)]
enum PktLine<D> {
    Data(D),
    Flush,
}

#[derive(Copy, Clone, Debug)]
struct RefPkt<'a> {
    hash: &'a str,
    ref_name: &'a str,
}

fn data_pkt(input: &[u8]) -> nom::IResult<&[u8], &[u8]> {
    let (input, len) = nom::combinator::map_res(
        nom::bytes::streaming::take_while_m_n(4, 4, nom::character::is_hex_digit),
        |s| u16::from_str_radix(std::str::from_utf8(s).unwrap(), 16),
    )(input)?;

    if len <= 4 {
        // TODO: this should probably point to the start of the length.
        return Err(nom::Err::Error(nom::error::Error::new(
            input,
            nom::error::ErrorKind::Eof,
        )));
    }

    let (input, data) = nom::bytes::streaming::take(len - 4)(input)?;

    Ok((input, data))
}

fn flush_pkt<D>(input: &[u8]) -> nom::IResult<&[u8], PktLine<D>> {
    let (input, _) = nom::bytes::streaming::tag(b"0000")(input)?;
    Ok((input, PktLine::Flush))
}

fn ref_pkt_line(input: &[u8]) -> nom::IResult<&[u8], PktLine<RefPkt>> {
    nom::branch::alt((flush_pkt, ref_pkt))(input)
}

fn ref_pkt(input: &[u8]) -> nom::IResult<&[u8], PktLine<RefPkt>> {
    let (input, pkt) = data_pkt(input)?;

    let (pkt, hash) =
        nom::bytes::complete::take_while_m_n(40, 40, nom::character::is_hex_digit)(pkt)?;

    let hash = std::str::from_utf8(hash)
        .map_err(|_| nom::Err::Error(nom::error::Error::new(pkt, nom::error::ErrorKind::Char)))?;

    let (pkt, _) = nom::bytes::complete::tag(" ")(pkt)?;

    let (pkt, ref_name) =
        nom::bytes::complete::take_while_m_n(1, 128, |c: u8| c != b'\0' && c != b'\n')(pkt)?;

    let ref_name = std::str::from_utf8(ref_name)
        .map_err(|_| nom::Err::Error(nom::error::Error::new(pkt, nom::error::ErrorKind::Char)))?;

    // TODO: parse the flags.

    Ok((input, PktLine::Data(RefPkt { hash, ref_name })))
}

#[async_trait::async_trait]
trait SshReader {
    type Output;

    async fn read<'a>(
        &mut self,
        client: &mut SshClient,
        input: &'a [u8],
    ) -> nom::IResult<&'a [u8], Self::Output>;
}

struct SshClientConfig<'a, Addr: tokio::net::ToSocketAddrs> {
    addr: Addr,
    user: &'a str,
    command: &'a str,
    key_pair: thrussh_keys::key::KeyPair,
}

struct SshClient {
    session: thrussh::client::Handle<Client>,
    channel: thrussh::client::Channel,
    buffer: Option<thrussh::CryptoVec>,
}

impl SshClient {
    pub async fn connect<Addr: tokio::net::ToSocketAddrs>(
        config: SshClientConfig<'_, Addr>,
    ) -> Result<SshClient> {
        let ssh_config = thrussh::client::Config::default();
        let ssh_config = Arc::new(ssh_config);

        let key_pair = Arc::new(config.key_pair);

        let tcp_stream = tokio::net::TcpStream::connect(config.addr)
            .await
            .context("connecting to the repository")?;

        let mut session = thrussh::client::connect_stream(ssh_config, tcp_stream, Client)
            .await
            .context("connecting to the repository")?;

        let success = session
            .authenticate_publickey(config.user, key_pair)
            .await
            .context("authenticating to the repository")?;

        if !success {
            return Err(anyhow!("could not authenticate"));
        }

        let mut channel = session
            .channel_open_session()
            .await
            .context("opening channel session")?;

        channel
            .exec(true, config.command)
            .await
            .context("executing command")?;

        Ok(SshClient {
            session,
            channel,
            buffer: None,
        })
    }

    pub async fn disconnect(mut self) -> Result<()> {
        self.session
            .disconnect(thrussh::Disconnect::ByApplication, "", "en")
            .await
            .context("disconnecting")?;

        self.session.await.context("waiting for session")?;

        Ok(())
    }

    pub async fn read<R: SshReader>(&mut self, reader: &mut R) -> Result<Option<R::Output>> {
        while let Some(message) = self.channel.wait().await {
            use thrussh::ChannelMsg;

            match message {
                ChannelMsg::Data { data } => {
                    if let Some(mut buffer) = self.buffer.take() {
                        buffer.extend(&data);
                        let result = reader.read(self, &buffer).await;
                        match result {
                            Ok((input, result)) => {
                                if !input.is_empty() {
                                    self.buffer = Some(thrussh::CryptoVec::from_slice(&input));
                                }

                                return Ok(Some(result));
                            }
                            Err(nom::Err::Incomplete(_)) => self.buffer = Some(buffer),
                            Err(nom::Err::Error(err)) => return Err(anyhow!("{:?}", err)),
                            Err(nom::Err::Failure(err)) => return Err(anyhow!("{:?}", err)),
                        }
                    } else {
                        let result = reader.read(self, &data).await;
                        match result {
                            Ok((input, result)) => {
                                if !input.is_empty() {
                                    self.buffer = Some(thrussh::CryptoVec::from_slice(&input));
                                }

                                return Ok(Some(result));
                            }
                            Err(nom::Err::Incomplete(_)) => self.buffer = Some(data),
                            Err(nom::Err::Error(err)) => return Err(anyhow!("{:?}", err)),
                            Err(nom::Err::Failure(err)) => return Err(anyhow!("{:?}", err)),
                        }
                    }
                }
                ChannelMsg::ExitStatus { exit_status: 0 } => return Ok(None),
                ChannelMsg::ExitStatus { exit_status } => {
                    return Err(anyhow!("exit status: {}", exit_status))
                }
                _ => (),
            }
        }

        Ok(None)
    }

    pub async fn data(&mut self, data: &[u8]) -> Result<()> {
        self.channel.data(data).await.context("sending data")?;
        Ok(())
    }
}

struct Client;

impl thrussh::client::Handler for Client {
    type Error = anyhow::Error;
    type FutureUnit =
        futures::future::Ready<Result<(Self, thrussh::client::Session), anyhow::Error>>;
    type FutureBool = futures::future::Ready<Result<(Self, bool), anyhow::Error>>;

    fn finished_bool(self, b: bool) -> Self::FutureBool {
        futures::future::ready(Ok((self, b)))
    }

    fn finished(self, session: thrussh::client::Session) -> Self::FutureUnit {
        futures::future::ready(Ok((self, session)))
    }

    fn check_server_key(
        self,
        server_public_key: &thrussh_keys::key::PublicKey,
    ) -> Self::FutureBool {
        tracing::info!("check_server_key: {:?}", server_public_key);
        self.finished_bool(true)
    }
}
