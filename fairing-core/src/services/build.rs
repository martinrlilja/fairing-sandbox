use anyhow::{anyhow, ensure, Context as _, Result};
use futures_util::{pin_mut, stream::FuturesUnordered, StreamExt};
use std::{
    io::ErrorKind,
    path::{Path, PathBuf},
};
use tokio::{fs, io::AsyncReadExt, task};
use tracing::Instrument as _;

use crate::{
    backends::{BuildQueue, Database, FileMetadata, RemoteSource},
    models::{self, prelude::*},
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
        file_metadata: FileMetadata,
        remote_source: RemoteSource,
        storage: Storage,
    ) -> BuildService {
        BuildService {
            build_queue,
            database,
            file_metadata,
            remote_source,
            storage,
            concurrent_builds: self.concurrent_builds,
            build_tasks: FuturesUnordered::new(),
        }
    }
}

pub struct BuildService {
    build_queue: BuildQueue,
    database: Database,
    file_metadata: FileMetadata,
    remote_source: RemoteSource,
    storage: Storage,
    concurrent_builds: usize,
    build_tasks: FuturesUnordered<task::JoinHandle<()>>,
}

impl BuildService {
    pub async fn run(mut self) -> Result<()> {
        let stream = self.build_queue.stream_builds().await?;
        pin_mut!(stream);

        while let Some(build) = stream.next().await {
            let build = build?;
            let build_task = BuildTask {
                database: self.database.clone(),
                file_metadata: self.file_metadata.clone(),
                remote_source: self.remote_source.clone(),
                storage: self.storage.clone(),
                build,
            };

            let span = tracing::info_span!(
                "build_task",
                build_name = build_task.build.name.name(),
                layer_id = ?build_task.build.layer_id,
                source_reference = %build_task.build.source_reference,
            );

            let build_task = async move {
                tracing::info!("starting build");
                let res = build_task.run().await;
                if let Err(err) = res {
                    tracing::error!("build failed with {:?}", err);
                } else {
                    tracing::info!("build succeded");
                }
            }
            .instrument(span);

            self.build_tasks.push(tokio::task::spawn(build_task));

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
    file_metadata: FileMetadata,
    remote_source: RemoteSource,
    storage: Storage,
    build: models::Build,
}

impl BuildTask {
    async fn run(self) -> Result<()> {
        let layer_set_name = self.build.name.parent();
        let layer_set = self
            .database
            .get_layer_set(&layer_set_name)
            .await?
            .ok_or_else(|| anyhow!("layer set not found"))?;

        let source_name = self.build.name.parent().parent();
        let source = self
            .database
            .get_source(&source_name)
            .await?
            .ok_or_else(|| anyhow!("site source not found"))?;

        let team_name = source_name.parent();
        let team = self
            .database
            .get_team(&team_name)
            .await?
            .ok_or_else(|| anyhow!("failed to find team: {}", team_name.name()))?;

        let file_keyspace = self
            .file_metadata
            .get_file_keyspace(&team.file_keyspace_id)
            .await?
            .ok_or_else(|| {
                anyhow!(
                    "failed to find file keyspace for team: {}",
                    team_name.name()
                )
            })?;

        let work_directory = {
            let mut work_directory = PathBuf::new();
            work_directory.push(".build");
            fs::create_dir_all(&work_directory).await?;

            work_directory.push(self.build.name.resource());
            work_directory
        };

        fs::create_dir(&work_directory).await?;

        let source_directory = self
            .remote_source
            .fetch(&source, &self.build, work_directory.clone())
            .await?;

        let source_directory = fs::canonicalize(source_directory).await?;

        tracing::trace!("fetched source");

        let build_file = read_build_file(&source_directory).await?;

        let mut modules = vec![];
        for module in build_file.modules {
            let module_path = source_directory.join(&module.path);
            let module_path = fs::canonicalize(module_path).await?;
            ensure!(
                module_path.starts_with(&source_directory),
                "module path \"{}\" points outside of source directory",
                module.path
            );

            let extension = module_path.extension();

            let mut file = fs::OpenOptions::new()
                .read(true)
                .open(&module_path)
                .await
                .context("read module")?;
            let metadata = file.metadata().await?;

            let file_id = match extension {
                Some(extension) if extension == "wat" => {
                    ensure!(metadata.len() < 4_194_304);

                    let mut buf = Vec::with_capacity(metadata.len().try_into()?);
                    file.read_to_end(&mut buf).await?;

                    let module_data = wat::parse_bytes(&buf)?;

                    if let std::borrow::Cow::Borrowed(_) = module_data {
                        return Err(anyhow!(
                            "module \"{}\" is not a valid wat file",
                            module.path
                        ));
                    }

                    let module_data_len = module_data.len();

                    let stream = futures_util::stream::once(async move { Ok(module_data) });

                    self.storage
                        .store_file(&file_keyspace, module_data_len as i64, Box::pin(stream))
                        .await
                        .context("store module")?
                }
                _ => {
                    // TODO: parse the wasm file to make sure it is correct.
                    let stream = tokio_util::io::ReaderStream::new(file);

                    self.storage
                        .store_file(&file_keyspace, metadata.len() as i64, stream)
                        .await
                        .context("store module")?
                }
            };

            modules.push(models::DeploymentModule { file_id });
        }

        let publish_directory = source_directory.join(build_file.build.publish);
        let publish_directory = fs::canonicalize(publish_directory)
            .await
            .context("canonicalizing publish directory")?;

        ensure!(
            publish_directory.starts_with(&source_directory),
            "build publish directory cannot be outside of source directory"
        );

        let mut directories = vec![publish_directory.clone()];
        let mut stored_files = 0;

        while let Some(directory) = directories.pop() {
            let mut read_dir = fs::read_dir(directory).await.context("read directory")?;

            while let Some(entry) = read_dir.next_entry().await? {
                let entry_metadata = entry.metadata().await.context("read file metadata")?;

                if entry_metadata.is_dir() {
                    directories.push(entry.path());
                } else if entry_metadata.is_file() {
                    let file = fs::OpenOptions::new()
                        .read(true)
                        .open(entry.path())
                        .await
                        .context("open file")?;

                    let stream = tokio_util::io::ReaderStream::new(file);

                    let file_id = self
                        .storage
                        .store_file(&file_keyspace, entry_metadata.len() as i64, stream)
                        .await
                        .context("store file")?;

                    let file_path = entry.path();
                    let file_relative_path = file_path.strip_prefix(&publish_directory)?;
                    let path = file_relative_path.components().fold(
                        String::new(),
                        |mut path, component| {
                            let component = component.as_os_str().to_string_lossy();
                            path.push('/');
                            path.push_str(&component);
                            path
                        },
                    );

                    let layer_member = models::CreateLayerMember {
                        layer_set_id: layer_set.id,
                        layer_id: self.build.layer_id,
                        path,
                        file_id: Some(file_id),
                    };

                    self.file_metadata
                        .create_layer_member(&layer_member)
                        .await?;

                    stored_files += 1;
                }
            }
        }

        tracing::trace!("stored {} files", stored_files);

        tracing::trace!("cleaning work directory: {:?}", work_directory);

        fs::remove_dir_all(work_directory).await?;

        tracing::trace!("creating deployments");

        let sites = self
            .database
            .list_sites_with_base_source(&source_name)
            .await?;

        for site in sites {
            let deployment = models::CreateDeployment {
                parent: site.name.clone(),
                projections: vec![models::CreateDeploymentProjection {
                    layer_set: layer_set_name.clone(),
                    layer_id: self.build.layer_id,
                    mount_path: "",
                    sub_path: "",
                }],
                modules: modules.clone(),
            };

            let deployment = self.database.create_deployment(&deployment).await?;

            // TODO: only update this if the build is of the HEAD branch.
            self.database
                .update_current_deployment(&deployment.name)
                .await?;

            tracing::trace!("created deployment: {}", deployment.name.name());
        }

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
                modules: vec![],
            })
        }
        Err(err) => Err(err.into()),
    }
}

#[derive(Clone, Debug, serde::Deserialize)]
struct BuildFile {
    build: BuildFileBuild,
    #[serde(default)]
    modules: Vec<BuildFileModule>,
}

#[derive(Clone, Debug, serde::Deserialize)]
struct BuildFileBuild {
    publish: String,
}

#[derive(Clone, Debug, serde::Deserialize)]
struct BuildFileModule {
    name: String,
    path: String,
}
