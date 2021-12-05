use anyhow::{ensure, Result};
use chrono::{DateTime, Utc};

use crate::models::{
    self,
    prelude::*,
    resource_name::{validators, ParentedResourceName, ResourceNameInner},
};

crate::impl_resource_name! {
    pub struct DeploymentName<'n>;
}

impl<'n> ParentedResourceName<'n> for DeploymentName<'n> {
    const COLLECTION: &'static str = "deployments";

    type Validator = validators::DomainLabelValidator;

    type Parent = crate::models::SiteName<'static>;
}

#[derive(sqlx::FromRow)]
pub struct Deployment {
    pub name: DeploymentName<'static>,
    pub created_time: DateTime<Utc>,
    pub projections: Vec<DeploymentProjection>,
}

pub struct CreateDeployment<'a> {
    pub parent: models::SiteName<'static>,
    pub projections: Vec<CreateDeploymentProjection<'a>>,
}

impl<'a> CreateDeployment<'a> {
    pub fn create(&self) -> Result<Deployment> {
        use rand::{distributions::WeightedIndex, prelude::*, thread_rng};

        // Generate a name for the deployment.
        // Use weights mimicking english letter frequency to make the names more friendly looking.
        const ALPHABET: &[u8] = b"abcdefghijklmnoprstvwy1234";
        const WEIGHTS: &[u8] = &[
            4, 1, 2, 2, 4, 1, 1, 3, 3, 1, 1, 2, 1, 4, 4, 1, 3, 3, 4, 1, 1, 1, 4, 3, 2, 1,
        ];

        let mut rng = thread_rng();
        let weights = WeightedIndex::new(WEIGHTS)?;

        let resource_id = (0..20)
            .map(|_| {
                let c = ALPHABET[weights.sample(&mut rng)];
                char::from(c)
            })
            .collect::<String>();

        let name = format!("{}/deployments/{}", self.parent.name(), resource_id);
        let name = DeploymentName::parse(name)?;

        let team_name = self.parent.parent();

        ensure!(
            self.projections.len() <= 8,
            "a deployment cannot have more than 8 projections"
        );

        let projections = self
            .projections
            .iter()
            .map(|projection| {
                ensure!(
                    team_name == projection.layer_set.parent().parent(),
                    "layer set projections must belong to the same team as the deployment",
                );

                Ok(DeploymentProjection {
                    layer_set: projection.layer_set.clone(),
                    layer_id: projection.layer_id,
                    mount_path: projection.mount_path.into(),
                    sub_path: projection.sub_path.into(),
                })
            })
            .collect::<Result<_>>()?;

        Ok(Deployment {
            name,
            created_time: Utc::now(),
            projections,
        })
    }
}

pub struct DeploymentProjection {
    pub layer_set: models::LayerSetName<'static>,
    pub layer_id: models::LayerId,
    pub mount_path: String,
    pub sub_path: String,
}

pub struct CreateDeploymentProjection<'a> {
    pub layer_set: models::LayerSetName<'static>,
    pub layer_id: models::LayerId,
    pub mount_path: &'a str,
    pub sub_path: &'a str,
}
