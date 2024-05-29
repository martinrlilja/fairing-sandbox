use anyhow::{anyhow, Error, Result};
use fairing_core::{
    backends::{Database, FileMetadata, FileStorage},
    models::{self, prelude::*},
};
use fairing_proto::{
    domains::v1beta1::domains_server::DomainsServer, sites::v1beta1::sites_server::SitesServer,
    sources::v1beta1::sources_server::SourcesServer, teams::v1beta1::teams_server::TeamsServer,
    users::v1beta1::users_server::UsersServer,
};
use futures::future::{self, Either, TryFutureExt};
use hyper::{service::make_service_fn, Server};
use std::{
    convert::Infallible,
    net::SocketAddr,
    pin::Pin,
    task::{Context, Poll},
};
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::server::TlsStream;
use tonic::{transport::Server as TonicServer, Request, Status};
use tower::Service;

mod certificate_resolver;
mod domains;
mod sites;
mod sources;
mod teams;
mod users;
mod web;

pub async fn serve(
    http_service: fairing_core2::services::HttpService,
    http_addr: Vec<SocketAddr>,
    https_redirect: bool,
    https_redirect_port: Option<u16>,
    https_addr: Vec<SocketAddr>,
    api_host: String,
) -> Result<()> {
    //let certificate_resolver = certificate_resolver::CertificateResolver::new(database.clone());

    let mut task_set = Vec::<tokio::task::JoinHandle<Result<()>>>::new();

    // Leak the api host so that we don't have to clone it everywhere.
    let api_host: &'static str = Box::leak(api_host.into_boxed_str());

    for http_addr in http_addr {
        let http_listener = TcpListener::bind(&http_addr).await?;
        let http_acceptor = hyper::server::conn::AddrIncoming::from_listener(http_listener)?;

        if https_redirect {
            task_set.push(tokio::spawn(async move {
                server_https_redirect(https_redirect_port, http_acceptor).await
            }));
        } else {
            task_set.push(tokio::spawn(async move {
                server(http_service, http_acceptor, api_host).await
            }));
        }

        tracing::info!("http listening on {http_addr}");
    }

    /*
    for https_addr in https_addr {
        let https_listener = TcpListener::bind(&https_addr).await?;
        let incoming_tls_stream =
            certificate_resolver::accept(https_listener, certificate_resolver.clone());
        let https_acceptor = hyper::server::accept::from_stream(incoming_tls_stream);

        task_set.push(tokio::spawn(async move {
            server(http_service, https_acceptor, api_host).await
        }));

        tracing::info!("https listening on {https_addr}");
    }
    */

    for task in task_set {
        task.await??;
    }

    Ok(())
}

