use anyhow::{anyhow, Error, Result};
use fairing_core::{
    backends::{Database, FileMetadata, FileStorage},
    models::{self, prelude::*},
};
use fairing_proto::{
    sites::v1beta1::sites_server::SitesServer, sources::v1beta1::sources_server::SourcesServer,
    teams::v1beta1::teams_server::TeamsServer, users::v1beta1::users_server::UsersServer,
};
use futures::future::{self, Either, TryFutureExt};
use hyper::{server::conn::AddrStream, service::make_service_fn, Server};
use std::{
    convert::Infallible,
    net::SocketAddr,
    pin::Pin,
    task::{Context, Poll},
};
use tonic::{transport::Server as TonicServer, Request, Status};
use tower::Service;

mod sites;
mod sources;
mod teams;
mod users;
mod web;

pub async fn serve(
    database: Database,
    file_metadata: FileMetadata,
    file_storage: FileStorage,
    addr: SocketAddr,
) -> Result<()> {
    let web_config = tonic_web::config();
    let auth = AuthInterceptor::new(&database);

    let users_server = UsersServer::new(users::UsersService::new(&database));

    let teams_server = TeamsServer::with_interceptor(
        teams::TeamsService::new(&database, &file_metadata),
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
        .add_service(web_config.enable(users_server))
        .add_service(web_config.enable(teams_server))
        .add_service(web_config.enable(sites_server))
        .add_service(web_config.enable(sources_server))
        .into_service();

    Server::bind(&addr)
        .serve(make_service_fn(move |s: &AddrStream| {
            let remote_addr = s.remote_addr();

            let mut tonic = tonic.clone();
            let database = database.clone();
            let file_metadata = file_metadata.clone();
            let file_storage = file_storage.clone();

            future::ok::<_, Infallible>(tower::service_fn(
                move |req: hyper::Request<hyper::Body>| {
                    let host = req.uri().host().or_else(|| {
                        req.headers()
                            .get(http::header::HOST)
                            .and_then(|host| host.to_str().ok())
                            .and_then(|host| host.split(':').next())
                    });

                    let path = req.uri().path_and_query().map(|p| p.as_str());

                    tracing::info!(
                        remote_addr = %remote_addr,
                        version = ?req.version(),
                        method = %req.method(),
                        path = %path.unwrap_or("None"),
                        host = %host.unwrap_or("None"),
                    );

                    match (req.version(), host) {
                        (http::Version::HTTP_2, Some(host)) if host == "api.localhost" => {
                            Either::Left(
                                tonic
                                    .call(req)
                                    .map_ok(|res| res.map(EitherBody::Left))
                                    .map_err(|err| anyhow!("tonic error: {:?}", err)),
                            )
                        }
                        _ => Either::Right(
                            web::handle(
                                req,
                                database.clone(),
                                file_metadata.clone(),
                                file_storage.clone(),
                            )
                            .map_ok(|res| res.map(EitherBody::Right)),
                        ),
                    }
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
