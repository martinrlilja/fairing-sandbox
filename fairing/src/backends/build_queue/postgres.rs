use anyhow::{Context as _, Result};
use async_stream::stream;
use fairing_core::{backends::build_queue, models};
use futures::Stream;
use std::{marker::Unpin, time::Duration};
use tokio::time;
use uuid::Uuid;

use crate::backends::PostgresDatabase;

#[async_trait::async_trait]
impl build_queue::BuildQueueBackend for PostgresDatabase {
    async fn stream_builds(
        &self,
    ) -> Result<Box<dyn Stream<Item = Result<models::Build>> + Unpin + Send>> {
        let pool = self.pool.clone();

        let stream = stream! {
            loop {
                let mut tx = pool.begin().await?;

                let build = sqlx::query_as(
                    r"
                    SELECT layer_set_id, name
                    FROM builds
                    WHERE status = $1
                    ORDER BY created_time ASC
                    FOR UPDATE SKIP LOCKED;
                    ",
                )
                .bind(models::BuildStatus::Queued)
                .fetch_optional(&mut tx)
                .await
                .context("stream builds (next)")?;

                let (layer_set_id, build_name): (Uuid, String) = match build {
                    Some((layer_set_id, build_name)) => (layer_set_id, build_name),
                    None => {
                        time::sleep(Duration::from_secs(2)).await;
                        continue;
                    },
                };

                sqlx::query(
                    r"
                    UPDATE builds
                    SET status = $3
                    WHERE layer_set_id = $1 AND name = $2;
                    ",
                )
                .bind(layer_set_id)
                .bind(&build_name)
                .bind(models::BuildStatus::Building)
                .execute(&mut tx)
                .await
                .context("stream builds (claim)")?;

                let build = sqlx::query_as(
                    r"
                    SELECT 'teams/' || t.name || '/sources/' || src.name || '/layersets/' || ls.name || '/builds/' || b.name AS name,
                        b.created_time, b.layer_id, b.status, b.source_reference
                    FROM builds b
                    JOIN layer_sets ls
                        ON ls.id = b.layer_set_id
                    JOIN sources src
                        ON src.id = ls.source_id
                    JOIN teams t
                        ON t.id = src.team_id
                    WHERE b.layer_set_id = $1 AND b.name = $2;
                    ",
                )
                .bind(layer_set_id)
                .bind(&build_name)
                .fetch_optional(&mut tx)
                .await
                .context("stream builds (get)")?;

                tx.commit().await?;

                yield Ok(build.unwrap());
            }
        };

        // is there a better way of returning this?
        Ok(Box::new(Box::pin(stream)))
    }
}
