use anyhow::{anyhow, Result};
use futures_util::{pin_mut, stream::FuturesUnordered, StreamExt};
use std::{
    io::ErrorKind,
    path::{Path, PathBuf},
};
use tokio::{fs, task};

use crate::{
    backends::{BuildQueue, Database, RemoteSiteSource},
    models::{prelude::*, TreeRevision},
    services::Storage,
};

pub struct BuildServiceBuilder {
    concurrent_builds: usize,
}

impl BuildServiceBuilder {
    pub fn new() -> BuildServiceBuilder {
        BuildServiceBuilder {
            concurrent_builds: 1,
        }
    }

    pub fn concurrent_builds(&mut self, concurrent_builds: usize) -> &mut BuildServiceBuilder {
        self.concurrent_builds = concurrent_builds;
        self
    }

    pub fn build(
        &self,
        build_queue: BuildQueue,
        database: Database,
        remote_site_source: RemoteSiteSource,
        storage: Storage,
    ) -> BuildService {
        BuildService {
            build_queue,
            database,
            remote_site_source,
            storage,
            concurrent_builds: self.concurrent_builds,
            build_tasks: FuturesUnordered::new(),
        }
    }
}

pub struct BuildService {
    build_queue: BuildQueue,
    database: Database,
    remote_site_source: RemoteSiteSource,
    storage: Storage,
    concurrent_builds: usize,
    build_tasks: FuturesUnordered<task::JoinHandle<()>>,
}

impl BuildService {
    pub async fn run(mut self) -> Result<()> {
        // TODO: make this loop better...

        let stream = self.build_queue.stream().await?;
        pin_mut!(stream);

        while let Some(tree_revision) = stream.next().await {
            let tree_revision = tree_revision?;
            let build_task = BuildTask {
                database: self.database.clone(),
                remote_site_source: self.remote_site_source.clone(),
                storage: self.storage.clone(),
                tree_revision,
            };

            let build_task = tokio::task::spawn(async move {
                let res = build_task.run().await;
                if let Err(err) = res {
                    tracing::error!("{:?}", err);
                }
            });

            self.build_tasks.push(build_task);

            if self.build_tasks.len() >= self.concurrent_builds {
                // If there are too many concurrent builds, wait for at least one of them to
                // complete.
                self.build_tasks.next().await;
            }
        }

        Ok(())
    }
}

struct BuildTask {
    database: Database,
    remote_site_source: RemoteSiteSource,
    storage: Storage,
    tree_revision: TreeRevision,
}

impl BuildTask {
    async fn run(self) -> Result<()> {
        let site_source_name = self.tree_revision.name.parent().parent();
        let site_source = self
            .database
            .get_site_source(&site_source_name)
            .await?
            .ok_or_else(|| anyhow!("site source not found"))?;

        tracing::trace!("running build");

        let work_directory: PathBuf = ".build".into();

        let source_directory = self
            .remote_site_source
            .fetch(&site_source, &self.tree_revision.name, work_directory)
            .await?;

        tracing::trace!("fetched source");

        let build_file = read_build_file(&source_directory).await?;

        tracing::debug!("build file: {:?}", build_file);

        Ok(())
    }
}

async fn read_build_file(path: impl AsRef<Path>) -> Result<BuildFile> {
    const BUILD_FILE_MAX_SIZE: u64 = 4_194_304;

    let build_file_path = path.as_ref().join("Fairing.toml");

    let build_file_metadata = fs::metadata(&build_file_path).await;
    match build_file_metadata {
        Ok(metadata) if metadata.len() > BUILD_FILE_MAX_SIZE => {
            Err(anyhow!("build file is too large"))
        }
        Ok(_) => {
            tracing::debug!("read build file from: {:?}", build_file_path);
            let build_file_data = fs::read(&build_file_path).await?;
            let build_file = toml::de::from_slice(&build_file_data)?;
            Ok(build_file)
        }
        Err(err) if err.kind() == ErrorKind::NotFound => {
            tracing::debug!("no build file found, using build file default");
            Ok(BuildFile {
                build: BuildFileBuild {
                    publish: ".".into(),
                },
            })
        }
        Err(err) => Err(err.into()),
    }
}

#[derive(Clone, Debug, serde::Deserialize)]
struct BuildFile {
    build: BuildFileBuild,
}

#[derive(Clone, Debug, serde::Deserialize)]
struct BuildFileBuild {
    publish: String,
}