async fn server<Accept>(
    http_service: fairing_core2::services::HttpService,
    acceptor: Accept,
    api_host: &'static str,
) -> Result<()>
where
    Accept: hyper::server::accept::Accept,
    Accept::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
    Accept::Conn:
        ConnectionInfo + tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    /*
    let web_config = tonic_web::config();
    let auth = AuthInterceptor::new(&database);

    let users_server = UsersServer::new(users::UsersService::new(&database));

    let teams_server = TeamsServer::with_interceptor(
        teams::TeamsService::new(&database, &file_metadata),
        auth.interceptor(),
    );

    let domains_server = DomainsServer::with_interceptor(
        domains::DomainsService::new(&database),
        auth.interceptor(),
    );

    let sites_server =
        SitesServer::with_interceptor(sites::SitesService::new(&database), auth.interceptor());

    let sources_server = SourcesServer::with_interceptor(
        sources::SourcesService::new(&database),
        auth.interceptor(),
    );

    let tonic = TonicServer::builder()
        .accept_http1(true)
        .add_service(web_config.enable(domains_server))
        .add_service(web_config.enable(users_server))
        .add_service(web_config.enable(teams_server))
        .add_service(web_config.enable(sites_server))
        .add_service(web_config.enable(sources_server))
        .into_service();
    */

    Server::builder(acceptor)
        .serve(make_service_fn(move |s: &Accept::Conn| {
            let connection = http_service.handle_connection(
                fairing_core2::services::ConnectionMeta::new(s.remote_addr(), s.sni_hostname()),
            );

            let remote_addr = s.remote_addr();
            let sni_hostname = s.sni_hostname().map(str::to_owned);

            future::ok::<_, Infallible>(tower::service_fn(
                move |req: hyper::Request<hyper::Body>| {
                    let sni_hostname = sni_hostname.clone();

                    let authority = req.uri().authority().cloned().or_else(|| {
                        req.headers().get(http::header::HOST).and_then(|host| {
                            let host = host.as_bytes().to_vec();
                            http::uri::Authority::from_maybe_shared(host).ok()
                        })
                    });

                    let host = authority.as_ref().map(|authority| authority.host());

                    match (sni_hostname, host) {
                        (Some(sni_hostname), Some(host))
                            if !sni_hostname.eq_ignore_ascii_case(host) =>
                        {
                            tracing::error!("sni hostname does not match request host");
                        }
                        _ => (),
                    }

                    let path = req.uri().path_and_query().map(|p| p.as_str());

                    tracing::info!(
                        remote_addr = %remote_addr,
                        version = ?req.version(),
                        method = %req.method(),
                        path = %path.unwrap_or("None"),
                        host = %host.unwrap_or("None"),
                    );

                    connection.handle_request(req)

                    /*
                    match (req.version(), host) {
                        (http::Version::HTTP_2, Some(host)) if host == api_host => Either::Left(
                            tonic
                                .call(req)
                                .map_ok(|res| res.map(EitherBody::Left))
                                .map_err(|err| anyhow!("tonic error: {:?}", err)),
                        ),
                        _ => Either::Right(
                            web::handle(
                                req,
                                database.clone(),
                                file_metadata.clone(),
                                file_storage.clone(),
                                authority,
                            )
                            .map_ok(|res| res.map(EitherBody::Right)),
                        ),
                    }
                    */
                },
            ))
        }))
        .await?;

    Ok(())
}

async fn server_https_redirect<Accept>(
    https_redirect_port: Option<u16>,
    acceptor: Accept,
) -> Result<()>
where
    Accept: hyper::server::accept::Accept,
    Accept::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
    Accept::Conn:
        ConnectionInfo + tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    Server::builder(acceptor)
        .serve(make_service_fn(move |_| {
            future::ok::<_, Infallible>(tower::service_fn(
                move |req: hyper::Request<hyper::Body>| {
                    let authority = req.headers().get(http::header::HOST).and_then(|host| {
                        let host = host.as_bytes().to_vec();
                        http::uri::Authority::from_maybe_shared(host).ok()
                    });

                    let res = if let Some(authority) = authority {
                        let host = authority.host();
                        let location = if let Some(https_redirect_port) = https_redirect_port {
                            format!("https://{host}:{https_redirect_port}")
                        } else {
                            format!("https://{host}")
                        };

                        hyper::Response::builder()
                            .status(hyper::StatusCode::PERMANENT_REDIRECT)
                            .header(hyper::header::LOCATION, location)
                            .body("".into())
                            .map_err(anyhow::Error::from)
                    } else {
                        hyper::Response::builder()
                            .status(hyper::StatusCode::BAD_REQUEST)
                            .body("Bad request, missing host.".into())
                            .map_err(anyhow::Error::from)
                    };

                    future::ready::<Result<hyper::Response<hyper::Body>>>(res)
                },
            ))
        }))
        .await?;

    Ok(())
}

