use anyhow::{anyhow, Result};
use http::{header, status::StatusCode, HeaderMap, Request, Response};
use http_body::Body;
use std::{collections::VecDeque, future::Future, io::Cursor, net::SocketAddr, pin::Pin, task};

use crate::{
    models,
    repositories::{DomainRepository, FileRepository, LayerRepository},
};

#[derive(Copy, Clone)]
pub struct ConnectionMeta {
    remote_addr: SocketAddr,
    sni_hostname_hash: Option<[u8; 16]>,
}

impl ConnectionMeta {
    pub fn new(remote_addr: SocketAddr, sni_hostname: Option<&str>) -> ConnectionMeta {
        let sni_hostname_hash = sni_hostname.map(ConnectionMeta::hash_hostname);

        ConnectionMeta {
            remote_addr,
            sni_hostname_hash,
        }
    }

    pub fn matches_sni_hostname(&self, hostname: &str) -> bool {
        if let Some(sni_hostname_hash) = self.sni_hostname_hash {
            let hostname_hash = ConnectionMeta::hash_hostname(hostname);
            sni_hostname_hash == hostname_hash
        } else {
            true
        }
    }

    fn hash_hostname(hostname: &str) -> [u8; 16] {
        use blake2::{digest::consts::U16, Blake2b, Digest};

        let mut hasher = Blake2b::<U16>::new();
        hasher.update(hostname);

        let hash = hasher.finalize();

        let mut hostname_hash = [0u8; 16];
        hostname_hash.copy_from_slice(&hash);
        hostname_hash
    }
}

#[derive(Copy, Clone)]
pub struct HttpService {
    layer_repository: &'static dyn LayerRepository,
    file_repository: &'static dyn FileRepository,
    domain_repository: &'static dyn DomainRepository,
}

impl HttpService {
    pub fn new(
        layer_repository: &'static dyn LayerRepository,
        file_repository: &'static dyn FileRepository,
        domain_repository: &'static dyn DomainRepository,
    ) -> HttpService {
        HttpService {
            layer_repository,
            file_repository,
            domain_repository,
        }
    }

    pub async fn get_certificate(&self, fqdn: &str) -> Result<Option<()>> {
        todo!();
    }

    pub fn handle_connection(&self, connection_meta: ConnectionMeta) -> HttpConnection {
        HttpConnection::new(
            self.layer_repository,
            self.file_repository,
            self.domain_repository,
            connection_meta,
        )
    }
}

#[derive(Copy, Clone)]
pub struct HttpConnection {
    layer_repository: &'static dyn LayerRepository,
    file_repository: &'static dyn FileRepository,
    domain_repository: &'static dyn DomainRepository,
    connection_meta: ConnectionMeta,
}

impl HttpConnection {
    pub fn new(
        layer_repository: &'static dyn LayerRepository,
        file_repository: &'static dyn FileRepository,
        domain_repository: &'static dyn DomainRepository,
        connection_meta: ConnectionMeta,
    ) -> HttpConnection {
        HttpConnection {
            layer_repository,
            file_repository,
            domain_repository,
            connection_meta,
        }
    }

    pub async fn handle_request<B: Body>(self, request: Request<B>) -> Result<Response<HttpBody>> {
        let result = self.handle_inner(request).await;
        match result {
            Ok(response) => Ok(response),
            Err(err) => {
                tracing::error!("{:?}", err);

                let builder = self
                    .default_response(StatusCode::INTERNAL_SERVER_ERROR)
                    .header(header::CONTENT_TYPE, "text/plain");

                let body = HttpBody::Static {
                    data: Some(b"500 Internal error".to_vec()),
                };
                Ok(builder.body(body)?)
            }
        }
    }

