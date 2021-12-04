use anyhow::Result;
use chrono::{DateTime, Utc};
use std::time::SystemTime;

use crate::models::{
    self,
    prelude::*,
    resource_name::{validators, ParentedResourceName, ResourceNameInner},
};

crate::impl_resource_name! {
    pub struct BuildName<'n>;
}

impl<'n> ParentedResourceName<'n> for BuildName<'n> {
    const COLLECTION: &'static str = "builds";

    // FIXME: restrict this.
    type Validator = validators::AnyValidator;

    type Parent = models::LayerSetName<'static>;
}

#[derive(Copy, Clone, Debug, sqlx::Type)]
#[sqlx(type_name = "build_status")]
#[sqlx(rename_all = "snake_case")]
pub enum BuildStatus {
    Queued,
    Building,
    Complete,
}

#[derive(sqlx::FromRow)]
pub struct Build {
    pub name: BuildName<'static>,
    pub created_time: DateTime<Utc>,
    pub layer_id: models::LayerId,
    pub status: BuildStatus,
    pub source_reference: String,
}

pub struct CreateBuild {
    pub parent: models::LayerSetName<'static>,
    pub source_reference: String,
}

impl CreateBuild {
    pub fn create(&self) -> Result<Build> {
        let resource_id = uuid::Uuid::new_v4().to_string();

        let name = format!("{}/builds/{}", self.parent.name(), resource_id);
        let name = BuildName::parse(name)?;

        // A crappy implementation of UUIDv7. Should be changed to a proper one in the future.
        let timestamp = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let seconds = (timestamp / 10_u128.pow(9)) & 0xffff_ffff;
        // Not really the same as the spec but avoids converting to floats.
        let subsec = (timestamp % 10_u128.pow(9)) & 0x3fff_ffff;
        // Ignore sequnce and node id.
        let uuid = (seconds << 96)
            | (0x0 << 92)
            | ((subsec >> 18) << 80)
            | (0x7 << 76)
            | (((subsec >> 6) & 0xfff) << 64)
            | (0b01 << 62)
            | ((subsec & 0x40) << 56);
        let layer_id = models::LayerId(uuid::Uuid::from_u128(uuid));

        Ok(Build {
            name,
            created_time: Utc::now(),
            layer_id,
            status: BuildStatus::Queued,
            source_reference: self.source_reference.clone(),
        })
    }
}
