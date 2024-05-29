use anyhow::{Context as _, Result};
use scylla::{Session, SessionBuilder};

mod domains;
mod files;
mod layers;
mod projects;
mod queue;
mod sources;
mod time;

pub struct ScyllaRepository {
    session: Session,
    domain_statements: domains::Statements,
    file_statements: files::Statements,
    layer_statements: layers::Statements,
}

impl ScyllaRepository {
    pub async fn connect<S: AsRef<str>>(
        known_nodes: &[S],
        keyspace_name: &str,
    ) -> Result<ScyllaRepository> {
        let session = SessionBuilder::new()
            .known_nodes(known_nodes)
            .use_keyspace(keyspace_name, false)
            .build()
            .await?;

        migrate(&session).await?;

        let domain_statements = domains::Statements::prepare(&session).await?;
        let file_statements = files::Statements::prepare(&session).await?;
        let layer_statements = layers::Statements::prepare(&session).await?;

        Ok(ScyllaRepository {
            session,
            domain_statements,
            file_statements,
            layer_statements,
        })
    }
}

async fn migrate(session: &Session) -> Result<()> {
    let initial = include_str!("../migrations/0001_initial.cql");

    let statements = initial
        .split_inclusive(';')
        .map(|statement| statement.trim())
        .filter(|statement| !statement.is_empty());

    for statement in statements {
        session.query(statement, ()).await.context(statement)?;
    }

    Ok(())
}
