use anyhow::{anyhow, Result};
use std::{collections::BTreeMap, path::Component, path::PathBuf};
use tokio::{
    fs,
    io::{AsyncReadExt, AsyncSeekExt},
};

use super::auth::{Authentication, LayerPermissions, LayerSetPermissions};
use crate::{
    models,
    repositories::{
        DomainRepository, FileRepository, GitSourceRepository, LayerPendingLayersFilter,
        LayerRepository, SourceRepository,
    },
};

pub struct BuildService {
    layer_repository: &'static dyn LayerRepository,
    source_repository: &'static dyn SourceRepository,
    git_source_repository: &'static dyn GitSourceRepository,
    file_repository: &'static dyn FileRepository,
    domain_repository: &'static dyn DomainRepository,
    worker_id: models::WorkerId,
}

impl BuildService {
    pub fn new(
        layer_repository: &'static dyn LayerRepository,
        source_repository: &'static dyn SourceRepository,
        git_source_repository: &'static dyn GitSourceRepository,
        file_repository: &'static dyn FileRepository,
        domain_repository: &'static dyn DomainRepository,
    ) -> BuildService {
        BuildService {
            layer_repository,
            source_repository,
            git_source_repository,
            file_repository,
            domain_repository,
            worker_id: models::WorkerId::new(),
        }
    }

    pub async fn build(&self) -> Result<()> {
        let pending_layers = self
            .layer_repository
            .get_pending_layers(LayerPendingLayersFilter::Building)
            .await?;

        for layer in pending_layers {
            let layer_set = self
                .layer_repository
                .get_layer_set(layer.project_id, &layer.layer_set_name)
                .await?
                .unwrap();

            match layer_set.build_status {
                models::LayerSetBuildStatus {
                    last_layer_id: Some(last_layer_id),
                    ..
                } if last_layer_id > layer.id => {
                    // This layer is stale and cannot be built.
                    self.layer_repository
                        .cancel_layer(layer.project_id, &layer.layer_set_name, layer.id)
                        .await?;
                    continue;
                }
                models::LayerSetBuildStatus {
                    current_layer_id: Some(current_layer_id),
                    ..
                } if current_layer_id != layer.id => {
                    // Another layer is being built, this one needs to wait.
                    continue;
                }
                _ => (),
            }

            let layer_id = layer.id;

            let result = self.build_single(layer_set.clone(), layer.clone()).await;

            if let Err(err) = result {
                tracing::error!("error building layer ({}): {:?}", layer_id.into_uuid(), err);
            }

            let result = self.finalize_single(layer_set, layer).await;

            if let Err(err) = result {
                tracing::error!(
                    "error finalizing layer ({}): {:?}",
                    layer_id.into_uuid(),
                    err
                );
            }
        }

        let pending_layers = self
            .layer_repository
            .get_pending_layers(LayerPendingLayersFilter::Finalizing)
            .await?;

        for layer in pending_layers {
            let layer_set = self
                .layer_repository
                .get_layer_set(layer.project_id, &layer.layer_set_name)
                .await?
                .unwrap();

            let layer_id = layer.id;

            let result = self.finalize_single(layer_set, layer).await;

            if let Err(err) = result {
                tracing::error!(
                    "error finalizing layer ({}): {:?}",
                    layer_id.into_uuid(),
                    err
                );
            }
        }

        Ok(())
    }

