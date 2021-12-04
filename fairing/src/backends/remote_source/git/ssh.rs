use anyhow::{anyhow, Context, Result};
use std::sync::Arc;

#[async_trait::async_trait]
pub trait SshReader {
    type Output;

    async fn read<'a>(
        &mut self,
        client: &mut SshClient,
        input: &'a [u8],
    ) -> nom::IResult<&'a [u8], Self::Output>;
}

pub struct SshClientConfig<'a, Addr: tokio::net::ToSocketAddrs> {
    pub addr: Addr,
    pub user: &'a str,
    pub command: &'a str,
    pub key_pair: thrussh_keys::key::KeyPair,
}

pub struct SshClient {
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
        // Parse any buffered data first. Otherwise the buffer might get very large, or we will
        // lose data if the server disconnects.
        if let Some(buffer) = self.buffer.take() {
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
        }

        while let Some(message) = self.channel.wait().await {
            use thrussh::ChannelMsg;

            match message {
                ChannelMsg::Data { data } => {
                    let buffer = if let Some(mut buffer) = self.buffer.take() {
                        buffer.extend(&data);
                        buffer
                    } else {
                        data
                    };

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
