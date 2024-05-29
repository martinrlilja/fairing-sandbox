use anyhow::{anyhow, Result};

use crate::models;

pub enum AuthenticationRole {
    Administrator,
    Viewer,
}

pub enum Authentication {
    Role {
        project_id: models::ProjectId,
        role: AuthenticationRole,
    },
    System {
        project_id: Option<models::ProjectId>,
    },
}

impl Authentication {
    pub fn can<P>(&self, permission: P) -> Result<()>
    where
        P: Into<ResourcePermissions>,
    {
        let permission = permission.into();
        match self {
            Authentication::Role {
                role: AuthenticationRole::Administrator,
                ..
            } => match permission {
                ResourcePermissions::Project(ProjectPermissions::Get)
                | ResourcePermissions::Source(SourcePermissions::Get)
                | ResourcePermissions::Source(SourcePermissions::Create)
                | ResourcePermissions::Source(SourcePermissions::Refresh)
                | ResourcePermissions::LayerSet(LayerSetPermissions::Get)
                | ResourcePermissions::LayerSet(LayerSetPermissions::Create)
                | ResourcePermissions::Layer(LayerPermissions::Create) => Ok(()),
                _ => Err(anyhow!("not allowed")),
            },
            Authentication::Role {
                role: AuthenticationRole::Viewer,
                ..
            } => match permission {
                ResourcePermissions::Project(ProjectPermissions::Get)
                | ResourcePermissions::Source(SourcePermissions::Get)
                | ResourcePermissions::LayerSet(LayerSetPermissions::Get) => Ok(()),
                _ => Err(anyhow!("not allowed")),
            },
            Authentication::System {
                project_id: Some(_),
            } => match permission {
                ResourcePermissions::Project(ProjectPermissions::Get)
                | ResourcePermissions::Source(SourcePermissions::Get)
                | ResourcePermissions::Source(SourcePermissions::Create)
                | ResourcePermissions::Source(SourcePermissions::Refresh)
                | ResourcePermissions::LayerSet(LayerSetPermissions::Get)
                | ResourcePermissions::LayerSet(LayerSetPermissions::Create)
                | ResourcePermissions::Layer(LayerPermissions::Create) => Ok(()),
                _ => Err(anyhow!("not allowed")),
            },
            Authentication::System { project_id: None } => match permission {
                ResourcePermissions::Project(ProjectPermissions::Create) => Ok(()),
                _ => Err(anyhow!("not allowed")),
            },
        }
    }

    pub fn project_id(&self) -> Result<models::ProjectId> {
        match self {
            Authentication::Role { project_id, .. } => Ok(*project_id),
            Authentication::System { project_id, .. } => {
                project_id.ok_or_else(|| anyhow!("not allowed"))
            }
        }
    }
}

#[derive(Copy, Clone, Debug)]
pub enum ResourcePermissions {
    Project(ProjectPermissions),
    Source(SourcePermissions),
    LayerSet(LayerSetPermissions),
    Layer(LayerPermissions),
}

#[derive(Copy, Clone, Debug)]
pub enum ProjectPermissions {
    Get,
    Create,
}

impl Into<ResourcePermissions> for ProjectPermissions {
    fn into(self) -> ResourcePermissions {
        ResourcePermissions::Project(self)
    }
}

#[derive(Copy, Clone, Debug)]
pub enum SourcePermissions {
    Get,
    Create,
    Refresh,
}

impl Into<ResourcePermissions> for SourcePermissions {
    fn into(self) -> ResourcePermissions {
        ResourcePermissions::Source(self)
    }
}

#[derive(Copy, Clone, Debug)]
pub enum LayerSetPermissions {
    Get,
    Create,
}

impl Into<ResourcePermissions> for LayerSetPermissions {
    fn into(self) -> ResourcePermissions {
        ResourcePermissions::LayerSet(self)
    }
}

#[derive(Copy, Clone, Debug)]
pub enum LayerPermissions {
    Get,
    Create,
}

impl Into<ResourcePermissions> for LayerPermissions {
    fn into(self) -> ResourcePermissions {
        ResourcePermissions::Layer(self)
    }
}
