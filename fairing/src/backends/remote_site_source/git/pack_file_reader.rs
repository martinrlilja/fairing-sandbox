use anyhow::{anyhow, ensure, Context, Result};
use async_compression::tokio::{bufread::ZlibDecoder, write::ZlibEncoder};
use miniz_oxide::inflate::stream::InflateState;
use sha1::{Digest, Sha1};
use std::{
    convert::TryInto,
    io::SeekFrom,
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::{
    fs,
    io::{AsyncRead, AsyncReadExt, AsyncSeek, AsyncSeekExt, AsyncWriteExt, BufReader},
    task,
};

use super::{
    parsers::{
        delta_instruction, pack_file_header, pack_file_object_header, pack_file_variable_length,
        DeltaInstruction, PackFileHeader, PackFileObjectHeader, PackFileObjectType,
    },
    SshClient, SshReader,
};

#[derive(Copy, Clone, Debug, serde::Serialize, serde::Deserialize)]
struct IndexObject {
    type_: PackFileObjectType,
    file_offset: u64,
    compressed_length: u64,
    decompressed_length: u64,
}

pub struct GitPackFileReader {
    path: PathBuf,
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
            path,
            header: None,
            index: Arc::new(index),
            pack,
            decoder: ObjectDecoder::new(),
            next_object_file_offset: 0,
            next_object_index: 0,
            current_object: None,
        })
    }

    pub async fn flush(mut self) -> Result<()> {
        let mut pack_delta_file = fs::OpenOptions::new()
            .read(true)
            .open(self.path.join("pack"))
            .await
            .context("opening pack file for delta")?;

        let mut pack_parent_file = fs::OpenOptions::new()
            .read(true)
            .open(self.path.join("pack"))
            .await
            .context("opening pack file for parent")?;

        let mut pack_delta_buffer = Vec::with_capacity(8192);
        let mut pack_parent_buffer = Vec::with_capacity(8192);

        let mut sha1_hasher = Sha1::new();

        let ref_deltas = self
            .list_ref_deltas(None)
            .await
            .context("listing ref deltas")?;
        for (_key, ref_delta, parent) in ref_deltas {
            pack_delta_file
                .seek(SeekFrom::Start(ref_delta.file_offset))
                .await
                .context("seek in pack file for delta")?;
            let mut pack_delta = ZlibDecoder::new(BufReader::new(pack_delta_file));
            pack_delta.multiple_members(true);

            let pack_original_length = self.pack.metadata().await?.len();

            let mut pack = ZlibEncoder::new(self.pack);
            let mut reconstructed_length: Option<u64> = None;
            let mut read_bytes = 0u64;
            let mut written_bytes = 0u64;

            loop {
                pack_delta
                    .read_buf(&mut pack_delta_buffer)
                    .await
                    .context("reading delta")?;

                let reconstructed_length = if let Some(reconstructed_length) = reconstructed_length
                {
                    reconstructed_length
                } else {
                    let chunk_len: u64 = pack_delta_buffer.len().try_into().unwrap();
                    let chunk_limit = chunk_len.min(ref_delta.decompressed_length - read_bytes);
                    let delta_chunk = &pack_delta_buffer[..chunk_limit.try_into().unwrap()];
                    let header = pack_file_variable_length(delta_chunk)
                        .and_then(|(input, _)| pack_file_variable_length(input));

                    match header {
                        Ok((input, reconstructed_length_)) => {
                            match parent.type_ {
                                PackFileObjectType::Commit => sha1_hasher.update(b"commit"),
                                PackFileObjectType::Tree => sha1_hasher.update(b"tree"),
                                PackFileObjectType::Blob => sha1_hasher.update(b"blob"),
                                PackFileObjectType::Tag => sha1_hasher.update(b"tag"),
                                PackFileObjectType::RefDelta { .. } => Err(anyhow!(
                                    "ref delta can not have another ref delta as a parent"
                                ))?,
                            }

                            let length_header = format!(" {}\0", reconstructed_length_);
                            sha1_hasher.update(length_header.as_bytes());

                            let read_length = delta_chunk.len() - input.len();
                            let unread_length = input.len();
                            pack_delta_buffer.copy_within(read_length.., 0);
                            pack_delta_buffer.truncate(unread_length);

                            read_bytes += read_length as u64;

                            reconstructed_length = Some(reconstructed_length_);
                            reconstructed_length_
                        }
                        Err(nom::Err::Incomplete(_)) => continue,
                        Err(err) => Err(anyhow!("error parsing delta header: {:?}", err))?,
                    }
                };

                loop {
                    let delta_chunk_len: u64 = pack_delta_buffer.len().try_into().unwrap();

                    tracing::trace!("{}/{}", read_bytes, ref_delta.decompressed_length);

                    if ref_delta.decompressed_length <= read_bytes {
                        break;
                    }

                    let delta_chunk_limit =
                        delta_chunk_len.min(ref_delta.decompressed_length - read_bytes);

                    let delta_chunk = &pack_delta_buffer[..delta_chunk_limit.try_into().unwrap()];

                    let instruction = delta_instruction(delta_chunk);

                    match instruction {
                        Ok((input, instruction)) => {
                            match instruction {
                                DeltaInstruction::InsertData(data) => {
                                    pack.write_all(data).await.context("writing to pack file")?;
                                    sha1_hasher.update(data);
                                    written_bytes += data.len() as u64;
                                }
                                DeltaInstruction::CopyFromParent {
                                    mut offset,
                                    mut length,
                                } => {
                                    pack_parent_file
                                        .seek(SeekFrom::Start(parent.file_offset))
                                        .await
                                        .context("seek in pack file for parent")?;
                                    let mut pack_parent =
                                        ZlibDecoder::new(BufReader::new(pack_parent_file));
                                    pack_parent.multiple_members(true);

                                    ensure!(
                                        offset + length <= parent.decompressed_length,
                                        "offset ({}) and length ({}) outside of parent length ({})",
                                        offset,
                                        length,
                                        parent.decompressed_length
                                    );

                                    while offset != 0 || length != 0 {
                                        pack_parent
                                            .read_buf(&mut pack_parent_buffer)
                                            .await
                                            .context("reading parent")?;

                                        let parent_chunk_len: u64 =
                                            pack_parent_buffer.len().try_into().unwrap();

                                        tracing::trace!(
                                            "chunk {}/{} at {}",
                                            parent_chunk_len,
                                            length,
                                            offset
                                        );

                                        if offset >= parent_chunk_len {
                                            offset -= parent_chunk_len;
                                        } else {
                                            // Make sure this is within range of usize.
                                            let parent_chunk_len =
                                                (parent_chunk_len - offset).min(length);
                                            let parent_chunk = &pack_parent_buffer[offset as usize
                                                ..(offset + parent_chunk_len) as usize];
                                            pack.write_all(parent_chunk)
                                                .await
                                                .context("writing to pack file")?;
                                            sha1_hasher.update(parent_chunk);

                                            written_bytes += parent_chunk_len;
                                            length -= parent_chunk_len;
                                            offset = 0;
                                        }

                                        pack_parent_buffer.truncate(0);
                                    }

                                    pack_parent_file = pack_parent.into_inner().into_inner();
                                }
                            }

                            let read_length = delta_chunk.len() - input.len();
                            // TODO: should this use the pack_delta_buffer length?
                            let unread_length = input.len();
                            pack_delta_buffer.copy_within(read_length.., 0);
                            pack_delta_buffer.truncate(unread_length);
                            read_bytes += read_length as u64;
                        }
                        Err(nom::Err::Incomplete(_)) => break,
                        Err(err) => Err(anyhow!("error parsing delta instruction: {:?}", err))?,
                    }

                    // Sanity check, replace with a limit on the whole packfile.
                    ensure!(written_bytes < 1_000_000_000);
                }

                if ref_delta.decompressed_length <= read_bytes {
                    ensure!(
                        read_bytes == ref_delta.decompressed_length,
                        "read bytes ({}) != expected length ({})",
                        read_bytes,
                        ref_delta.decompressed_length
                    );
                    ensure!(
                        written_bytes == reconstructed_length,
                        "written bytes ({}) != expected length ({}))",
                        written_bytes,
                        reconstructed_length
                    );
                    break;
                }
            }

            pack.shutdown().await?;
            self.pack = pack.into_inner();

            let sha1_hash = {
                let mut sha1_hash = [0u8; 20];
                let output = sha1_hasher.finalize_reset();
                sha1_hash.copy_from_slice(&output);
                sha1_hash
            };

            let value = bincode::serialize(&IndexObject {
                type_: parent.type_,
                file_offset: self.next_object_file_offset,
                compressed_length: self.pack.metadata().await?.len() - pack_original_length,
                decompressed_length: written_bytes,
            })?;

            tracing::trace!("writing object {:?} to index", sha1_hash);

            let index = self.index.clone();
            task::spawn_blocking(move || index.put(&sha1_hash, value)).await??;

            pack_delta_file = pack_delta.into_inner().into_inner();

            pack_delta_buffer.truncate(0);
            pack_parent_buffer.truncate(0);
        }

        let index = self.index.clone();
        task::spawn_blocking(move || index.flush()).await??;

        self.pack.flush().await?;

        Ok(())
    }

    async fn list_ref_deltas(
        &self,
        from: Option<Box<[u8]>>,
    ) -> Result<Vec<(Box<[u8]>, IndexObject, IndexObject)>> {
        let index = self.index.clone();

        task::spawn_blocking(move || {
            let mode = match from.as_ref() {
                Some(from) => rocksdb::IteratorMode::From(&from, rocksdb::Direction::Forward),
                None => rocksdb::IteratorMode::Start,
            };

            let mut ref_deltas = vec![];

            for (key, value) in index.iterator(mode) {
                if ref_deltas.len() == 127 {
                    break;
                }

                let value = bincode::deserialize::<IndexObject>(&value)?;

                let parent = if let PackFileObjectType::RefDelta { parent } = value.type_ {
                    parent
                } else {
                    continue;
                };

                if let Some(parent) = index.get(&parent)? {
                    let parent = bincode::deserialize::<IndexObject>(&parent)?;
                    ref_deltas.push((key, value, parent));
                }
            }

            Ok(ref_deltas)
        })
        .await?
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
            let (rest, decoded_object) = self.decoder.write(input, current_object).await?;

            let data_to_write = &input[..input.len() - rest.len()];

            tracing::trace!("writing {} bytes to pack", data_to_write.len());

            self.pack.write_all(data_to_write).await.unwrap();

            if let Some(decoded_object) = decoded_object {
                let key = decoded_object.sha1_hash;

                let value = bincode::serialize(&IndexObject {
                    type_: current_object.type_,
                    file_offset: self.next_object_file_offset,
                    compressed_length: decoded_object.compressed_length,
                    decompressed_length: decoded_object.decompressed_length,
                })
                .unwrap();

                tracing::trace!("writing object {:?} to index", decoded_object.sha1_hash);

                let index = self.index.clone();
                task::spawn_blocking(move || index.put(&key, value))
                    .await
                    .unwrap()
                    .unwrap();

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
            let (input, object_header) = pack_file_object_header(input)?;

            tracing::trace!("current_object: {:?}", object_header);

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

#[async_trait::async_trait]
pub trait LocalPackFileParser {
    type Output;

    async fn parse<'a>(
        &mut self,
        input: &'a [u8],
    ) -> nom::IResult<&'a [u8], Self::Output>;
}

struct LocalPackFileReader<Reader> {
    pack: Reader,
    pack_offset: u64,
    inflate_state: Box<InflateState>,
    inflate_offset: u64,
    input_buffer: Vec<u8>,
    output_buffer: Vec<u8>,
    output_buffer_offset: usize,
}

impl LocalPackFileReader<fs::File> {
    async fn open(path: impl AsRef<Path>) -> Result<LocalPackFileReader<fs::File>> {
        let pack = fs::OpenOptions::new().read(true).open(path).await?;

        Ok(Self::new(pack))
    }
}

impl<Reader> LocalPackFileReader<Reader>
where
    Reader: AsyncRead + AsyncSeek + Unpin,
{
    fn new(pack: Reader) -> Self {
        Self::with_capacity(pack, 8192)
    }

    fn with_capacity(pack: Reader, capacity: usize) -> Self {
        let inflate_state = InflateState::new_boxed(miniz_oxide::DataFormat::Zlib);

        let input_buffer = Vec::with_capacity(capacity);
        let output_buffer = vec![0u8; capacity];

        Self {
            pack,
            pack_offset: 0,
            inflate_state,
            inflate_offset: 0,
            input_buffer,
            output_buffer,
            output_buffer_offset: 0,
        }
    }

    fn start_offset(&self, object: &IndexObject, range: impl std::ops::RangeBounds<u64>) -> u64 {
        use std::ops::Bound;

        match range.start_bound() {
            Bound::Included(&start_offset) => {
                assert!(start_offset < object.decompressed_length);
                start_offset
            }
            Bound::Excluded(&start_offset) => {
                assert!(start_offset + 1 < object.decompressed_length);
                start_offset + 1
            }
            Bound::Unbounded => 0,
        }
    }

    fn end_offset(&self, object: &IndexObject, range: impl std::ops::RangeBounds<u64>) -> u64 {
        use std::ops::Bound;

        match range.end_bound() {
            Bound::Included(&end_offset) => {
                assert!(end_offset + 1 <= object.decompressed_length);
                end_offset + 1
            }
            Bound::Excluded(&end_offset) => {
                assert!(end_offset <= object.decompressed_length);
                end_offset
            }
            Bound::Unbounded => object.decompressed_length,
        }
    }

    async fn seek(&mut self, object: &IndexObject, start_offset: u64) -> Result<()> {
        use miniz_oxide::DataFormat;

        let object_end = object.file_offset + object.compressed_length;
        let buffer_start = self.inflate_offset - self.output_buffer_offset as u64;

        if self.pack_offset < object.file_offset
            || self.pack_offset > object_end
            || buffer_start > start_offset
        {
            self.pack_offset = object.file_offset;
            self.pack.seek(SeekFrom::Start(self.pack_offset)).await?;

            self.inflate_state.reset(DataFormat::Zlib);
            self.inflate_offset = 0;

            self.input_buffer.clear();
            self.output_buffer_offset = 0;
        }

        Ok(())
    }

    async fn read<'a>(
        &'a mut self,
        object: &IndexObject,
        range: impl std::ops::RangeBounds<u64> + Clone,
    ) -> Result<Option<(usize, usize)>> {
        use miniz_oxide::{inflate::stream::inflate, DataFormat, MZError, MZFlush};

        let start_offset = self.start_offset(&object, range.clone());
        let end_offset = self.end_offset(&object, range.clone());

        let object_end = object.file_offset + object.compressed_length;

        if self.pack_offset <= object.file_offset || self.pack_offset > object_end {
            self.pack_offset = object.file_offset;
            self.pack.seek(SeekFrom::Start(self.pack_offset)).await?;

            self.inflate_state.reset(DataFormat::Zlib);
            self.inflate_offset = 0;

            self.input_buffer.clear();
            self.output_buffer_offset = 0;
        }

        loop {
            let buffer_start_offset = self.inflate_offset - self.output_buffer_offset as u64;

            if self.output_buffer_offset > 0 && start_offset < self.inflate_offset && end_offset >= buffer_start_offset {
                let chunk_start_offset =
                    start_offset.max(buffer_start_offset) - buffer_start_offset;
                let chunk_end_offset = end_offset.min(self.inflate_offset) - buffer_start_offset;

                return Ok(Some((chunk_start_offset as usize, chunk_end_offset as usize)));
            } else if end_offset <= buffer_start_offset {
                return Ok(None);
            } else {
                self.output_buffer_offset = 0;
            }

            self.pack.read_buf(&mut self.input_buffer).await?;

            let result = inflate(
                &mut self.inflate_state,
                &self.input_buffer,
                &mut self.output_buffer[self.output_buffer_offset..],
                MZFlush::None,
            );

            self.input_buffer.copy_within(result.bytes_consumed.., 0);
            self.input_buffer
                .truncate(self.input_buffer.len() - result.bytes_consumed);

            self.pack_offset += result.bytes_consumed as u64;
            self.inflate_offset += result.bytes_written as u64;
            self.output_buffer_offset += result.bytes_written;

            match result.status {
                Ok(_) => continue,
                Err(MZError::Buf) => continue,
                Err(err) => return Err(anyhow!("inflate error: {:?}", err)),
            }
        }
    }

    async fn read_bytes<'a>(
        &'a mut self,
        object: &IndexObject,
        range: impl std::ops::RangeBounds<u64> + Clone,
    ) -> Result<&'a [u8]> {
        if let Some((chunk_start_offset, chunk_end_offset)) = self.read(object, range).await? {
            let buffer = &self.output_buffer[chunk_start_offset..chunk_end_offset];
            self.output_buffer_offset = 0;
            Ok(buffer)
        } else {
            Ok(&[])
        }
    }

    async fn parse<'a, P: LocalPackFileParser>(
        &'a mut self,
        object: &IndexObject,
        range: impl std::ops::RangeBounds<u64> + Clone,
        parser: &mut P,
    ) -> Result<Option<P::Output>> {
        while let Some((chunk_start_offset, chunk_end_offset)) = self.read(object, range.clone()).await? {
            let mut output_buffer = std::mem::replace(&mut self.output_buffer, vec![]);

            let input = &output_buffer[chunk_start_offset..chunk_end_offset];

            let result = parser.parse(input).await;
            match result {
                Ok((input, result)) => {
                    let consumed_bytes = chunk_end_offset - chunk_start_offset - input.len();

                    output_buffer.copy_within(chunk_start_offset + consumed_bytes.., 0);
                    self.output_buffer_offset -= chunk_start_offset + consumed_bytes;
                    self.output_buffer = output_buffer;

                    return Ok(Some(result));
                }
                Err(nom::Err::Incomplete(_)) => {
                    output_buffer.copy_within(chunk_start_offset.., 0);
                    self.output_buffer_offset -= chunk_start_offset;
                    self.output_buffer = output_buffer;
                }
                Err(nom::Err::Error(err)) => return Err(anyhow!("{:?}", err)),
                Err(nom::Err::Failure(err)) => return Err(anyhow!("{:?}", err)),
            }
        }

        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    const LOCAL_PACK: &[u8] = &[
        0x78, 0x9c, 0x4b, 0xcb, 0x53, 0xc8, 0x4d, 0xcc, 0xcc, 0xd3, 0xd0, 0x54, 0xa8, 0xe6, 0x52,
        0x00, 0x82, 0x82, 0xa2, 0xcc, 0xbc, 0x92, 0x9c, 0x3c, 0x45, 0x0d, 0x25, 0x8f, 0xd4, 0x9c,
        0x9c, 0x7c, 0x1d, 0x85, 0xf2, 0xfc, 0xa2, 0x9c, 0x14, 0x45, 0x25, 0x4d, 0x6b, 0xae, 0x5a,
        0x2e, 0x00, 0x35, 0xfa, 0x0d, 0x22, 0x78, 0x9c, 0x2b, 0x28, 0x4d, 0x52, 0xc8, 0xcd, 0x4f,
        0x51, 0x48, 0x4a, 0x4c, 0xce, 0x4e, 0xcd, 0x4b, 0x29, 0xb6, 0xe6, 0x52, 0x8e, 0xce, 0x4d,
        0x4c, 0x2e, 0xca, 0x8f, 0x2f, 0x2d, 0x4e, 0x8d, 0xe5, 0x2a, 0x80, 0x4a, 0x03, 0x71, 0x6a,
        0x0e, 0x50, 0x12, 0xc6, 0x2f, 0x4e, 0x2d, 0x2a, 0xcb, 0x4c, 0x4e, 0x05, 0x8a, 0x00, 0x00,
        0xfa, 0x85, 0x16, 0xeb,
    ];

    const LOCAL_PACK_OBJECT_A: IndexObject = IndexObject {
        type_: PackFileObjectType::Blob,
        file_offset: 0,
        compressed_length: 51,
        decompressed_length: 45,
    };

    const LOCAL_PACK_OBJECT_B: IndexObject = IndexObject {
        type_: PackFileObjectType::Blob,
        file_offset: 51,
        compressed_length: 58,
        decompressed_length: 65,
    };

    #[tokio::test]
    async fn local_pack_read_full() {
        let local_pack = Cursor::new(LOCAL_PACK);
        let mut local_pack = LocalPackFileReader::new(local_pack);

        local_pack.seek(&LOCAL_PACK_OBJECT_A, 0).await.unwrap();

        let data = local_pack
            .read_bytes(&LOCAL_PACK_OBJECT_A, ..)
            .await
            .unwrap();
        assert_eq!(data, b"fn main() {\n    println!(\"Hello, world!\");\n}\n");
    }

    #[tokio::test]
    async fn local_pack_read_chunks() {
        let local_pack = Cursor::new(LOCAL_PACK);
        let mut local_pack = LocalPackFileReader::with_capacity(local_pack, 16);

        local_pack.seek(&LOCAL_PACK_OBJECT_A, 0).await.unwrap();

        let data = local_pack
            .read_bytes(&LOCAL_PACK_OBJECT_A, ..)
            .await
            .unwrap();
        assert_eq!(data, b"fn main() {\n ");

        let data = local_pack
            .read_bytes(&LOCAL_PACK_OBJECT_A, ..)
            .await
            .unwrap();
        assert_eq!(data, b"   println!(\"Hel");

        let data = local_pack
            .read_bytes(&LOCAL_PACK_OBJECT_A, ..)
            .await
            .unwrap();
        assert_eq!(data, b"lo");

        let data = local_pack
            .read_bytes(&LOCAL_PACK_OBJECT_A, ..)
            .await
            .unwrap();
        assert_eq!(data, b", world!\");\n}\n");
    }

    #[tokio::test]
    async fn local_pack_read_partial_start() {
        let local_pack = Cursor::new(LOCAL_PACK);
        let mut local_pack = LocalPackFileReader::new(local_pack);

        local_pack.seek(&LOCAL_PACK_OBJECT_A, 16).await.unwrap();

        let data = local_pack
            .read_bytes(&LOCAL_PACK_OBJECT_A, 16..)
            .await
            .unwrap();
        assert_eq!(data, b"println!(\"Hello, world!\");\n}\n");
    }

    #[tokio::test]
    async fn local_pack_read_partial_end() {
        let local_pack = Cursor::new(LOCAL_PACK);
        let mut local_pack = LocalPackFileReader::new(local_pack);

        local_pack.seek(&LOCAL_PACK_OBJECT_A, 0).await.unwrap();

        let data = local_pack
            .read_bytes(&LOCAL_PACK_OBJECT_A, ..9)
            .await
            .unwrap();
        assert_eq!(data, b"fn main()");
    }

    #[tokio::test]
    async fn local_pack_read_partial_range() {
        let local_pack = Cursor::new(LOCAL_PACK);
        let mut local_pack = LocalPackFileReader::new(local_pack);

        local_pack.seek(&LOCAL_PACK_OBJECT_A, 26).await.unwrap();

        let data = local_pack
            .read_bytes(&LOCAL_PACK_OBJECT_A, 26..39)
            .await
            .unwrap();
        assert_eq!(data, b"Hello, world!");
    }

    #[tokio::test]
    async fn local_pack_read_partial_range_buf() {
        let local_pack = Cursor::new(LOCAL_PACK);
        let mut local_pack = LocalPackFileReader::new(local_pack);

        local_pack.seek(&LOCAL_PACK_OBJECT_A, 0).await.unwrap();

        let data = local_pack
            .read_bytes(&LOCAL_PACK_OBJECT_A, ..)
            .await
            .unwrap();
        assert_eq!(data, b"fn main() {\n    println!(\"Hello, world!\");\n}\n");

        local_pack.seek(&LOCAL_PACK_OBJECT_A, 26).await.unwrap();

        let data = local_pack
            .read_bytes(&LOCAL_PACK_OBJECT_A, 26..39)
            .await
            .unwrap();
        assert_eq!(data, b"Hello, world!");
    }

    #[tokio::test]
    async fn local_pack_read_ab_partial_range_buf() {
        let local_pack = Cursor::new(LOCAL_PACK);
        let mut local_pack = LocalPackFileReader::new(local_pack);

        local_pack.seek(&LOCAL_PACK_OBJECT_A, 26).await.unwrap();

        let data = local_pack
            .read_bytes(&LOCAL_PACK_OBJECT_A, 26..39)
            .await
            .unwrap();
        assert_eq!(data, b"Hello, world!");

        local_pack.seek(&LOCAL_PACK_OBJECT_B, 31).await.unwrap();

        let data = local_pack
            .read_bytes(&LOCAL_PACK_OBJECT_B, 31..46)
            .await
            .unwrap();
        assert_eq!(data, b"pub mod models;");
    }

    #[tokio::test]
    async fn local_pack_read_ba_partial_range_buf() {
        let local_pack = Cursor::new(LOCAL_PACK);
        let mut local_pack = LocalPackFileReader::new(local_pack);

        local_pack.seek(&LOCAL_PACK_OBJECT_B, 31).await.unwrap();

        let data = local_pack
            .read_bytes(&LOCAL_PACK_OBJECT_B, 31..46)
            .await
            .unwrap();
        assert_eq!(data, b"pub mod models;");

        local_pack.seek(&LOCAL_PACK_OBJECT_A, 26).await.unwrap();

        let data = local_pack
            .read_bytes(&LOCAL_PACK_OBJECT_A, 26..39)
            .await
            .unwrap();
        assert_eq!(data, b"Hello, world!");
    }

    #[tokio::test]
    async fn local_pack_parse() {
        let local_pack = Cursor::new(LOCAL_PACK);
        let mut local_pack = LocalPackFileReader::new(local_pack);

        local_pack.seek(&LOCAL_PACK_OBJECT_A, 0).await.unwrap();

        struct Parser;

        #[async_trait::async_trait]
        impl LocalPackFileParser for Parser {
            type Output = Vec<u8>;

            async fn parse<'a>(
                &mut self,
                input: &'a [u8],
            ) -> nom::IResult<&'a [u8], Self::Output> {
                let (input, _) = nom::bytes::streaming::tag(b"fn")(input)?;
                let (input, _) = nom::bytes::streaming::tag(b" ")(input)?;
                let (input, fn_name) = nom::bytes::streaming::take_until("(")(input)?;

                Ok((input, fn_name.to_vec()))
            }
        }

        let mut parser = Parser;

        let result = local_pack.parse(&LOCAL_PACK_OBJECT_A, .., &mut parser).await.unwrap();
        assert_eq!(result, Some(b"main".to_vec()));
    }

    #[tokio::test]
    async fn local_pack_parse_multiple() {
        let local_pack = Cursor::new(LOCAL_PACK);
        let mut local_pack = LocalPackFileReader::new(local_pack);

        local_pack.seek(&LOCAL_PACK_OBJECT_A, 0).await.unwrap();

        struct Parser;

        #[async_trait::async_trait]
        impl LocalPackFileParser for Parser {
            type Output = Vec<u8>;

            async fn parse<'a>(
                &mut self,
                input: &'a [u8],
            ) -> nom::IResult<&'a [u8], Self::Output> {
                let (input, name) = nom::bytes::streaming::take_until(" ")(input)?;
                let (input, _) = nom::bytes::streaming::tag(" ")(input)?;

                Ok((input, name.to_vec()))
            }
        }

        let mut parser = Parser;

        let result = local_pack.parse(&LOCAL_PACK_OBJECT_A, .., &mut parser).await.unwrap();
        assert_eq!(result, Some(b"fn".to_vec()));

        let result = local_pack.parse(&LOCAL_PACK_OBJECT_A, .., &mut parser).await.unwrap();
        assert_eq!(result, Some(b"main()".to_vec()));

        let result = local_pack.parse(&LOCAL_PACK_OBJECT_A, .., &mut parser).await.unwrap();
        assert_eq!(result, Some(b"{\n".to_vec()));
    }
}
