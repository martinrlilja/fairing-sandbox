use anyhow::Result;
use chrono::{DateTime, Utc};

use crate::models::{
    self,
    prelude::*,
    resource_name::{validators, ParentedResourceName, ResourceNameInner},
};

crate::impl_resource_name! {
    pub struct LayerSetName<'n>;
}

impl<'n> ParentedResourceName<'n> for LayerSetName<'n> {
    const COLLECTION: &'static str = "layersets";

    // FIXME: restrict this.
    type Validator = validators::AnyValidator;

    type Parent = models::SourceName<'static>;
}

#[derive(Copy, Clone, Debug, sqlx::Type)]
#[sqlx(transparent)]
pub struct LayerSetId(pub uuid::Uuid);

#[derive(sqlx::FromRow)]
pub struct LayerSet {
    pub id: LayerSetId,
    pub name: LayerSetName<'static>,
    pub created_time: DateTime<Utc>,
}

pub struct CreateLayerSet<'a> {
    pub resource_id: &'a str,
    pub parent: models::SourceName<'static>,
}

impl<'a> CreateLayerSet<'a> {
    pub fn create(&self) -> Result<LayerSet> {
        let id = LayerSetId(uuid::Uuid::new_v4());

        let name = format!("{}/layersets/{}", self.parent.name(), self.resource_id);
        let name = LayerSetName::parse(name)?;

        Ok(LayerSet {
            id,
            name,
            created_time: Utc::now(),
        })
    }
}

#[derive(Copy, Clone, Debug, sqlx::Type)]
#[sqlx(transparent)]
pub struct LayerId(pub uuid::Uuid);
