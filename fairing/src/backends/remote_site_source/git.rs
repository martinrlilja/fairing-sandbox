use anyhow::{anyhow, Context, Result};
use miniz_oxide::inflate::stream::InflateState;
use sha1::{Digest, Sha1};
use std::{borrow::Cow, path::Path, sync::Arc};
use tokio::{
    fs,
    io::AsyncWriteExt,
    task,
};

use fairing_core::models::{self, prelude::*};

const REVISION_LIMIT: usize = 4096;

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
    ) -> Result<()> {
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
                    tracing::info!("caps: {:?}", reader.capabilities);

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

        let mut reader = GitPackFileReader::open(".").await?;

        while let Some(Some(())) = client.read(&mut reader).await? {}

        reader.flush().await?;

        client.disconnect().await?;

        Ok(())
    }
}

#[derive(Copy, Clone, Debug)]
struct PackFileHeader {
    version: u32,
    objects: u32,
}

#[derive(Copy, Clone, Debug)]
enum ObjectType {
    Commit,
    Tree,
    Blob,
    Tag,
    RefDelta {
        parent: [u8; 20],
    },
}

#[derive(Copy, Clone, Debug)]
struct ObjectHeader {
    type_: ObjectType,
    length: u64,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct IndexKey {
    type_: IndexKeyType,
    sha1_hash: [u8; 20],
}

#[derive(serde::Serialize, serde::Deserialize)]
enum IndexKeyType {
    Commit,
    Tree,
    Blob,
    Tag,
    RefDelta,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct IndexValue {
    file_offset: u64,
    compressed_length: u64,
}

impl From<ObjectType> for IndexKeyType {
    fn from(type_: ObjectType) -> Self {
        match type_ {
            ObjectType::Commit => IndexKeyType::Commit,
            ObjectType::Tree => IndexKeyType::Tree,
            ObjectType::Blob => IndexKeyType::Blob,
            ObjectType::Tag => IndexKeyType::Tag,
            ObjectType::RefDelta { .. } => IndexKeyType::RefDelta,
        }
    }
}

struct GitPackFileReader {
    header: Option<PackFileHeader>,
    index: rocksdb::DB,
    pack: fs::File,
    decoder: ObjectDecoder,

    next_object_file_offset: u64,
    next_object_index: u32,
    current_object: Option<ObjectHeader>,
}

impl GitPackFileReader {
    async fn open(path: impl AsRef<Path>) -> Result<GitPackFileReader> {
        let path = path.as_ref().to_owned();
        let datbase_path = path.join("index");
        let pack_path = path.join("pack");

        let index = task::spawn_blocking(|| rocksdb::DB::open_default(datbase_path)).await??;

        let pack = fs::OpenOptions::new()
            .create(true)
            .write(true)
            .open(pack_path)
            .await?;

        Ok(GitPackFileReader {
            header: None,
            index,
            pack,
            decoder: ObjectDecoder::new(),
            next_object_file_offset: 0,
            next_object_index: 0,
            current_object: None,
        })
    }

    async fn flush(mut self) -> Result<()> {
        let index = self.index;
        task::spawn_blocking(move || index.flush()).await??;

        self.pack.flush().await?;

        Ok(())
    }
}

#[async_trait::async_trait]
impl SshReader for GitPackFileReader {
    type Output = Option<()>;

    async fn read<'a>(
        &mut self,
        _client: &mut SshClient,
        input: &'a [u8],
    ) -> nom::IResult<&'a [u8], Self::Output> {
        let (input, header) = if let Some(header) = self.header {
            (input, header)
        } else {
            let (input, _) = nom::bytes::streaming::tag(b"0008NAK\n")(input)?;
            let (input, _) = nom::bytes::streaming::tag(b"PACK")(input)?;
            let (input, version) = nom::number::streaming::be_u32(input)?;
            let (input, objects) = nom::number::streaming::be_u32(input)?;

            let header = PackFileHeader { version, objects };

            // TODO: check the version and number of objects.

            self.header = Some(header);
            (input, header)
        };

