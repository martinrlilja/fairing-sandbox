use anyhow::Result;
use chrono::{DateTime, Utc};

use crate::models::{
    self,
    prelude::*,
    resource_name::{validators, ParentedResourceName, ResourceNameInner},
};

crate::impl_resource_name! {
    pub struct SiteName<'n>;
}

impl<'n> ParentedResourceName<'n> for SiteName<'n> {
    const COLLECTION: &'static str = "sites";

    type Validator = validators::DomainLabelValidator;

    type Parent = crate::models::TeamName<'static>;
}

#[derive(sqlx::FromRow)]
pub struct Site {
    pub name: SiteName<'static>,
    pub created_time: DateTime<Utc>,
    pub base_source: models::SourceName<'static>,
}

pub struct CreateSite<'a> {
    pub resource_id: &'a str,
    pub parent: models::TeamName<'static>,
    pub base_source: models::SourceName<'static>,
}

impl<'a> CreateSite<'a> {
    pub fn create(&self) -> Result<Site> {
        let name = format!("{}/sites/{}", self.parent.name(), self.resource_id);
        let name = SiteName::parse(name)?;

        Ok(Site {
            name,
            created_time: Utc::now(),
            base_source: self.base_source.clone(),
        })
    }
}