    async fn handle_inner<B: Body>(&self, request: Request<B>) -> Result<Response<HttpBody>> {
        let host = request
            .headers()
            .get(header::HOST)
            .and_then(|host| host.to_str().ok())
            .and_then(|host| host.split(':').next());

        let host = match host {
            Some(host) if self.connection_meta.matches_sni_hostname(host) => host,
            _ => {
                let builder = self
                    .default_response(StatusCode::BAD_REQUEST)
                    .header(header::CONTENT_TYPE, "text/plain");

                let body = HttpBody::Static {
                    data: Some(b"400 Bad request".to_vec()),
                };
                return Ok(builder.body(body)?);
            }
        };

        let validate_domain = self.domain_repository.get_validated_domain(host).await?;

        let (project_id, layer_set_name, layer_id) = match validate_domain {
            /*
            Some(models::ValidatedDomain {
                project_id,
                kind:
                    models::ValidatedDomainKind::Layer {
                        layer_set_name,
                        layer_id,
                    },
                ..
            }) => (project_id, layer_set_name, layer_id),
            */
            _ => {
                let builder = self
                    .default_response(StatusCode::NOT_FOUND)
                    .header(header::CONTENT_TYPE, "text/plain");

                let body = HttpBody::Static {
                    data: Some(b"404 Not found".to_vec()),
                };
                return Ok(builder.body(body)?);
            }
        };

        let layer_members = self
            .layer_repository
            .get_layer_member_summary(
                project_id,
                &layer_set_name,
                layer_id,
                &[request.uri().path()],
            )
            .await?;

        if let Some(layer_member) = layer_members.first() {
            let mut builder = self.default_response(StatusCode::OK);

            for (key, value) in layer_member.headers.iter() {
                builder = builder.header(key, value);
            }

            let file_chunks = self
                .file_repository
                .get_file_chunks(project_id, layer_member.checksum, (0, 1))
                .await?;

            let total_length = file_chunks
                .first()
                .map(|file_chunk| file_chunk.total_length)
                .unwrap_or(0);

            let mut buffer = VecDeque::with_capacity(1);
            for file_chunk in file_chunks {
                buffer.push_back(file_chunk);
            }

            let body = HttpBody::File {
                repository: self.file_repository,
                project_id,
                checksum: layer_member.checksum,
                total_length,
                total_length_sent: 0,
                chunks_future: None,
                chunks_future_completed: false,
                buffer,
            };

            Ok(builder.body(body)?)
        } else {
            let builder = self
                .default_response(StatusCode::NOT_FOUND)
                .header(header::CONTENT_TYPE, "text/plain");

            let body = HttpBody::Static {
                data: Some(b"404 Not found".to_vec()),
            };
            Ok(builder.body(body)?)
        }
    }

    fn default_response(&self, status_code: StatusCode) -> http::response::Builder {
        Response::builder()
            .status(status_code)
            .header(header::CONTENT_SECURITY_POLICY, "frame-ancestors 'self'")
            .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
            .header(header::X_FRAME_OPTIONS, "SAMEORIGIN")
    }
}

#[pin_project::pin_project(project = HttpBodyProj)]
pub enum HttpBody {
    Static {
        data: Option<Vec<u8>>,
    },
    File {
        repository: &'static dyn FileRepository,
        project_id: models::ProjectId,
        checksum: models::FileChecksum,
        total_length: u64,
        total_length_sent: u64,
        #[pin]
        chunks_future: Option<Pin<Box<dyn Future<Output = Result<Vec<models::FileChunk>>> + Send>>>,
        chunks_future_completed: bool,
        buffer: VecDeque<models::FileChunk>,
    },
}

impl Body for HttpBody {
    type Data = Cursor<Vec<u8>>;
    type Error = anyhow::Error;

    fn poll_data(
        self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
    ) -> task::Poll<Option<Result<Self::Data, Self::Error>>> {
        let this = self.project();

        match this {
            HttpBodyProj::Static { data } => {
                let data = data.take().map(|data| Ok(Cursor::new(data)));
                task::Poll::Ready(data)
            }
            HttpBodyProj::File {
                repository,
                project_id,
                checksum,
                total_length,
                total_length_sent,
                mut chunks_future,
                chunks_future_completed,
                buffer,
            } => {
                if total_length == total_length_sent {
                    return task::Poll::Ready(None);
                }

                if let Some(chunk) = buffer.pop_front() {
                    *total_length_sent += chunk.data.len() as u64;
                    return task::Poll::Ready(Some(Ok(Cursor::new(chunk.data))));
                }

                if chunks_future.is_none() || *chunks_future_completed {
                    let future = repository.get_file_chunks(
                        *project_id,
                        *checksum,
                        (*total_length_sent, *total_length_sent + (4 << 20)),
                    );

                    *chunks_future = Some(future);
                    *chunks_future_completed = false;
                }

                let future = chunks_future.as_pin_mut().unwrap();
                match future.poll(cx) {
                    task::Poll::Ready(Ok(mut chunks)) => {
                        let mut chunks = chunks.drain(..);
                        let first_chunk = chunks.next();

                        for chunk in chunks {
                            buffer.push_back(chunk);
                        }

                        *chunks_future_completed = true;
                        if let Some(chunk) = first_chunk {
                            *total_length_sent += chunk.data.len() as u64;
                            task::Poll::Ready(Some(Ok(Cursor::new(chunk.data))))
                        } else {
                            task::Poll::Ready(Some(Err(anyhow!("expected more chunks"))))
                        }
                    }
                    task::Poll::Ready(Err(err)) => {
                        // The task shouldn't be called again after an error.
                        task::Poll::Ready(Some(Err(err)))
                    }
                    task::Poll::Pending => task::Poll::Pending,
                }
            }
        }
    }

    fn poll_trailers(
        self: Pin<&mut Self>,
        _cx: &mut task::Context<'_>,
    ) -> task::Poll<Result<Option<HeaderMap>, Self::Error>> {
        task::Poll::Ready(Ok(None))
    }

    fn size_hint(&self) -> http_body::SizeHint {
        match self {
            HttpBody::Static { data: Some(data) } => {
                http_body::SizeHint::with_exact(data.len() as u64)
            }
            HttpBody::Static { data: None } => http_body::SizeHint::with_exact(0),
            HttpBody::File { total_length, .. } => http_body::SizeHint::with_exact(*total_length),
        }
    }
}
