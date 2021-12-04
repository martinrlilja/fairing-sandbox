use anyhow::{anyhow, ensure, Context as _, Result};
use memmap::{Mmap, MmapOptions};
use sha1::{Digest, Sha1};
use std::{
    convert::TryInto,
    fs::Permissions,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::{
    fs,
    io::{AsyncWriteExt, BufWriter},
    task,
};

use super::{
    parsers::{
        commit_object, delta_instruction, pack_file_variable_length, tree_item, DeltaInstruction,
        PackFileObjectType, TreeItem, TreeItemBlobMode,
    },
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

        let pack = fs::OpenOptions::new().read(true).open(pack_path).await?;

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

    pub async fn object(&self, key: [u8; 20]) -> Result<Option<(IndexObject, &[u8])>> {
        let index = self.index.clone();
        let object = task::spawn_blocking(move || index.get(&key))
            .await?
            .context("getting object from index")?;

        if let Some(object) = object {
            let object =
                bincode::deserialize::<IndexObject>(&object).context("deserializing object")?;

            let object_data = self.object_data(&object);

            Ok(object_data.map(|object_data| (object, object_data)))
        } else {
            Ok(None)
        }
    }

    pub fn object_data(&self, object: &IndexObject) -> Option<&[u8]> {
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

struct TreeParser<'a> {
    input: &'a [u8],
    path: PathBuf,
}

pub async fn extract(commit_key: [u8; 20], work_directory: impl AsRef<Path>) -> Result<PathBuf> {
    let work_directory = work_directory.as_ref();
    let mut pack_file_reader = LocalPackFileReader::open(work_directory).await?;
    let source_directory = work_directory.join("source");

    // Reconstruct ref deltas.
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
            let (reconstructed_key, reconstructed_object) =
                reconstruct(&ref_delta, &parent, &mut pack_file_reader).await?;

            let value = bincode::serialize(&reconstructed_object)?;

            let index = pack_file_reader.index();

            tracing::trace!("writing object {:?} to index", reconstructed_key);
            task::spawn_blocking(move || {
                index.put(reconstructed_key, value)?;
                index.delete(ref_delta_key)?;
                Ok::<_, anyhow::Error>(())
            })
            .await??;
        }

        pack_file_reader.refresh_pack_file().await?;
    }

    let (_commit, commit_data) = pack_file_reader
        .object(commit_key)
        .await?
        .ok_or_else(|| anyhow!("couldn't find commit"))?;

    let (_, commit) =
        commit_object(commit_data).map_err(|err| anyhow!("error parsing commit: {:?}", err))?;

    tracing::debug!("commit: {:?}", commit);

    let (_tree, tree_data) = pack_file_reader
        .object(commit.tree)
        .await?
        .ok_or_else(|| anyhow!("couldn't find tree"))?;

    fs::create_dir_all(&source_directory).await?;

    tracing::debug!("{:?}", tree_data);

    let path_build = fs::canonicalize(&source_directory).await?;
    let mut tree_parsers = vec![TreeParser {
        input: tree_data,
        path: path_build.clone(),
    }];

    while let Some(tree_parser) = tree_parsers.pop() {
        let (input, tree_item) = tree_item(tree_parser.input)
            .map_err(|err| anyhow!("error when parsing tree: {:?}", err))?;

        tracing::debug!("tree item: {:?}", tree_item);
        match tree_item {
            TreeItem::Blob { mode, hash, name } => {
                let (_blob, blob_data) = pack_file_reader
                    .object(hash)
                    .await?
                    .ok_or_else(|| anyhow!("couldn't find blob"))?;

                let path = tree_parser.path.join(name);
                let parent_path = fs::canonicalize(path.parent().unwrap()).await?;
                if !parent_path.starts_with(&path_build) {
                    return Err(anyhow!("path points outside of build directory"));
                }

                match mode {
                    TreeItemBlobMode::Normal => {
                        fs::write(&path, blob_data).await?;
                        fs::set_permissions(&path, Permissions::from_mode(0o644)).await?;
                    }
                    TreeItemBlobMode::Executable => {
                        fs::write(&path, blob_data).await?;
                        fs::set_permissions(&path, Permissions::from_mode(0o755)).await?;
                    }
                    TreeItemBlobMode::SymbolicLink => {
                        let target_path = std::str::from_utf8(blob_data)
                            .map_err(|_| anyhow!("symlink is not a valid string"))?;
                        fs::symlink(&path, target_path).await?;
                    }
                };
            }
            TreeItem::Tree { name, hash } => {
                let path = tree_parser.path.join(name);
                let parent_path = fs::canonicalize(path.parent().unwrap()).await?;
                if !parent_path.starts_with(&path_build) {
                    return Err(anyhow!("path points outside of build directory"));
                }

                fs::create_dir(&path).await?;

                let (_tree, tree_data) = pack_file_reader
                    .object(hash)
                    .await?
                    .ok_or_else(|| anyhow!("couldn't find tree"))?;

                tree_parsers.push(TreeParser {
                    input: tree_data,
                    path,
                });
            }
        }

        if !input.is_empty() {
            tree_parsers.push(TreeParser {
                input,
                ..tree_parser
            })
        }
    }

    Ok(source_directory)
}

async fn reconstruct<'a>(
    ref_delta: &IndexObject,
    parent: &IndexObject,
    reader: &mut LocalPackFileReader,
) -> Result<([u8; 20], IndexObject)> {
    let mut sha1_hasher = Sha1::new();
    let mut written_bytes = 0;

    let pack_file = reader.open_writer().await?;
    let pack_file_len = pack_file.metadata().await?.len();
    let mut pack_file = BufWriter::new(pack_file);

    let input = reader.object_data(ref_delta).unwrap();
    let parent_input = reader.object_data(parent).unwrap();

    let (input, _) = pack_file_variable_length(input).unwrap();
    let (input, reconstructed_length) = pack_file_variable_length(input).unwrap();

    match parent.type_ {
        PackFileObjectType::Commit => sha1_hasher.update(b"commit"),
        PackFileObjectType::Tree => sha1_hasher.update(b"tree"),
        PackFileObjectType::Blob => sha1_hasher.update(b"blob"),
        PackFileObjectType::Tag => sha1_hasher.update(b"tag"),
        PackFileObjectType::RefDelta { .. } => {
            return Err(anyhow!(
                "ref delta can not have another ref delta as a parent"
            ));
        }
    }

    let length_header = format!(" {}\0", reconstructed_length);
    sha1_hasher.update(length_header.as_bytes());

    let mut loop_input = input;
    while loop_input.len() > 0 {
        let (input, instruction) =
            nom::combinator::complete(delta_instruction)(loop_input).unwrap();
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
