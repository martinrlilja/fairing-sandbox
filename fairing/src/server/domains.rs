use tonic::{Request, Response, Status};

use fairing_core::{
    backends::Database,
    models::{self, prelude::*},
};
use fairing_proto::domains::v1beta1::{
    domains_server::Domains, CreateDomainRequest, Domain, SetDomainSiteRequest,
    SetDomainSiteResponse,
};

#[derive(Debug)]
pub struct DomainsService {
    database: Database,
}

impl DomainsService {
    pub fn new(database: &Database) -> DomainsService {
        DomainsService {
            database: database.clone(),
        }
    }
}

#[tonic::async_trait]
impl Domains for DomainsService {
    #[tracing::instrument]
    async fn create_domain(
        &self,
        request: Request<CreateDomainRequest>,
    ) -> Result<Response<Domain>, Status> {
        let _user = super::auth(&self.database, &request).await?;

        let create_domain = request.into_inner();

        let parent = models::TeamName::parse(create_domain.parent)
            .map_err(|_err| Status::invalid_argument("parent is not a valid team name"))?;

        // FIXME: check if the user is a member of the team.

        let domain = fairing_core::models::CreateDomain {
            resource_id: &create_domain.resource_id,
            parent,
        };

        let domain = self.database.create_domain(&domain).await.map_err(|err| {
            tracing::error!("error: {:?}", err);
            Status::internal("error when creating domain")
        })?;

        let reply = Domain {
            name: domain.name.name().into(),
            acme_label: domain.acme_label.clone(),
            is_validated: domain.is_validated,
        };

        Ok(Response::new(reply))
    }

    async fn set_domain_site(
        &self,
        request: Request<SetDomainSiteRequest>,
    ) -> Result<Response<SetDomainSiteResponse>, Status> {
        let _user = super::auth(&self.database, &request).await?;

        let set_domain_site = request.into_inner();

        let domain = models::DomainName::parse(set_domain_site.name)
            .map_err(|_err| Status::invalid_argument("name is not a valid domain name"))?;

        let site = models::SiteName::parse(set_domain_site.site)
            .map_err(|_err| Status::invalid_argument("site is not a valid site name"))?;

        // FIXME: check if the user is a member of the team.

        self.database
            .set_domain_site(&domain, &site)
            .await
            .map_err(|err| {
                tracing::error!("error: {:?}", err);
                Status::internal("error when setting domain site")
            })?;

        let reply = SetDomainSiteResponse {};

        Ok(Response::new(reply))
    }
}