    async fn build_single(&self, layer_set: models::LayerSet, layer: models::Layer) -> Result<()> {
        self.layer_repository
            .try_set_current_build(layer.project_id, &layer.layer_set_name, layer.id)
            .await?;

        self.layer_repository
            .build_layer(
                layer.project_id,
                &layer.layer_set_name,
                layer.id,
                self.worker_id,
            )
            .await?;

        let mut path = PathBuf::new();
        path.push(".data");
        path.push("builds");
        path.push(layer.id.into_uuid().to_string());

        fs::create_dir_all(&path).await?;

        match (layer_set.source, layer.source) {
            (
                Some(models::LayerSetSource {
                    name,
                    kind: models::LayerSetSourceKind::Git { ref_ },
                }),
                Some(models::LayerSource::Git { commit }),
            ) => {
                let source = self
                    .source_repository
                    .get_source(&layer.project_id, &name)
                    .await?
                    .unwrap();

                let source = source.try_with_kind()?;

                let ref_and_commit = models::GitSourceRefAndCommit { ref_, commit };

                path = self
                    .git_source_repository
                    .git_clone(&source, &ref_and_commit, path.clone())
                    .await?;
            }
            (None, None) => (),
            _ => return Err(anyhow!("unsupported combination of sources")),
        }

        let base_path = path.clone();
        let mut paths = vec![path];
        let mut changes = vec![];

        while let Some(path) = paths.pop() {
            let mut dir = fs::read_dir(&path).await?;

            while let Some(entry) = dir.next_entry().await? {
                let file_type = entry.file_type().await?;

                if file_type.is_dir() {
                    paths.push(entry.path());
                } else if file_type.is_file() {
                    let path = entry.path();
                    let rel_path = path.strip_prefix(&base_path)?;

                    let mut file = fs::OpenOptions::new().read(true).open(&path).await?;
                    let metadata = file.metadata().await?;

                    let mut hasher = models::FileChecksum::blake2b_hasher(layer.project_id);

                    let mut buffer = vec![0u8; 1 << 20];
                    while file.read(&mut buffer).await? != 0 {
                        hasher.update(&buffer);
                    }

                    let checksum = hasher.finalize();

                    let file_exists = self
                        .file_repository
                        .get_file(layer.project_id, &checksum)
                        .await?
                        .is_some();

                    if !file_exists {
                        let mut file_offset = 0;
                        let mut buffer_offset = 0;

                        file.rewind().await?;

                        loop {
                            let read = file.read(&mut buffer).await?;
                            buffer_offset += read;

                            if buffer_offset == buffer.len() || buffer_offset > 0 && read == 0 {
                                let mut data = vec![0u8; buffer_offset];
                                data.copy_from_slice(&buffer[..buffer_offset]);

                                self.file_repository
                                    .create_chunk(
                                        layer.project_id,
                                        &checksum,
                                        metadata.len(),
                                        file_offset,
                                        data,
                                    )
                                    .await?;

                                if buffer_offset < buffer.len() && read == 0 {
                                    break;
                                }

                                file_offset += buffer_offset as u64;
                                buffer_offset = 0;
                            }
                        }

                        self.file_repository
                            .finish_file(layer.project_id, &checksum, metadata.len())
                            .await?;
                    }

                    let mut headers = BTreeMap::new();

                    let content_type =
                        path.extension()
                            .and_then(|s| s.to_str())
                            .and_then(|s| match s {
                                "html" | "htm" => Some("text/html"),
                                "css" => Some("text/stylesheet"),
                                _ => None,
                            });

                    if let Some(content_type) = content_type {
                        headers.insert("content-type".into(), content_type.into());
                    }

                    let mut path = String::new();
                    for component in rel_path.components() {
                        if let Component::Normal(s) = component {
                            path.push('/');
                            path.push_str(&s.to_string_lossy());
                        }
                    }

                    let index_path = path
                        .strip_suffix("/index.html")
                        .or_else(|| path.strip_suffix("/index.htm"));

                    if let Some(index_path) = index_path {
                        let mut path = String::with_capacity(index_path.len() + 1);
                        path.push_str(index_path);
                        path.push('/');

                        changes.push(models::LayerChange {
                            project_id: layer.project_id,
                            layer_set_name: layer.layer_set_name.clone(),
                            layer_id: layer.id,
                            worker_id: self.worker_id,
                            path,
                            checksum,
                            content_encoding_hint: models::ContentEncodingHint::Relative {
                                identity: 1,
                                gzip: 0,
                                zstd: 0,
                                brotli: 0,
                            },
                            headers: headers.clone(),
                        });
                    }

                    changes.push(models::LayerChange {
                        project_id: layer.project_id,
                        layer_set_name: layer.layer_set_name.clone(),
                        layer_id: layer.id,
                        worker_id: self.worker_id,
                        path,
                        checksum,
                        content_encoding_hint: models::ContentEncodingHint::Relative {
                            identity: 1,
                            gzip: 0,
                            zstd: 0,
                            brotli: 0,
                        },
                        headers,
                    });
                }
            }

            if changes.len() > 128 {
                self.layer_repository.create_layer_changes(&changes).await?;
                changes.clear();
            }
        }

        if !changes.is_empty() {
            self.layer_repository.create_layer_changes(&changes).await?;
        }

        self.layer_repository
            .finish_build(
                layer.project_id,
                &layer.layer_set_name,
                layer.id,
                self.worker_id,
            )
            .await?;

        Ok(())
    }

    async fn finalize_single(
        &self,
        layer_set: models::LayerSet,
        layer: models::Layer,
    ) -> Result<()> {
        self.layer_repository
            .try_set_current_build(layer.project_id, &layer.layer_set_name, layer.id)
            .await?;

        self.layer_repository
            .finalize_layer(
                layer.project_id,
                &layer.layer_set_name,
                layer.id,
                self.worker_id,
            )
            .await?;

        let layer_changes = self
            .layer_repository
            .list_layer_changes(
                layer.project_id,
                &layer.layer_set_name,
                layer.id,
                self.worker_id,
            )
            .await?;

        for layer_change in layer_changes {
            let layer_member = models::LayerMember {
                project_id: layer_change.project_id,
                layer_set_name: layer_change.layer_set_name.clone(),
                layer_id: layer_change.layer_id,
                path: layer_change.path.clone(),
                checksum: layer_change.checksum,
                content_encoding_hint: layer_change.content_encoding_hint,
                headers: layer_change.headers.clone(),
            };

            self.layer_repository
                .create_layer_members(&[layer_member])
                .await?;
        }

        self.layer_repository
            .finish_finalizing(
                layer.project_id,
                &layer.layer_set_name,
                layer.id,
                self.worker_id,
            )
            .await?;

        let fqdn = format!("{}.localhost", layer.id.into_uuid().as_hyphenated());

        /*
        self.domain_repository
            .create_validated_domain(
                models::ValidatedDomain {
                    fqdn,
                    project_id: layer.project_id,
                    kind: models::ValidatedDomainKind::Layer {
                        layer_set_name: layer.layer_set_name.clone(),
                        layer_id: layer.id,
                    },
                },
                models::ValidatedDomainCertificate {
                    private_key: vec![],
                    public_key_chain: vec![],
                },
            )
            .await?;
        */

        Ok(())
    }
}