        if let Some(current_object) = self.current_object {
            let (rest, decoded_object) = self.decoder.write(input, current_object)?;

            let data_to_write = &input[..input.len() - rest.len()];

            tracing::trace!("writing {} bytes to pack", data_to_write.len());

            self.pack.write_all(data_to_write).await.unwrap();

            if let Some(decoded_object) = decoded_object {
                let key = bincode::serialize(&IndexKey {
                    type_: current_object.type_.into(),
                    sha1_hash: decoded_object.sha1_hash,
                }).unwrap();

                let value = bincode::serialize(&IndexValue {
                    file_offset: self.next_object_file_offset,
                    compressed_length: decoded_object.compressed_length,
                }).unwrap();

                tracing::trace!("writing object {:?} to index", decoded_object.sha1_hash);

                task::block_in_place(|| self.index.put(key, value)).unwrap();

                self.next_object_file_offset += decoded_object.compressed_length;
                self.next_object_index += 1;
                self.current_object = None;
            }

            if self.next_object_index == header.objects {
                // We have read all the objects we expected to read.
                tracing::trace!("read all {} objects", header.objects);
                Ok((rest, None))
            } else {
                Ok((rest, Some(())))
            }
        } else {
            let (input, (has_long_object_size, object_type, object_size_first)) =
                nom::bits::bits::<_, (bool, u8, u64), nom::error::Error<(&[u8], usize)>, _, _>(
                    nom::sequence::tuple((
                        nom::combinator::map(nom::bits::streaming::take(1_u8), |bit: u8| bit == 1),
                        nom::bits::streaming::take(3_u8),
                        nom::bits::streaming::take(4_u8),
                    )),
                )(input)?;

            let (input, object_size) =
                if has_long_object_size {
                    let (input, ((offset, object_size_first), object_size_last)) =
                        nom::bits::bits::<
                            _,
                            ((u64, u64), u64),
                            nom::error::Error<(&[u8], usize)>,
                            _,
                            _,
                        >(nom::sequence::tuple((
                            nom::multi::fold_many0(
                                nom::sequence::preceded(
                                    nom::bits::streaming::tag(1_u8, 1_u8),
                                    nom::bits::streaming::take(7_u8),
                                ),
                                || (4, object_size_first),
                                |(offset, size): (u64, u64), value: u64| {
                                    (offset + 7, (value << offset) | size)
                                },
                            ),
                            nom::sequence::preceded(
                                nom::bits::streaming::tag(0_u8, 1_u8),
                                nom::bits::streaming::take(7_u8),
                            ),
                        )))(input)?;

                    (input, (object_size_last << offset) | object_size_first)
                } else {
                    (input, object_size_first)
                };

            let (input, object_type) = match object_type {
                1 => (input, ObjectType::Commit),
                2 => (input, ObjectType::Tree),
                3 => (input, ObjectType::Blob),
                4 => (input, ObjectType::Tag),
                7 => {
                    let (input, parent) = nom::bytes::streaming::take(20usize)(input)?;
                    let parent = {
                        let mut output = [0u8; 20];
                        output.copy_from_slice(parent);
                        output
                    };

                    (input, ObjectType::RefDelta { parent })
                }
                _ => return Err(nom::Err::Failure(nom::error::Error::new(input, nom::error::ErrorKind::Verify))),
            };

            self.current_object = Some(ObjectHeader {
                type_: object_type,
                length: object_size,
            });

            tracing::trace!("current_object: {:?}", self.current_object);

            Ok((input, Some(())))
        }
    }
}

struct ObjectDecoder {
    sha1_hasher: Sha1,
    inflate_state: Box<InflateState>,
    buffer: Vec<u8>,
    bytes_read: u64,
}

struct ObjectDecoderResult {
    sha1_hash: [u8; 20],
    compressed_length: u64,
}

impl ObjectDecoder {
    fn new() -> ObjectDecoder {
        ObjectDecoder {
            sha1_hasher: Sha1::new(),
            inflate_state: InflateState::new_boxed(miniz_oxide::DataFormat::Zlib),
            buffer: vec![0u8; 8192],
            bytes_read: 0,
        }
    }

