use anyhow::{ensure, anyhow, Result, Context as _};
use sha1::{Digest, Sha1};
use memmap::{Mmap, MmapOptions};
use tokio::{task, fs, io::{BufWriter, AsyncWriteExt}};
use std::{sync::Arc, path::{Path, PathBuf}, convert::TryInto};

use super::{
    parsers::{delta_instruction, DeltaInstruction, pack_file_variable_length, PackFileObjectType},
    IndexObject,
};

struct LocalPackFileReader {
    index: Arc<rocksdb::DB>,
    pack_path: PathBuf,
    pack_mmap: Mmap,
}

impl LocalPackFileReader {
    pub async fn open(path: impl AsRef<Path>) -> Result<LocalPackFileReader> {
        let path = path.as_ref().to_owned();
        let datbase_path = path.join("index");
        let pack_path = path.join("pack");

        let index = task::spawn_blocking(|| rocksdb::DB::open_default(datbase_path)).await??;

        let pack_mmap = LocalPackFileReader::open_pack_mmap(&pack_path).await?;

        Ok(LocalPackFileReader {
            index: Arc::new(index),
            pack_path,
            pack_mmap,
        })
    }

    pub async fn open_writer(&self) -> Result<fs::File> {
        let pack_file = fs::OpenOptions::new()
            .append(true)
            .open(&self.pack_path)
            .await?;

        Ok(pack_file)
    }

    async fn open_pack_mmap(pack_path: impl AsRef<Path>) -> Result<Mmap> {
        let mut options = MmapOptions::new();

        let pack = fs::OpenOptions::new()
            .read(true)
            .open(pack_path)
            .await?;

        let pack_metadata = pack.metadata().await?;
        let pack_len: usize = pack_metadata.len().try_into()?;

        options.len(pack_len);

        let pack = &pack.into_std().await;

        let mmap = unsafe { options.map(pack)? };

        Ok(mmap)
    }

    pub async fn refresh_pack_file(&mut self) -> Result<()> {
        let pack_mmap = LocalPackFileReader::open_pack_mmap(&self.pack_path).await?;
        self.pack_mmap = pack_mmap;
        Ok(())
    }

    pub fn object(&self, object: &IndexObject) -> Option<&[u8]> {
        let object_start: usize = object.offset.try_into().ok()?;
        let object_end: usize = (object.offset + object.length).try_into().ok()?;

        if object_end <= self.pack_mmap.len() {
            Some(&self.pack_mmap[object_start..object_end])
        } else {
            None
        }
    }

    pub async fn list_ref_deltas(&self) -> Result<Vec<(Box<[u8]>, IndexObject, IndexObject)>> {
        let index = self.index.clone();

        task::spawn_blocking(move || {
            let mut ref_deltas = vec![];

            for (key, value) in index.iterator(rocksdb::IteratorMode::Start) {
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

    pub fn index(&self) -> Arc<rocksdb::DB> {
        self.index.clone()
    }
}

pub async fn extract(path: impl AsRef<Path>, path_build: impl AsRef<Path>) -> Result<()> {
    let mut pack_file_reader = LocalPackFileReader::open(path).await?;

    fs::create_dir_all(path_build).await?;

    loop {
        let ref_deltas = pack_file_reader
            .list_ref_deltas()
            .await
            .context("listing ref deltas")?;

        if ref_deltas.is_empty() {
            break;
        } else {
            tracing::debug!("reconstructing {} ref deltas", ref_deltas.len());
        }

        for (ref_delta_key, ref_delta, parent) in ref_deltas {
            let (reconstructed_key, reconstructed_object) = reconstruct(&ref_delta, &parent, &mut pack_file_reader).await?;

            let value = bincode::serialize(&reconstructed_object)?;

            let index = pack_file_reader.index();

            tracing::trace!("writing object {:?} to index", reconstructed_key);
            task::spawn_blocking(move || {
                index.put(reconstructed_key, value)?;
                index.delete(ref_delta_key)?;
                Ok::<_, anyhow::Error>(())
            }).await??;
        }

        pack_file_reader.refresh_pack_file().await?;
    }

    Ok(())
}

async fn reconstruct<'a>(ref_delta: &IndexObject, parent: &IndexObject, reader: &mut LocalPackFileReader) -> Result<([u8; 20], IndexObject)> {
    let mut sha1_hasher = Sha1::new();
    let mut written_bytes = 0;

    let pack_file = reader.open_writer().await?;
    let pack_file_len = pack_file.metadata().await?.len();
    let mut pack_file = BufWriter::new(pack_file);

    let input = reader.object(ref_delta).unwrap();
    let parent_input = reader.object(parent).unwrap();

    let (input, _) = pack_file_variable_length(input).unwrap();
    let (input, reconstructed_length) = pack_file_variable_length(input).unwrap();

    match parent.type_ {
        PackFileObjectType::Commit => sha1_hasher.update(b"commit"),
        PackFileObjectType::Tree => sha1_hasher.update(b"tree"),
        PackFileObjectType::Blob => sha1_hasher.update(b"blob"),
        PackFileObjectType::Tag => sha1_hasher.update(b"tag"),
        PackFileObjectType::RefDelta { .. } => {
            return Err(anyhow!("ref delta can not have another ref delta as a parent"));
        }
    }

    let length_header = format!(" {}\0", reconstructed_length);
    sha1_hasher.update(length_header.as_bytes());

    let mut loop_input = input;
    while loop_input.len() > 0 {
        let (input, instruction) = nom::combinator::complete(delta_instruction)(loop_input).unwrap();
        loop_input = input;

        match instruction {
            DeltaInstruction::InsertData(data) => {
                tracing::debug!("inserting data {}", data.len());

                pack_file
                    .write_all(data)
                    .await
                    .expect("writing to pack file");
                sha1_hasher.update(data);
                written_bytes += data.len() as u64;
            }
            DeltaInstruction::CopyFromParent { offset, length } => {
                tracing::debug!("copy from parent: {}/{}", offset, length);
                assert!(
                    offset + length <= parent.length,
                    "offset ({}) and length ({}) outside of parent length ({})",
                    offset,
                    length,
                    parent.length
                );

                // Since we know that parent.length is within usize, otherwise mmap wouldn't
                // work, we can just cast them here.
                let slice = &parent_input[offset as usize..(offset + length) as usize];
                pack_file.write_all(slice).await?;
                sha1_hasher.update(slice);
                written_bytes += slice.len() as u64;
            }
        }
    }

    pack_file.flush().await?;

    ensure!(written_bytes == reconstructed_length);

    let sha1_hash = {
        let mut sha1_hash = [0u8; 20];
        let output = sha1_hasher.finalize_reset();
        sha1_hash.copy_from_slice(&output);
        sha1_hash
    };

    let object = IndexObject {
        type_: parent.type_,
        offset: pack_file_len,
        length: written_bytes,
    };

    Ok((sha1_hash, object))
}
