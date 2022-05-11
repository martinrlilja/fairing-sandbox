use anyhow::{ensure, Result};
use miniz_oxide::inflate::stream::InflateState;
use sha1::{Digest, Sha1};
use std::{path::Path, sync::Arc};
use tokio::{
    fs,
    io::{AsyncWrite, AsyncWriteExt},
    task,
};

use super::{
    parsers::{
        pack_file_header, pack_file_object_header, PackFileHeader, PackFileObjectHeader,
        PackFileObjectType,
    },
    IndexObject, SshClient, SshReader,
};

pub struct GitPackFileReader {
    header: Option<PackFileHeader>,
    index: Arc<rocksdb::DB>,
    pack: fs::File,
    decoder: ObjectDecoder,

    next_object_file_offset: u64,
    next_object_index: u32,
    current_object: Option<PackFileObjectHeader>,
}

impl GitPackFileReader {
    pub async fn open(path: impl AsRef<Path>) -> Result<GitPackFileReader> {
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
            index: Arc::new(index),
            pack,
            decoder: ObjectDecoder::new(),
            next_object_file_offset: 0,
            next_object_index: 0,
            current_object: None,
        })
    }

    pub async fn flush(mut self) -> Result<Arc<rocksdb::DB>> {
        tracing::debug!("wrote {} bytes", self.next_object_file_offset);

        if let Some(header) = self.header {
            ensure!(
                self.next_object_index == header.objects,
                "expected {} objects got {}",
                header.objects,
                self.next_object_index
            );
        }

        self.pack.flush().await?;
        Ok(self.index)
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
            let (input, header) = pack_file_header(input)?;

            // TODO: check the version and number of objects.

            self.header = Some(header);

            (input, header)
        };

        if let Some(current_object) = self.current_object {
            let (rest, decoded_object) = self
                .decoder
                .write(input, current_object, &mut self.pack)
                .await?;

            if let Some(decoded_object) = decoded_object {
                let key = decoded_object.sha1_hash;

                let value = bincode::serialize(&IndexObject {
                    type_: current_object.type_,
                    offset: self.next_object_file_offset,
                    length: decoded_object.decompressed_length,
                })
                .unwrap();

                let index = self.index.clone();
                task::spawn_blocking(move || index.put(&key, value))
                    .await
                    .unwrap()
                    .unwrap();

                self.next_object_file_offset += decoded_object.decompressed_length;
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
            let (input, object_header) = pack_file_object_header(input)?;

            self.current_object = Some(object_header);

            Ok((input, Some(())))
        }
    }
}

struct ObjectDecoder {
    sha1_hasher: Sha1,
    inflate_state: Box<InflateState>,
    buffer: Vec<u8>,
    bytes_read: u64,
    bytes_written: u64,
}

struct ObjectDecoderResult {
    sha1_hash: [u8; 20],
    compressed_length: u64,
    decompressed_length: u64,
}

impl ObjectDecoder {
    fn new() -> ObjectDecoder {
        ObjectDecoder {
            sha1_hasher: Sha1::new(),
            inflate_state: InflateState::new_boxed(miniz_oxide::DataFormat::Zlib),
            buffer: vec![0u8; 8192],
            bytes_read: 0,
            bytes_written: 0,
        }
    }

    async fn write<'a>(
        &mut self,
        input: &'a [u8],
        object_header: PackFileObjectHeader,
        mut writer: impl AsyncWrite + Unpin,
    ) -> nom::IResult<&'a [u8], Option<ObjectDecoderResult>> {
        use miniz_oxide::{inflate::stream::inflate, DataFormat, MZError, MZFlush, MZStatus};
        use nom::error::{Error, ErrorKind};

        let result = inflate(
            &mut self.inflate_state,
            input,
            &mut self.buffer,
            MZFlush::None,
        );

        let rest = &input[result.bytes_consumed..];

        let output = &self.buffer[..result.bytes_written];

        writer.write_all(output).await.unwrap();

        if self.bytes_read == 0 {
            match object_header.type_ {
                PackFileObjectType::Commit => self.sha1_hasher.update(b"commit"),
                PackFileObjectType::Tree => self.sha1_hasher.update(b"tree"),
                PackFileObjectType::Blob => self.sha1_hasher.update(b"blob"),
                PackFileObjectType::Tag => self.sha1_hasher.update(b"tag"),
                PackFileObjectType::RefDelta { .. } => self.sha1_hasher.update(b"ref-delta"),
            }

            let length_header = format!(" {}\0", object_header.length);
            self.sha1_hasher.update(length_header.as_bytes());
        }

        self.sha1_hasher.update(&output);
        self.bytes_read += result.bytes_consumed as u64;
        self.bytes_written += result.bytes_written as u64;

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

                debug_assert!(self.bytes_written == object_header.length);

                let object = ObjectDecoderResult {
                    sha1_hash,
                    compressed_length: self.bytes_read,
                    decompressed_length: self.bytes_written,
                };

                self.inflate_state.reset(DataFormat::Zlib);
                self.bytes_read = 0;
                self.bytes_written = 0;

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
