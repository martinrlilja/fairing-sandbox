use tonic::{Request, Response, Status};

use fairing_core::{
    backends::Database,
    models::{self, prelude::*},
};
use fairing_proto::sites::v1beta1::{
    site_source, sites_server::Sites, CreateSiteRequest, CreateSiteSourceRequest,
    DeleteSiteRequest, DeleteSiteResponse, GetSiteRequest, ListSiteSourcesRequest,
    ListSiteSourcesResponse, ListSitesRequest, ListSitesResponse, RefreshSiteSourceRequest,
    RefreshSiteSourceResponse, Site, SiteSource,
};

#[derive(Debug)]
pub struct SitesService {
    database: Database,
}

impl SitesService {
    pub fn new(database: &Database) -> SitesService {
        SitesService {
            database: database.clone(),
        }
    }
}

#[tonic::async_trait]
impl Sites for SitesService {
    #[tracing::instrument]
    async fn list_sites(
        &self,
        request: Request<ListSitesRequest>,
    ) -> Result<Response<ListSitesResponse>, Status> {
        let _user = super::auth(&self.database, &request).await?;

        let parent = models::TeamName::parse(&request.get_ref().parent)
            .map_err(|_err| Status::invalid_argument("parent is not a valid team name"))?;

        // FIXME: check if the user is a member of the team.

        let sites = self.database.list_sites(&parent).await.map_err(|err| {
            tracing::error!("error: {:?}", err);
            Status::internal("error when listing sites")
        })?;

        let resources = sites
            .into_iter()
            .map(|site| Site {
                name: site.name.name().into(),
            })
            .collect();

        let reply = ListSitesResponse { resources };

        Ok(Response::new(reply))
    }

    #[tracing::instrument]
    async fn get_site(&self, request: Request<GetSiteRequest>) -> Result<Response<Site>, Status> {
        let _user = super::auth(&self.database, &request).await?;
        todo!();
    }

    #[tracing::instrument]
    async fn create_site(
        &self,
        request: Request<CreateSiteRequest>,
    ) -> Result<Response<Site>, Status> {
        let _user = super::auth(&self.database, &request).await?;

        let create_site = request.into_inner();

        let parent = models::TeamName::parse(create_site.parent)
            .map_err(|_err| Status::invalid_argument("parent is not a valid team name"))?;

        // FIXME: check if the user is a member of the team.

        let site = fairing_core::models::CreateSite {
            resource_id: &create_site.resource_id,
            parent,
        };

        let site = self.database.create_site(&site).await.map_err(|err| {
            tracing::error!("error: {:?}", err);
            Status::internal("error when creating site")
        })?;

        let reply = Site {
            name: site.name.name().into(),
        };
        Ok(Response::new(reply))
    }

    #[tracing::instrument]
    async fn delete_site(
        &self,
        request: Request<DeleteSiteRequest>,
    ) -> Result<Response<DeleteSiteResponse>, Status> {
        let _user = super::auth(&self.database, &request).await?;
        todo!();
    }

    #[tracing::instrument]
    async fn list_site_sources(
        &self,
        request: Request<ListSiteSourcesRequest>,
    ) -> Result<Response<ListSiteSourcesResponse>, Status> {
        let _user = super::auth(&self.database, &request).await?;

        let parent = models::SiteName::parse(&request.get_ref().parent)
            .map_err(|_err| Status::invalid_argument("parent is not a valid site name"))?;

        // FIXME: check if the user is a member of the team/site.

        let site_sources = self
            .database
            .list_site_sources(&parent)
            .await
            .map_err(|err| {
                tracing::error!("error: {:?}", err);
                Status::internal("error when listing site sources")
            })?;

        let resources = site_sources.into_iter().map(Into::into).collect();

        let reply = ListSiteSourcesResponse { resources };

        Ok(Response::new(reply))
    }

    #[tracing::instrument]
    async fn create_site_source(
        &self,
        request: Request<CreateSiteSourceRequest>,
    ) -> Result<Response<SiteSource>, Status> {
        let _user = super::auth(&self.database, &request).await?;

        let create_site = request.into_inner();

        let parent = models::SiteName::parse(create_site.parent)
            .map_err(|_err| Status::invalid_argument("parent is not a valid site name"))?;

        // FIXME: check if the user is a member of the team.

        let kind = create_site
            .site_source
            .as_ref()
            .and_then(|site_source| site_source.kind.as_ref());

        let kind = match kind {
            Some(site_source::Kind::GitSource(ref git_source)) => {
                models::CreateSiteSourceKind::GitSource {
                    repository_url: &git_source.repository_url,
                }
            }
            None => return Err(Status::invalid_argument("missing kind on site source")),
        };

        let site_source = fairing_core::models::CreateSiteSource {
            resource_id: &create_site.resource_id,
            parent,
            kind,
        };

        let site_source = self
            .database
            .create_site_source(&site_source)
            .await
            .map_err(|err| {
                tracing::error!("error: {:?}", err);
                Status::internal("error when creating site source")
            })?;

        Ok(Response::new(site_source.into()))
    }

    #[tracing::instrument]
    async fn refresh_site_source(
        &self,
        request: Request<RefreshSiteSourceRequest>,
    ) -> Result<Response<RefreshSiteSourceResponse>, Status> {
        let _user = super::auth(&self.database, &request).await?;

        let site_source = request.into_inner();

        let site_source_name = models::SiteSourceName::parse(site_source.name)
            .map_err(|_err| Status::invalid_argument("name is not a valid site source name"))?;

        // FIXME: check if the user is a member of the team.

        let site_source = self
            .database
            .get_site_source(&site_source_name)
            .await
            .map_err(|err| {
                tracing::error!("error: {:?}", err);
                Status::internal("error when getting site source")
            })?
            .ok_or_else(|| Status::not_found("site source not found"))?;

        let remote_site_source = crate::backends::GenericRemoteSiteSource::new();

        let builds = tokio::time::timeout(
            std::time::Duration::from_secs(60),
            remote_site_source.list_tree_revisions(&site_source),
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

        Ok(Response::new(RefreshSiteSourceResponse {}))
    }
}
