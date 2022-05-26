use anyhow::Result;

use fairing_core::{
    backends::{Database, FileMetadata, FileStorage},
    models,
};

pub async fn handle(
    req: hyper::Request<hyper::Body>,
    database: Database,
    file_metadata: FileMetadata,
    file_storage: FileStorage,
    authority: Option<http::uri::Authority>,
) -> Result<hyper::Response<hyper::Body>> {
    let res = handle_inner(req, database, file_metadata, file_storage, authority).await;
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
    authority: Option<http::uri::Authority>,
) -> Result<hyper::Response<hyper::Body>> {
    let deployment_lookup = authority
        .as_ref()
        .map(|authority| authority.host())
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

    let deployment = database.get_deployment_by_host(&deployment_lookup).await?;
    let (projections, modules) = match deployment {
        Some((mut projections, modules)) => {
            projections.sort_by(|a, b| b.mount_path.cmp(&a.mount_path));
            (projections, modules)
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
    let path = percent_encoding::percent_decode_str(path).decode_utf8()?;

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

    let file = find_layer_member_file(&file_metadata, &projection, &path).await?;
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

    let mut config = wasmtime::Config::new();
    config.async_support(true);

    let engine = wasmtime::Engine::new(&config)?;

    let mut store = wasmtime::Store::new(&engine, res);

    for module in modules.iter() {
        let blob_checksums = file_metadata.get_file_chunks(&module.file_id).await?;
        let mut module_data = Vec::new();

        for blob_checksum in blob_checksums {
            let blob = file_storage
                .load_blob(&models::BlobChecksum(blob_checksum))
                .await?;
            let reader = std::io::Cursor::new(blob);
            // TODO: check if the blob is compressed.
            let blob = zstd::decode_all(reader)?;
            module_data.extend_from_slice(&blob);
        }

        let module = wasmtime::Module::new(&engine, module_data)?;

        let mut linker = wasmtime::Linker::new(&engine);

        linker.func_wrap(
            "fairing_v1alpha1",
            "response_set_status_code",
            |mut caller: wasmtime::Caller<'_, hyper::Response<hyper::Body>>, status_code: u32| {
                let res = caller.data_mut();

                let status_code: u16 = match status_code.try_into() {
                    Ok(status_code) => status_code,
                    Err(_) => return 1,
                };

                let status_code = match hyper::StatusCode::from_u16(status_code) {
                    Ok(status_code) => status_code,
                    Err(_) => return 1,
                };

                *res.status_mut() = status_code;

                0
            },
        )?;

        linker.func_wrap(
            "fairing_v1alpha1",
            "response_append_header",
            |mut caller: wasmtime::Caller<'_, hyper::Response<hyper::Body>>,
             name_ptr: u32,
             name_len: u32,
             value_ptr: u32,
             value_len: u32| {
                let memory = match caller.get_export("mem") {
                    Some(wasmtime::Extern::Memory(memory)) => memory,
                    _ => return 1,
                };

                let memory = memory.data(&caller);

                if name_len > 8_192 || value_len > 8_192 {
                    return 2;
                }

                let name_end = match name_ptr.checked_add(name_len) {
                    Some(name_end) => name_end as usize,
                    None => return 2,
                };

                let value_end = match value_ptr.checked_add(value_len) {
                    Some(value_end) => value_end as usize,
                    None => return 2,
                };

                if name_end >= memory.len() || value_end >= memory.len() {
                    return 2;
                }

                let name = &memory[name_ptr as usize..name_end];
                let name = match hyper::header::HeaderName::from_bytes(name) {
                    Ok(name) => name,
                    Err(_) => return 3,
                };

                let value = &memory[value_ptr as usize..value_end];
                let value = match hyper::header::HeaderValue::from_bytes(value) {
                    Ok(value) => value,
                    Err(_) => return 3,
                };

                let res = caller.data_mut();

                res.headers_mut().append(name, value);

                0
            },
        )?;

        let instance = linker.instantiate_async(&mut store, &module).await?;

        let fairing_request =
            instance.get_typed_func::<(), (), _>(&mut store, "fairing_request")?;

        fairing_request.call_async(&mut store, ()).await?;
    }

    let res = store.into_data();

    Ok(res)
}

async fn find_layer_member_file(
    file_metadata: &FileMetadata,
    projection: &models::DeploymentProjectionAsdf,
    path: &str,
) -> Result<Option<(models::File, &'static str)>> {
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
