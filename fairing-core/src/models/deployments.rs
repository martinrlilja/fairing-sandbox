use anyhow::{ensure, Result};
use chrono::{DateTime, Utc};
use std::ops::Range;

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
    pub modules: sqlx::types::Json<Vec<DeploymentModule>>,
}

pub struct CreateDeployment<'a> {
    pub parent: models::SiteName<'static>,
    pub projections: Vec<CreateDeploymentProjection<'a>>,
    pub modules: Vec<DeploymentModule>,
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub struct DeploymentModule {
    pub file_id: models::FileId,
}

impl<'a> CreateDeployment<'a> {
    pub fn create(&self) -> Result<(Deployment, Vec<DeploymentProjection>)> {
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

        Ok((
            Deployment {
                name,
                created_time: Utc::now(),
                modules: sqlx::types::Json(self.modules.clone()),
            },
            projections,
        ))
    }
}

#[derive(sqlx::FromRow)]
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

#[derive(Clone, Debug, sqlx::FromRow)]
pub struct DeploymentProjectionAsdf {
    pub file_keyspace_id: models::FileKeyspaceId,
    pub layer_set_id: models::LayerSetId,
    pub layer_id: models::LayerId,
    pub mount_path: String,
    pub sub_path: String,
}

#[derive(Clone, Debug)]
pub struct DeploymentHostLookup<'a> {
    base_str: &'a str,
    host: Range<usize>,
    site: Option<Range<usize>>,
    deployment: Option<Range<usize>>,
    tail_labels: Option<Range<usize>>,
    idna: Option<String>,
}

impl<'a> DeploymentHostLookup<'a> {
    pub fn parse(s: &'a str) -> Option<DeploymentHostLookup<'a>> {
        lazy_static::lazy_static! {
            static ref RE: regex::Regex = regex::Regex::new(
                r"^((([a-z0-9]+)(-+[a-z0-9]+)*)(\.(([a-z0-9]+)(-+[a-z0-9]+)*)(\.([a-z0-9]+)(-+[a-z0-9]+)*)*)?)\.?(:[1-9][0-9]*)?$",
            ).unwrap();
        }

        let captures = RE.captures(s)?;

        let host = &captures[1];
        let first_label = &captures[2];
        let tail_labels = captures.get(6);

        if first_label.starts_with("xn--") {
            let (idna, _) = idna::domain_to_unicode(first_label);

            let (site, deployment) = DeploymentHostLookup::parse_first_label(&idna);

            Some(DeploymentHostLookup {
                base_str: s,
                host: 0..host.len(),
                site,
                deployment,
                tail_labels: tail_labels.map(|tail_labels| tail_labels.range()),
                idna: Some(idna),
            })
        } else {
            let (site, deployment) = DeploymentHostLookup::parse_first_label(first_label);

            Some(DeploymentHostLookup {
                base_str: s,
                host: 0..host.len(),
                site,
                deployment,
                tail_labels: tail_labels.map(|tail_labels| tail_labels.range()),
                idna: None,
            })
        }
    }

    fn parse_first_label(first_label: &str) -> (Option<Range<usize>>, Option<Range<usize>>) {
        lazy_static::lazy_static! {
            static ref RE_DEPLOY: regex::Regex = regex::Regex::new(
                r"^([a-z0-9]{20})--(([a-z0-9]+)(-+[a-z0-9]+)*)$",
            ).unwrap();
        }

        if let Some(deploy_captures) = RE_DEPLOY.captures(first_label) {
            (
                deploy_captures.get(2).map(|site| site.range()),
                deploy_captures.get(1).map(|deployment| deployment.range()),
            )
        } else {
            (Some(0..first_label.len()), None)
        }
    }

    pub fn host(&self) -> &str {
        &self.base_str[self.host.clone()]
    }

    pub fn site(&self) -> Option<&str> {
        if let Some(idna) = self.idna.as_ref() {
            self.site.clone().map(|site| &idna[site])
        } else {
            self.site.clone().map(|site| &self.base_str[site])
        }
    }

    pub fn deployment(&self) -> Option<&str> {
        if let Some(idna) = self.idna.as_ref() {
            self.deployment.clone().map(|deployment| &idna[deployment])
        } else {
            self.deployment
                .clone()
                .map(|deployment| &self.base_str[deployment])
        }
    }

    pub fn tail_labels(&self) -> Option<&str> {
        self.tail_labels
            .clone()
            .map(|tail_labels| &self.base_str[tail_labels])
    }
}
