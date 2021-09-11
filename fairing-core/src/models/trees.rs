use anyhow::Result;
use chrono::{DateTime, Utc};
use std::borrow::Cow;

use crate::models::{
    self,
    prelude::*,
    resource_name::{validators, ParentedResourceName, ResourceNameInner},
};

crate::impl_resource_name! {
    pub struct TreeName<'n>;
    pub struct TreeRevisionName<'n>;
}

impl<'n> ParentedResourceName<'n> for TreeName<'n> {
    const COLLECTION: &'static str = "trees";

    // FIXME: restrict this.
    type Validator = validators::AnyValidator;

    type Parent = models::SiteSourceName<'static>;
}

impl<'n> ParentedResourceName<'n> for TreeRevisionName<'n> {
    const COLLECTION: &'static str = "revisions";

    type Validator = validators::RevisionValidator;

    type Parent = models::TreeName<'static>;
}

#[derive(Copy, Clone, Debug)]
pub struct TreeId(pub uuid::Uuid);

#[derive(sqlx::FromRow)]
pub struct Tree {
    pub name: TreeName<'static>,
    pub created_time: DateTime<Utc>,
    // /// Serially increasing version numbers for tree revisions.
    //pub version: i64,
}

pub struct CreateTree<'a> {
    pub resource_id: &'a str,
    pub parent: models::SiteSourceName<'static>,
}

impl<'a> CreateTree<'a> {
    pub fn create(&self) -> Result<Tree> {
        let name = format!("{}/trees/{}", self.parent.name(), self.resource_id);
        let name = TreeName::parse(name)?;

        Ok(Tree {
            name,
            created_time: Utc::now(),
            //version: 1,
        })
    }
}

#[derive(Clone, Debug, sqlx::FromRow)]
pub struct TreeRevision {
    pub name: TreeRevisionName<'static>,
    //pub version: i64,
    pub created_time: DateTime<Utc>,
    pub status: TreeRevisionStatus,
}

#[derive(Copy, Clone, Debug, sqlx::Type)]
#[sqlx(type_name = "tree_revision_status")]
#[sqlx(rename_all = "snake_case")]
pub enum TreeRevisionStatus {
    Ignore,
    Fetch,
    Build,
    Draft,
    Complete,
}

#[derive(Clone, Debug)]
pub struct CreateTreeRevision<'a> {
    pub resource_id: Cow<'a, str>,
    pub parent: models::TreeName<'static>,
    pub status: TreeRevisionStatus,
}

impl<'a> CreateTreeRevision<'a> {
    pub fn create(&self /*, version: i64*/) -> Result<TreeRevision> {
        /*
        use rand::distributions::Distribution;
        const HEX: &[u8; 16] = b"0123456789abcdef";

        let draft_token = rand::distributions::Uniform::new_inclusive(0, HEX.len() - 1)
            .sample_iter(&mut rand::thread_rng())
            .take(32)
            .map(|i| char::from(HEX[i]))
            .collect::<String>();
        */

        let name = format!("{}/revisions/{}", self.parent.name(), self.resource_id);
        let name = TreeRevisionName::parse(name)?;

        Ok(TreeRevision {
            name,
            //version,
            created_time: Utc::now(),
            status: self.status,
        })
    }
}
