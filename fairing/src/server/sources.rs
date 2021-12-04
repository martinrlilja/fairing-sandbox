use tonic::{Request, Response, Status};

use fairing_core::{
    backends::Database,
    models::{self, prelude::*},
};
use fairing_proto::sources::v1beta1::{
    source, sources_server::Sources, CreateSourceRequest, ListSourcesRequest, ListSourcesResponse,
    RefreshSourceRequest, RefreshSourceResponse, Source,
};

#[derive(Debug)]
pub struct SourcesService {
    database: Database,
}

impl SourcesService {
    pub fn new(database: &Database) -> SourcesService {
        SourcesService {
            database: database.clone(),
        }
    }
}

#[tonic::async_trait]
impl Sources for SourcesService {
    #[tracing::instrument]
    async fn list_sources(
        &self,
        request: Request<ListSourcesRequest>,
    ) -> Result<Response<ListSourcesResponse>, Status> {
        let _user = super::auth(&self.database, &request).await?;

        let parent = models::TeamName::parse(&request.get_ref().parent)
            .map_err(|_err| Status::invalid_argument("parent is not a valid team name"))?;

        // FIXME: check if the user is a member of the team.

        let sources = self.database.list_sources(&parent).await.map_err(|err| {
            tracing::error!("error: {:?}", err);
            Status::internal("error when listing sources")
        })?;

        let resources = sources.into_iter().map(Into::into).collect();

        let reply = ListSourcesResponse { resources };

        Ok(Response::new(reply))
    }

    #[tracing::instrument]
    async fn create_source(
        &self,
        request: Request<CreateSourceRequest>,
    ) -> Result<Response<Source>, Status> {
        let _user = super::auth(&self.database, &request).await?;

        let create_source = request.into_inner();

        let parent = models::TeamName::parse(create_source.parent)
            .map_err(|_err| Status::invalid_argument("parent is not a valid team name"))?;

        // FIXME: check if the user is a member of the team.

        let kind = create_source
            .source
            .as_ref()
            .and_then(|source| source.kind.as_ref());

        let kind = match kind {
            Some(source::Kind::GitSource(ref git_source)) => models::CreateSourceKind::GitSource {
                repository_url: &git_source.repository_url,
            },
            None => return Err(Status::invalid_argument("missing kind on source")),
        };

        let source = fairing_core::models::CreateSource {
            resource_id: &create_source.resource_id,
            parent,
            kind,
        };

        let source = self.database.create_source(&source).await.map_err(|err| {
            tracing::error!("error: {:?}", err);
            Status::internal("error when creating source")
        })?;

        Ok(Response::new(source.into()))
    }

    #[tracing::instrument]
    async fn refresh_source(
        &self,
        request: Request<RefreshSourceRequest>,
    ) -> Result<Response<RefreshSourceResponse>, Status> {
        let _user = super::auth(&self.database, &request).await?;

        let source = request.into_inner();

        let source_name = models::SourceName::parse(source.name)
            .map_err(|_err| Status::invalid_argument("name is not a valid site source name"))?;

        // FIXME: check if the user is a member of the team.

        let source = self
            .database
            .get_source(&source_name)
            .await
            .map_err(|err| {
                tracing::error!("error: {:?}", err);
                Status::internal("error when getting site source")
            })?
            .ok_or_else(|| Status::not_found("site source not found"))?;

        let remote_source = crate::backends::GenericRemoteSource::new();

        let builds = tokio::time::timeout(
            std::time::Duration::from_secs(60),
            remote_source.list_tree_revisions(&source),
        )
        .await
        .map_err(|_err| Status::unavailable("timed out waiting for the remote source"))?
        .map_err(|err| {
            tracing::error!("error when listing remote revisions: {:?}", err);
            Status::unavailable("there was a problem when trying to list remote revisions")
        })?;

        for build in builds {
            self.database.create_build(&build).await.map_err(|err| {
                tracing::error!("error: {:?}", err);
                Status::internal("error when creating build")
            })?;
        }

        Ok(Response::new(RefreshSourceResponse {}))
    }
}
