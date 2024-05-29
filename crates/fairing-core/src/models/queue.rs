use anyhow::Result;
use uuid::Uuid;

use crate::models::{uuid_v7, LayerId, LayerSetName, ProjectId};

#[derive(Copy, Clone, Debug)]
pub struct QueueMessageId(Uuid);

impl QueueMessageId {
    pub fn new() -> Result<Self> {
        Ok(Self(uuid_v7()?))
    }

    pub fn into_uuid(self) -> Uuid {
        let Self(uuid) = self;
        uuid
    }
}

impl From<Uuid> for QueueMessageId {
    fn from(uuid: Uuid) -> Self {
        Self(uuid)
    }
}

#[derive(Copy, Clone, Debug)]
pub struct WorkerId(Uuid);

impl WorkerId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    pub fn into_uuid(self) -> Uuid {
        let Self(uuid) = self;
        uuid
    }
}

impl From<Uuid> for WorkerId {
    fn from(uuid: Uuid) -> Self {
        Self(uuid)
    }
}

#[derive(Clone, Debug)]
pub struct BuildQueueMessage {
    pub id: QueueMessageId,
    pub project_id: ProjectId,
    pub layer_set_name: LayerSetName,
    pub layer_id: LayerId,
}
