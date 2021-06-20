use tonic::{Request, Response, Status};

use fairing_core::{backends::Database, models::prelude::*};
use fairing_proto::users::v1beta1::{users_server::Users, CreateUserRequest, User};

#[derive(Debug)]
pub struct UsersService {
    database: Database,
}

impl UsersService {
    pub fn new(database: &Database) -> UsersService {
        UsersService {
            database: database.clone(),
        }
    }
}

#[tonic::async_trait]
impl Users for UsersService {
    #[tracing::instrument]
    async fn create_user(
        &self,
        request: Request<CreateUserRequest>,
    ) -> Result<Response<User>, Status> {
        let user = fairing_core::models::CreateUser {
            resource_id: &request.get_ref().resource_id,
            password: &request.get_ref().password,
        };

        let user = self.database.create_user(&user).await.map_err(|err| {
            tracing::error!("error: {:?}", err);
            Status::internal("error when creating user")
        })?;

        let reply = User {
            name: user.name.name().into(),
        };
        Ok(Response::new(reply))
    }
}
