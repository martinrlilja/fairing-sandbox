use anyhow::Result;
use scylla::{query::Query, statement::SerialConsistency, FromRow};
use uuid::Uuid;

use fairing_core2::{models, repositories::QueueRepository};

use crate::ScyllaRepository;

#[derive(Debug, FromRow)]
struct BuildQueueMessage {
    id: Uuid,
    project_id: Uuid,
    layer_set_name: String,
    layer_id: Uuid,
}

impl Into<models::BuildQueueMessage> for BuildQueueMessage {
    fn into(self) -> models::BuildQueueMessage {
        models::BuildQueueMessage {
            id: self.id.into(),
            project_id: self.project_id.into(),
            layer_set_name: self.layer_set_name.parse().unwrap(),
            layer_id: self.layer_id.into(),
        }
    }
}

#[async_trait::async_trait]
impl QueueRepository for ScyllaRepository {
    async fn queue_build(&self, message: &models::BuildQueueMessage) -> Result<()> {
        self.session
            .query(
                r"
                INSERT INTO build_queue_messages (
                    bucket, id, worker_id, project_id, layer_set_name, layer_id
                )
                VALUES (?, ?, ?, ?, ?, ?);
                ",
                (
                    0i64,
                    message.id.into_uuid(),
                    Uuid::nil(),
                    message.project_id.into_uuid(),
                    message.layer_set_name.as_str(),
                    message.layer_id.into_uuid(),
                ),
            )
            .await?;

        Ok(())
    }

    async fn assign_build(
        &self,
        worker_id: models::WorkerId,
    ) -> Result<Option<models::BuildQueueMessage>> {
        let unassigned_messages = self
            .session
            .query(
                r"
                SELECT id, project_id, layer_set_name, layer_id
                FROM build_queue_messages
                WHERE bucket = ? AND worker_id IS NULL
                ALLOW FILTERING;
                ",
                (0i64,),
            )
            .await?
            .rows_typed()?;

        for row in unassigned_messages {
            let message: BuildQueueMessage = row?;

            let mut query = Query::new(
                r"
                UPDATE build_queue_messages
                USING TTL 1800
                SET worker_id = ?
                WHERE bucket = ? AND id = ?
                IF worker_id = ?;
                ",
            );

            query.set_serial_consistency(Some(SerialConsistency::Serial));

            self.session
                .query(
                    query,
                    (worker_id.into_uuid(), 0i64, message.id, Uuid::nil()),
                )
                .await?;
        }

        todo!();
    }
}
