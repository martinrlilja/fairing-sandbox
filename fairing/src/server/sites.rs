use tonic::{Request, Response, Status};

use fairing_core::{
    backends::Database,
    models::{self, prelude::*},
};
use fairing_proto::sites::v1beta1::{
    sites_server::Sites, CreateSiteRequest, DeleteSiteRequest, DeleteSiteResponse, GetSiteRequest,
    ListSitesRequest, ListSitesResponse, Site,
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
                base_source: site.base_source.name().into(),
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

        let base_source = models::SourceName::parse(create_site.base_source)
            .map_err(|_err| Status::invalid_argument("base source is not a valid source name"))?;

        // FIXME: check if the user is a member of the team.

        let site = fairing_core::models::CreateSite {
            resource_id: &create_site.resource_id,
            parent,
            base_source,
        };

        let site = self.database.create_site(&site).await.map_err(|err| {
            tracing::error!("error: {:?}", err);
            Status::internal("error when creating site")
        })?;

        let reply = Site {
            name: site.name.name().into(),
            base_source: site.base_source.name().into(),
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
}