async fn auth<T>(
    database: &Database,
    req: &Request<T>,
) -> Result<models::UserName<'static>, Status> {
    let token = match req.metadata().get("authorization") {
        Some(token) => token,
        None => return Err(Status::unauthenticated("invalid authorization token")),
    };

    let mut parts = token
        .to_str()
        .map_err(|_err| Status::invalid_argument("invalid authorization token"))?
        .splitn(2, ' ')
        .fuse();

    let token_type = parts.next();
    let token = parts.next();

    match (token_type, token) {
        (Some(token_type), Some(token)) if token_type.eq_ignore_ascii_case("Basic") => {
            let token = base64::decode_config(token, base64::URL_SAFE)
                .map_err(|_err| Status::invalid_argument("invalid authorization token"))?;

            let token = String::from_utf8(token)
                .map_err(|_err| Status::invalid_argument("invalid authorization token"))?;

            let mut token = token.splitn(2, ':').fuse();

            let user = token.next();
            let password = token.next();

            if let (Some(user), Some(password)) = (user, password) {
                let user =
                    models::resource_name::validators::UnicodeIdentifierValidator::normalize(&user);

                let user_name = models::UserName::parse(format!("users/{}", user))
                    .map_err(|_err| Status::invalid_argument("invalid authorization header"))?;

                let password = models::Password::new(password);

                database
                    .verify_user_password(&user_name, password)
                    .await
                    .map(|_| user_name)
                    .map_err(|_err| Status::invalid_argument("invalid authorization token"))
            } else {
                Err(Status::invalid_argument("invalid authorization token"))
            }
        }
        (Some(token_type), Some(_token)) if token_type.eq_ignore_ascii_case("Bearer") => {
            Err(Status::unimplemented("Bearer tokens are not supported"))
        }
        _ => Err(Status::invalid_argument("invalid authorization header")),
    }
}

struct AuthInterceptor {
    database: Database,
}

impl AuthInterceptor {
    fn new(database: &Database) -> AuthInterceptor {
        AuthInterceptor {
            database: database.clone(),
        }
    }

    fn interceptor(&self) -> fn(Request<()>) -> Result<Request<()>, Status> {
        let _database = self.database.clone();

        move |req: Request<()>| -> Result<Request<()>, Status> { Ok(req) }
    }
}

enum EitherBody<A, B> {
    Left(A),
    Right(B),
}

// From: https://github.com/hyperium/tonic/blob/master/examples/src/hyper_warp_multiplex/server.rs
impl<A, B> http_body::Body for EitherBody<A, B>
where
    A: http_body::Body + Send + Unpin,
    B: http_body::Body<Data = A::Data> + Send + Unpin,
    A::Error: Into<Error>,
    B::Error: Into<Error>,
{
    type Data = A::Data;
    type Error = anyhow::Error;

    fn is_end_stream(&self) -> bool {
        match self {
            EitherBody::Left(b) => b.is_end_stream(),
            EitherBody::Right(b) => b.is_end_stream(),
        }
    }

    fn poll_data(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Self::Data, Self::Error>>> {
        match self.get_mut() {
            EitherBody::Left(b) => Pin::new(b).poll_data(cx).map(map_option_err),
            EitherBody::Right(b) => Pin::new(b).poll_data(cx).map(map_option_err),
        }
    }

    fn poll_trailers(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<Option<http::HeaderMap>, Self::Error>> {
        match self.get_mut() {
            EitherBody::Left(b) => Pin::new(b).poll_trailers(cx).map_err(Into::into),
            EitherBody::Right(b) => Pin::new(b).poll_trailers(cx).map_err(Into::into),
        }
    }
}

fn map_option_err<T, U: Into<Error>>(err: Option<Result<T, U>>) -> Option<Result<T, Error>> {
    err.map(|e| e.map_err(Into::into))
}

trait ConnectionInfo {
    fn remote_addr(&self) -> SocketAddr;

    fn sni_hostname(&self) -> Option<&str>;
}

impl ConnectionInfo for TlsStream<TcpStream> {
    fn remote_addr(&self) -> SocketAddr {
        let (tcp_stream, _) = self.get_ref();
        tcp_stream.peer_addr().unwrap()
    }

    fn sni_hostname(&self) -> Option<&str> {
        let (_, connection) = self.get_ref();
        connection.sni_hostname()
    }
}

impl ConnectionInfo for hyper::server::conn::AddrStream {
    fn remote_addr(&self) -> SocketAddr {
        self.remote_addr()
    }

    fn sni_hostname(&self) -> Option<&str> {
        None
    }
}
