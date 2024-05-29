use anyhow::{anyhow, ensure, Result};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};
use tokio::{
    fs,
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader, BufWriter},
    sync::mpsc,
    task,
};

use fairing_core2::models;

use super::{SshClient, SshReader};

#[derive(Clone, Debug, serde::Deserialize)]
struct GitLfsAuthenticate {
    #[serde(default)]
    href: Option<String>,
    header: BTreeMap<String, String>,
}

#[derive(Clone, Debug, serde::Serialize)]
struct GitLfsBatchRequest<'a> {
    operation: GitLfsBatchRequestOperation,
    objects: &'a [GitLfsBatchRequestObject<'a>],
}

#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "lowercase")]
enum GitLfsBatchRequestOperation {
    Download,
}

#[derive(Clone, Debug, serde::Serialize)]
struct GitLfsBatchRequestObject<'a> {
    oid: &'a str,
    size: u64,
}

#[derive(Clone, Debug, serde::Deserialize)]
struct GitLfsBatchResponse {
    objects: Vec<GitLfsBatchResponseObject>,
}

#[derive(Clone, Debug, serde::Deserialize)]
struct GitLfsBatchResponseObject {
    oid: String,
    size: u64,
    #[serde(default)]
    authenticated: bool,
    actions: GitLfsBatchResponseObjectActions,
}

#[derive(Clone, Debug, serde::Deserialize)]
struct GitLfsBatchResponseObjectActions {
    download: GitLfsBatchResponseObjectActionDownload,
}

#[derive(Clone, Debug, serde::Deserialize)]
struct GitLfsBatchResponseObjectActionDownload {
    href: String,
    #[serde(default)]
    header: BTreeMap<String, String>,
}

/// Try to automatically detect if this directory might contain any files from git lfs.
#[tracing::instrument(skip_all)]
pub async fn detect(source_directory: impl AsRef<Path>) -> Result<bool> {
    let mut paths = vec![source_directory.as_ref().to_owned()];

    while let Some(path) = paths.pop() {
        let mut directory = fs::read_dir(&path).await?;

        while let Some(file) = directory.next_entry().await? {
            let file_type = file.file_type().await?;

            if file_type.is_dir() {
                paths.push(file.path());
            } else if file_type.is_file() && file.file_name() == ".gitattributes" {
                let lfs_globs = parse_attributes(file.path()).await?;
                if !lfs_globs.is_empty() {
                    return Ok(true);
                }
            }
        }
    }

    Ok(false)
}

/// Parse a .gitattributes file and return any glob patterns for git lfs files.
#[tracing::instrument(skip_all)]
async fn parse_attributes(path: impl AsRef<Path>) -> Result<Vec<glob::Pattern>> {
    let file = fs::OpenOptions::new().read(true).open(path).await?;
    let metadata = file.metadata().await?;

    ensure!(metadata.len() < 65_536);

    let file = BufReader::new(file);
    let mut lines = file.lines();

    let mut globs = vec![];

    while let Some(line) = lines.next_line().await? {
        let mut parts = line.split(' ');

        let pattern = parts.next().unwrap();

        if pattern.is_empty() || pattern.starts_with('#') {
            continue;
        }

        for attribute in parts {
            if attribute.is_empty() || attribute.starts_with('#') {
                continue;
            }

            if attribute == "filter=lfs" {
                let pattern = glob::Pattern::new(&pattern)?;
                globs.push(pattern);
            }
        }
    }

    Ok(globs)
}

struct SshVecReader {
    limit: usize,
    data: Vec<u8>,
}

#[async_trait::async_trait]
impl SshReader for SshVecReader {
    type Output = ();

    async fn read<'a>(
        &mut self,
        _client: &mut SshClient,
        input: &'a [u8],
    ) -> nom::IResult<&'a [u8], Self::Output> {
        if self.data.len() + input.len() > self.limit {
            Err(nom::Err::Error(nom::error::Error::new(
                input,
                nom::error::ErrorKind::TooLarge,
            )))
        } else {
            self.data.extend_from_slice(input);
            Ok((&[], ()))
        }
    }
}

/// Download every git lfs file to the source directory.
#[tracing::instrument(skip(client, source_directory))]
pub async fn download(
    client: &mut SshClient,
    repository: &models::GitRepositoryParts,
    source_directory: impl AsRef<Path>,
) -> Result<()> {
    let (sender, mut receiver) = mpsc::channel(32);

    let source_directory = fs::canonicalize(source_directory).await?;
    let list_files = task::spawn(list_files(source_directory, sender));

    let command = format!("git-lfs-authenticate '{}' download", repository.path);
    client.exec(&command).await?;

    // Read the response of the command into a buffer.
    let mut reader = SshVecReader {
        limit: 16_384,
        data: Vec::with_capacity(1024),
    };

    while let Some(()) = client.read(&mut reader).await? {}

    let auth: GitLfsAuthenticate = serde_json::from_slice(&reader.data)?;

    // If href is not set we have to compute a default.
    // https://github.com/git-lfs/git-lfs/blob/main/docs/api/server-discovery.md
    let auth_href = auth.href.unwrap_or_else(|| {
        tracing::trace!("guessing git lfs endpoint");
        format!("https://{}/{}/info/lfs", repository.host, repository.path)
    });

    tracing::trace!("using git lfs endpoint {auth_href}");

    let mut http_client = reqwest::Client::builder().build()?;

    let mut file_buffer = Vec::with_capacity(32);

    while let Some(Some(lfs_file)) = receiver.recv().await {
        file_buffer.push(lfs_file);

        if file_buffer.len() == file_buffer.capacity() {
            fetch(&mut http_client, &auth_href, &auth.header, &file_buffer).await?;
            file_buffer.clear();
        }
    }

    receiver.close();

    list_files.await??;

    if !file_buffer.is_empty() {
        fetch(&mut http_client, &auth_href, &auth.header, &file_buffer).await?;
    }

    Ok(())
}

