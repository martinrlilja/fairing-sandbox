use anyhow::Result;
use async_stream::stream;
use fairing_core::{backends::build_queue, models};
use futures::Stream;
use std::{marker::Unpin, time::Duration};
use tokio::time;
use uuid::Uuid;

use crate::backends::PostgresDatabase;

#[async_trait::async_trait]
impl build_queue::BuildQueueBackend for PostgresDatabase {
    async fn stream(
        &self,
    ) -> Result<Box<dyn Stream<Item = Result<models::TreeRevision>> + Unpin + Send>> {
        let pool = self.pool.clone();

        let stream = stream! {
            loop {
                let mut tx = pool.begin().await?;

                let tree_revision = sqlx::query_as(
                    r"
                    SELECT tree_id, name
                    FROM tree_revisions
                    WHERE status = $1
                    ORDER BY created_time ASC
                    FOR UPDATE SKIP LOCKED;
                    ",
                )
                .bind(models::TreeRevisionStatus::Fetch)
                .fetch_optional(&mut tx)
                .await?;

                let (tree_id, tree_revision_name): (Uuid, String) = match tree_revision {
                    Some((tree_id, tree_revision_name)) => (tree_id, tree_revision_name),
                    None => {
                        time::sleep(Duration::from_secs(2)).await;
                        continue;
                    },
                };

                sqlx::query(
                    r"
                    UPDATE tree_revisions
                    SET status = $3
                    WHERE tree_id = $1 AND name = $2;
                    ",
                )
                .bind(tree_id)
                .bind(&tree_revision_name)
                .bind(models::TreeRevisionStatus::Build)
                .execute(&mut tx)
                .await?;

                let tree_revision = sqlx::query_as(
                    r"
                    SELECT 'teams/' || t.name || '/sites/' || s.name || '/sources/' || ss.name || '/trees/' || tr.name || '/revisions/' || trr.name AS name,
                        trr.created_time, trr.status
                    FROM tree_revisions trr
                    JOIN trees tr
                        ON tr.id = trr.tree_id
                    JOIN site_sources ss
                        ON ss.id = tr.site_source_id
                    JOIN sites s
                        ON s.id = ss.site_id
                    JOIN teams t
                        ON t.id = s.team_id
                    WHERE trr.tree_id = $1 AND trr.name = $2;
                    ",
                )
                .bind(tree_id)
                .bind(&tree_revision_name)
                .fetch_optional(&mut tx)
                .await?;

                tx.commit().await?;

                yield Ok(tree_revision.unwrap());
            }
        };

        // TODO: is there a better way of returning this?
        Ok(Box::new(Box::pin(stream)))
    }
}