    fn write<'a>(&mut self, input: &'a [u8], object_header: ObjectHeader) -> nom::IResult<&'a [u8], Option<ObjectDecoderResult>> {
        use miniz_oxide::{inflate::stream::inflate, MZStatus, MZError, MZFlush, DataFormat};
        use nom::error::{Error, ErrorKind};

        let result = inflate(&mut self.inflate_state, input, &mut self.buffer, MZFlush::None);

        let rest = &input[result.bytes_consumed..];

        let output = &self.buffer[..result.bytes_written];

        if self.bytes_read == 0 {
            match object_header.type_ {
                ObjectType::Commit => self.sha1_hasher.update(b"commit"),
                ObjectType::Tree => self.sha1_hasher.update(b"tree"),
                ObjectType::Blob => self.sha1_hasher.update(b"blob"),
                ObjectType::Tag => self.sha1_hasher.update(b"tag"),
                ObjectType::RefDelta { .. } => self.sha1_hasher.update(b"ref-delta"),
            }

            let length_header = format!(" {}\0", object_header.length);
            self.sha1_hasher.update(length_header.as_bytes());
        }

        self.sha1_hasher.update(&output);
        self.bytes_read += result.bytes_consumed as u64;

        match result.status {
            Ok(MZStatus::Ok) => Ok((rest, None)),
            Ok(MZStatus::NeedDict) => {
                // TODO: verify that this is the right way to handle this status code.
                assert!(result.bytes_consumed == 0);
                Err(nom::Err::Incomplete(nom::Needed::Unknown))
            }
            Ok(MZStatus::StreamEnd) => {
                let sha1_hash = {
                    let mut sha1_hash = [0u8; 20];
                    let output = self.sha1_hasher.finalize_reset();
                    sha1_hash.copy_from_slice(&output);
                    sha1_hash
                };

                let object = ObjectDecoderResult {
                    sha1_hash,
                    compressed_length: self.bytes_read,
                };

                self.inflate_state.reset(DataFormat::Zlib);
                self.bytes_read = 0;

                Ok((rest, Some(object)))
            }
            Err(MZError::Buf) => {
                debug_assert!(result.bytes_consumed == 0);
                Err(nom::Err::Incomplete(nom::Needed::Unknown))
            }
            Err(err) => {
                tracing::debug!("inflate error: {:?}", err);
                Err(nom::Err::Failure(Error::new(input, ErrorKind::Verify)))
            }
        }
    }
}

enum GitPktLineOutput {
    RefPkt(models::CreateTreeRevision<'static>),
    Flush,
}

struct GitPktLineReader<'n> {
    site_source_name: &'n models::SiteSourceName<'n>,
    head_hash: Option<String>,
    capabilities: Option<String>,
}

impl<'n> GitPktLineReader<'n> {
    pub fn new(site_source_name: &'n models::SiteSourceName<'n>) -> GitPktLineReader<'n> {
        GitPktLineReader {
            site_source_name,
            head_hash: None,
            capabilities: None,
        }
    }
}

#[async_trait::async_trait]
impl<'n> SshReader for GitPktLineReader<'n> {
    type Output = Option<GitPktLineOutput>;

    async fn read<'a>(
        &mut self,
        _client: &mut SshClient,
        input: &'a [u8],
    ) -> nom::IResult<&'a [u8], Self::Output> {
        let (input, pkt_line) = ref_pkt_line(input)?;

        // Only the first pkt line is allowed to contain capabilities.
        if self.capabilities.is_none() {
            if let PktLine::Data(RefPkt { capabilities, .. }) = pkt_line {
                self.capabilities = Some(capabilities.to_owned());
            }
        }

        match pkt_line {
            PktLine::Data(RefPkt {
                hash,
                ref_name: "HEAD",
                ..
            }) => {
                self.head_hash = Some(hash.to_owned());
                Ok((input, None))
            }
            PktLine::Data(RefPkt { hash, ref_name, .. }) if ref_name.starts_with("refs/heads/") => {
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

                Ok((input, Some(GitPktLineOutput::RefPkt(revision))))
            }
            PktLine::Data(RefPkt { .. }) => {
                // Ignore anything that is not a branch.
                Ok((input, None))
            }
            PktLine::Flush => Ok((input, Some(GitPktLineOutput::Flush))),
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
    capabilities: &'a str,
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

    // Read hash
    let (pkt, hash) =
        nom::bytes::complete::take_while_m_n(40, 40, nom::character::is_hex_digit)(pkt)?;

    let hash = std::str::from_utf8(hash)
        .map_err(|_| nom::Err::Error(nom::error::Error::new(pkt, nom::error::ErrorKind::Char)))?;

    let (pkt, _) = nom::bytes::complete::tag(" ")(pkt)?;

    // Read ref_name
    let (pkt, ref_name) =
        nom::bytes::complete::take_while_m_n(1, 128, |c: u8| c != b'\0' && c != b'\n')(pkt)?;

    let ref_name = std::str::from_utf8(ref_name)
        .map_err(|_| nom::Err::Error(nom::error::Error::new(pkt, nom::error::ErrorKind::Char)))?;

    // Read capabilities
    let (pkt, capabilities) =
        nom::bytes::complete::take_while_m_n(0, 16_384, |c: u8| c != b'\n')(pkt)?;

    let capabilities = std::str::from_utf8(capabilities)
        .map_err(|_| nom::Err::Error(nom::error::Error::new(pkt, nom::error::ErrorKind::Char)))?;

    Ok((
        input,
        PktLine::Data(RefPkt {
            hash,
            ref_name,
            capabilities,
        }),
    ))
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