#[tracing::instrument(skip(client, auth_headers, files))]
async fn fetch(
    client: &mut reqwest::Client,
    auth_href: &str,
    auth_headers: &BTreeMap<String, String>,
    files: &[PathBuf],
) -> Result<()> {
    let mut file_map: BTreeMap<(String, u64), Vec<PathBuf>> = BTreeMap::new();

    // Parse the lfs files and find their oid's and sizes.
    // https://github.com/git-lfs/git-lfs/blob/main/docs/spec.md
    for path in files {
        let file = fs::OpenOptions::new().read(true).open(path).await?;

        // Make sure the file is not unexpectedly large.
        let metadata = file.metadata().await?;
        ensure!(metadata.len() < 1024);

        let mut oid: Option<String> = None;
        let mut size: Option<u64> = None;

        let reader = BufReader::new(file);
        let mut lines = reader.lines();
        while let Some(line) = lines.next_line().await? {
            if oid.is_none() {
                // Grab the first line with a sha256 oid.
                oid = line.strip_prefix("oid sha256:").map(str::to_owned);
            }

            if size.is_none() {
                // Grab the first line with a file size.
                size = line
                    .strip_prefix("size ")
                    .and_then(|size| size.parse().ok());
            }
        }

        if let (Some(oid), Some(size)) = (oid, size) {
            file_map.entry((oid, size)).or_default().push(path.clone());
        }
    }

    let objects = file_map
        .keys()
        .map(|(oid, size)| GitLfsBatchRequestObject { oid, size: *size })
        .collect::<Vec<_>>();

    let batch_request = GitLfsBatchRequest {
        operation: GitLfsBatchRequestOperation::Download,
        objects: &objects,
    };

    let body = serde_json::to_vec(&batch_request)?;

    let mut request = client
        .post(format!("{auth_href}/objects/batch"))
        .header("accept", "application/vnd.git-lfs+json")
        .header("content-type", "application/vnd.git-lfs+json")
        .body(body);

    for (key, value) in auth_headers.iter() {
        request = request.header(key, value);
    }

    let response = request.send().await?;

    if !response.status().is_success() {
        let response_status = response.status();
        // TODO: limit the response body.
        let response_body = response.text().await?;
        return Err(anyhow!(
            "git lfs returned {}: {}",
            response_status,
            response_body
        ));
    }

    // TODO: limit the response body.
    let response_body: GitLfsBatchResponse = response.json().await?;

    // Combine each object with its path.
    let objects = response_body.objects.into_iter().flat_map(|object| {
        file_map
            .get(&(object.oid.clone(), object.size))
            .map(|file_paths| (object, file_paths))
    });

    // Download each file.
    for (object, file_paths) in objects {
        let mut request = client.get(object.actions.download.href);

        let headers = if object.authenticated {
            object.actions.download.header.iter()
        } else {
            auth_headers.iter()
        };

        for (key, value) in headers {
            request = request.header(key, value);
        }

        let mut response = request.send().await?;

        let mut writers = Vec::with_capacity(file_paths.len());

        for file_path in file_paths {
            let file = fs::OpenOptions::new()
                .write(true)
                .truncate(true)
                .open(file_path)
                .await?;

            let writer = BufWriter::new(file);

            writers.push(writer);
        }

        // TODO: limit the response body.
        while let Some(chunk) = response.chunk().await? {
            for writer in writers.iter_mut() {
                writer.write_all(&chunk).await?;
            }
        }

        for writer in writers.iter_mut() {
            writer.flush().await?;
        }
    }

    Ok(())
}

/// Looks for .gitattributes-files and sends back any matching git lfs files.
#[tracing::instrument(skip_all)]
async fn list_files(
    source_directory: PathBuf,
    sender: mpsc::Sender<Option<PathBuf>>,
) -> Result<()> {
    let mut paths = vec![source_directory];

    while let Some(path) = paths.pop() {
        let mut directory = fs::read_dir(&path).await?;

        while let Some(file) = directory.next_entry().await? {
            let file_type = file.file_type().await?;

            if file_type.is_dir() {
                paths.push(file.path());
            } else if file_type.is_file() && file.file_name() == ".gitattributes" {
                let lfs_globs = parse_attributes(file.path()).await?;

                if !lfs_globs.is_empty() {
                    find_files_matching_pattern(&path, &lfs_globs, &sender).await?;
                }
            }
        }
    }

    // Wait until the receiver is closed.
    while let Ok(()) = sender.send(None).await {}

    Ok(())
}

/// Find all files matching any of the patterns in the path.
#[tracing::instrument(skip_all)]
async fn find_files_matching_pattern(
    path: impl AsRef<Path>,
    patterns: &[glob::Pattern],
    sender: &mpsc::Sender<Option<PathBuf>>,
) -> Result<()> {
    let root_path = path.as_ref();
    let mut paths = vec![root_path.to_owned()];

    while let Some(path) = paths.pop() {
        let mut directory = fs::read_dir(&path).await?;

        while let Some(file) = directory.next_entry().await? {
            let file_type = file.file_type().await?;

            if file_type.is_dir() {
                paths.push(file.path());
            } else if file_type.is_file() {
                let file_path = file.path();
                let relative_path = file_path.strip_prefix(&root_path)?;

                if patterns
                    .iter()
                    .any(|pattern| pattern.matches_path(relative_path))
                {
                    sender.send(Some(file_path)).await?;
                }
            }
        }
    }

    Ok(())
}
