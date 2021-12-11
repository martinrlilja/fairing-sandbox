use anyhow::Result;

use fairing_core::{
    backends::{Database, FileMetadata, FileStorage},
    models::{self, prelude::*},
};

pub async fn handle(
    req: hyper::Request<hyper::Body>,
    database: Database,
    file_metadata: FileMetadata,
    file_storage: FileStorage,
) -> Result<hyper::Response<hyper::Body>> {
    let res = handle_inner(req, database, file_metadata, file_storage).await;
    match res {
        Ok(res) => Ok(res),
        Err(err) => {
            tracing::error!("{:?}", err);
            let res = hyper::Response::builder()
                .status(http::status::StatusCode::INTERNAL_SERVER_ERROR)
                .header(http::header::CONTENT_TYPE, "text/plain; charset=utf8")
                .body(hyper::Body::from("500 Internal server error"))?;
            Ok(res)
        }
    }
}

async fn handle_inner(
    req: hyper::Request<hyper::Body>,
    database: Database,
    file_metadata: FileMetadata,
    file_storage: FileStorage,
) -> Result<hyper::Response<hyper::Body>> {
    let deployment_lookup = req
        .uri()
        .host()
        .or_else(|| {
            req.headers()
                .get(http::header::HOST)
                .and_then(|host| host.to_str().ok())
        })
        .and_then(models::DeploymentHostLookup::parse);

    let deployment_lookup = match deployment_lookup {
        Some(deployment_lookup) => deployment_lookup,
        None => {
            let res = hyper::Response::builder()
                .status(http::status::StatusCode::BAD_REQUEST)
                .header(http::header::CONTENT_TYPE, "text/plain; charset=utf8")
                .body(hyper::Body::from("400 Bad request"))?;
            return Ok(res);
        }
    };

    let projections = database.get_deployment_by_host(&deployment_lookup).await?;
    let projections = match projections {
        Some(mut projections) => {
            projections.sort_by(|a, b| b.mount_path.cmp(&a.mount_path));
            projections
        }
        None => {
            let res = hyper::Response::builder()
                .status(http::status::StatusCode::NOT_FOUND)
                .header(http::header::CONTENT_TYPE, "text/plain; charset=utf8")
                .body(hyper::Body::from("404 Site not found"))?;
            return Ok(res);
        }
    };

    let path = req.uri().path();

    let projection = projections
        .into_iter()
        .find(|p| path.starts_with(&p.mount_path));

    let projection = match projection {
        Some(projection) => projection,
        None => {
            let res = hyper::Response::builder()
                .status(http::status::StatusCode::NOT_FOUND)
                .header(http::header::CONTENT_TYPE, "text/plain; charset=utf8")
                .body(hyper::Body::from("404 Not found"))?;
            return Ok(res);
        }
    };

    let file = find_layer_member_file(&file_metadata, &projection, path).await?;
    let (file, mime_type) = match file {
        Some(file) => file,
        None => {
            let res = hyper::Response::builder()
                .status(http::status::StatusCode::NOT_FOUND)
                .header(http::header::CONTENT_TYPE, "text/plain; charset=utf8")
                .body(hyper::Body::from("404 Not found"))?;
            return Ok(res);
        }
    };

    let blob_checksums = file_metadata.get_file_chunks(&file.id).await?;

    let mut body = Vec::with_capacity(file.size as usize);

    // TODO: stream blobs.
    for blob_checksum in blob_checksums {
        let blob = file_storage
            .load_blob(&models::BlobChecksum(blob_checksum))
            .await?;
        let reader = std::io::Cursor::new(blob);
        // TODO: check if the blob is compressed.
        let blob = zstd::decode_all(reader)?;
        body.extend_from_slice(&blob);
    }

    let res = hyper::Response::builder()
        .status(http::status::StatusCode::OK)
        .header(http::header::CONTENT_TYPE, mime_type)
        .body(hyper::Body::from(body))?;

    Ok(res)
}

async fn find_layer_member_file(file_metadata: &FileMetadata, projection: &models::DeploymentProjectionAsdf, path: &str) -> Result<Option<(models::File, &'static str)>> {
    let exts = if path.ends_with('/') {
        &["index.html", "index.htm"][..]
    } else {
        &["", "/index.html", "/index.htm"][..]
    };

    for ext in exts {
        let file = if ext.is_empty() {
            file_metadata
                .get_layer_member_file(projection.layer_set_id, projection.layer_id, path)
                .await?
                .map(|file| {
                    let mime = mime_type(path, &file);
                    (file, mime)
                })
        } else {
            let mut p = String::with_capacity(path.len() + ext.len());
            p.push_str(path);
            p.push_str(ext);

            file_metadata
                .get_layer_member_file(projection.layer_set_id, projection.layer_id, &p)
                .await?
                .map(|file| {
                    let mime = mime_type(&p, &file);
                    (file, mime)
                })
        };

        if let Some(file) = file {
            return Ok(Some(file));
        }
    }

    Ok(None)
}

fn mime_type(path: &str, file: &models::File) -> &'static str {
    let ext = match path.rsplit_once('.') {
        Some((_path, ext)) => ext,
        None => "application/octet-stream",
    };

    match ext {
        // application/*
        "eot" => "application/vnd.ms-fontobject",
        "gz" => "application/gzip",
        "json" => "application/json",
        "jsonld" => "application/ld+json",
        "ogg" => "application/ogg",
        "pdf" => "application/pdf",
        "wasm" => "application/wasm",
        "xml" => "application/xml",
        "zip" => "application/zip",
        "zstd" => "application/zstd",
        // audio/*
        "aac" => "audio/aac",
        "mp3" => "audio/mp3",
        "oga" => "audio/ogg",
        "opus" => "audio/opus",
        "weba" => "image/webm",
        // font/*
        "otf" => "font/otf",
        "ttf" => "font/ttf",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        // image/*
        "avif" => "image/avif",
        "bmp" => "image/bmp",
        "gif" => "image/gif",
        "heif" => "image/heif",
        "ico" => "image/vnd.microsoft.icon",
        "jpeg" | "jpg" => "image/jpeg",
        "png" => "image/png",
        "svg" => "image/svg+xml",
        "webp" => "image/webp",
        // text/* with charset
        "css" if file.is_valid_utf8 => "text/css; charset=utf-8",
        "csv" if file.is_valid_utf8 => "text/csv; charset=utf-8",
        "html" | "htm" if file.is_valid_utf8 => "text/html; charset=utf-8",
        "ics" if file.is_valid_utf8 => "text/calendar; charset=utf-8",
        "js" | "mjs" if file.is_valid_utf8 => "text/javascript; charset=utf-8",
        "markdown" if file.is_valid_utf8 => "text/markdown; charset=utf-8",
        "txt" if file.is_valid_utf8 => "text/plain; charset=utf-8",
        // text/*
        "css" => "text/css",
        "csv" => "text/csv",
        "html" | "htm" => "text/html",
        "ics" => "text/calendar",
        "js" | "mjs" => "text/javascript",
        "markdown" => "text/markdown",
        "txt" => "text/plain",
        // video/*
        "mp4" => "video/mp4",
        "ogv" => "video/ogg",
        "webm" => "video/webm",
        _ => "application/octet-stream",
    }
}
