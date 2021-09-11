use fairing_core::{
    backends::Database,
    models::{self, prelude::*},
};
use fairing_proto::{
    sites::v1beta1::sites_server::SitesServer, teams::v1beta1::teams_server::TeamsServer,
    users::v1beta1::users_server::UsersServer,
};
use std::net::SocketAddr;
use tonic::{
    transport::{Error, Server},
    Request, Status,
};

mod sites;
mod teams;
mod users;

pub async fn api_server(database: Database, addr: SocketAddr) -> Result<(), Error> {
    let web_config = tonic_web::config();
    let auth = AuthInterceptor::new(&database);

    let users_server = UsersServer::new(users::UsersService::new(&database));

    let teams_server =
        TeamsServer::with_interceptor(teams::TeamsService::new(&database), auth.interceptor());

    let sites_server =
        SitesServer::with_interceptor(sites::SitesService::new(&database), auth.interceptor());

    Server::builder()
        .accept_http1(true)
        .add_service(web_config.enable(users_server))
        .add_service(web_config.enable(teams_server))
        .add_service(web_config.enable(sites_server))
        .serve(addr)
        .await
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

/// Authentication interceptor (middleware) for tonic that adds the current user's name to the
/// request's metadata. If the user didn't authenticate themselves an error is returned.
struct AuthInterceptor {
    database: Database,
}

impl AuthInterceptor {
    fn new(database: &Database) -> AuthInterceptor {
        AuthInterceptor {
            database: database.clone(),
        }
    }

    fn interceptor(&self) -> tonic::Interceptor {
        let database = self.database.clone();

        tonic::Interceptor::new(move |req: Request<()>| -> Result<Request<()>, Status> { Ok(req) })
    }
}
