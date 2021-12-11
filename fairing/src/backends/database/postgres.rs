use anyhow::{anyhow, ensure, Context, Result};
use std::sync::Arc;
use uuid::Uuid;

use fairing_core::{
    backends::{build_queue, database, file_metadata},
    models::{self, prelude::*},
};

#[derive(Clone, Debug)]
pub struct PostgresDatabase {
    pub(crate) pool: sqlx::PgPool,
}

impl PostgresDatabase {
    #[tracing::instrument]
    pub async fn connect(uri: &str) -> Result<PostgresDatabase> {
        let pool = sqlx::PgPool::connect(uri).await?;

        Ok(PostgresDatabase { pool })
    }

    pub fn build_queue(&self) -> build_queue::BuildQueue {
        Arc::new(self.clone())
    }

    pub fn database(&self) -> database::Database {
        Arc::new(self.clone())
    }

    pub fn file_metadata(&self) -> file_metadata::FileMetadata {
        Arc::new(self.clone())
    }

    pub async fn migrate(&self) -> Result<()> {
        use sqlx::migrate::{Migrate, Migration, MigrationType};

        const MIGRATIONS: &[(i64, &'static str, &'static str)] = &[(
            1,
            "initial",
            include_str!("../../../migrations/postgres/0001_initial.sql"),
        )];

        tracing::info!("checking migrations");

        let mut conn = self.pool.acquire().await?;

        conn.lock().await?;

        conn.ensure_migrations_table().await?;

        if let Some(version) = conn.dirty_version().await? {
            return Err(anyhow!("dirty migration: {:04}", version));
        }

        let applied_migrations = conn
            .list_applied_migrations()
            .await?
            .into_iter()
            .map(|applied_migration| (applied_migration.version, applied_migration))
            .collect::<std::collections::HashMap<_, _>>();

        let mut num_applied_migrations = 0;

        for &(version, description, sql) in MIGRATIONS {
            let migration = Migration::new(
                version,
                description.into(),
                MigrationType::Simple,
                sql.into(),
            );

            if let Some(applied_migration) = applied_migrations.get(&version) {
                ensure!(
                    applied_migration.checksum == migration.checksum,
                    "migration checksum mismatch {:04}: {}",
                    version,
                    description
                );
            } else {
                tracing::info!("applying migration {:04}: {}", version, description);
                conn.apply(&migration).await?;
                num_applied_migrations += 1;
            }
        }

        conn.unlock().await?;

        if num_applied_migrations > 0 {
            tracing::info!("applied migrations");
        } else {
            tracing::info!("no migrations to apply");
        }

        Ok(())
    }

    async fn create_team(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        team: &models::CreateTeam<'_>,
    ) -> Result<models::Team> {
        let (team, team_member) = team.create()?;

        let team_id = Uuid::new_v4();

        sqlx::query(
            r"
            INSERT INTO teams (id, name, created_time, file_keyspace_id)
            VALUES ($1, $2, $3, $4);
            ",
        )
        .bind(&team_id)
        .bind(team.name.resource())
        .bind(&team.created_time)
        .bind(&team.file_keyspace_id)
        .execute(&mut *tx)
        .await
        .context("create team")?;

        sqlx::query(
            r"
            INSERT INTO team_members (team_id, user_id, created_time)
            SELECT $1, id, $3
            FROM users
            WHERE name = $2;
            ",
        )
        .bind(&team_id)
        .bind(&team_member.name.resource())
        .bind(&team_member.created_time)
        .execute(&mut *tx)
        .await
        .context("create team (owner)")?;

        Ok(team)
    }
}

#[async_trait::async_trait]
impl database::UserRepository for PostgresDatabase {
    #[tracing::instrument]
    async fn get_user(&self, user_name: &models::UserName) -> Result<Option<models::User>> {
        let user = sqlx::query_as(
            r"
            SELECT 'users/' || name, created_time
            FROM users
            WHERE name = $1;
            ",
        )
        .bind(user_name.resource())
        .fetch_optional(&self.pool)
        .await
        .context("get user")?;

        Ok(user)
    }

    //#[tracing::instrument]
    async fn create_user(&self, user: &models::CreateUser) -> Result<models::User> {
        let mut tx = self.pool.begin().await?;
        let (user, password) = user.create()?;

        let password_hash = tokio::task::spawn_blocking(|| password.hash()).await?;

        let user_id = Uuid::new_v4();

        sqlx::query(
            r"
            INSERT INTO users (id, name, password, created_time)
            VALUES ($1, $2, $3, $4);
            ",
        )
        .bind(&user_id)
        .bind(user.name.resource())
        .bind(&password_hash)
        .bind(&user.created_time)
        .execute(&mut tx)
        .await
        .context("create user")?;

        //self.create_team(&mut tx, &team).await?;

        tx.commit().await?;

        Ok(user)
    }

    //#[tracing::instrument]
    async fn verify_user_password(
        &self,
        user_name: &models::UserName,
        password: models::Password,
    ) -> Result<()> {
        let password_hash: Option<String> = sqlx::query_scalar(
            r"
            SELECT password
            FROM users
            WHERE name = $1;
            ",
        )
        .bind(user_name.resource())
        .fetch_optional(&self.pool)
        .await
        .context("verify user password")?;

        if let Some(password_hash) = password_hash {
            password.verify(&password_hash)
        } else {
            Err(anyhow!("user not found"))
        }
    }
}

#[async_trait::async_trait]
impl database::TeamRepository for PostgresDatabase {
    #[tracing::instrument]
    async fn list_teams(&self, user_name: &models::UserName) -> Result<Vec<models::Team>> {
        let teams = sqlx::query_as(
            r"
            SELECT 'teams/' || t.name AS name, t.created_time
            FROM teams t
            JOIN team_members tm
                ON t.id = tm.team_id
            JOIN users u
                ON u.id = tm.user_id
            WHERE u.name = $1
            LIMIT 100;
            ",
        )
        .bind(user_name.resource())
        .fetch_all(&self.pool)
        .await
        .context("list teams")?;

        Ok(teams)
    }

    #[tracing::instrument]
    async fn get_team(&self, team_name: &models::TeamName) -> Result<Option<models::Team>> {
        let team = sqlx::query_as(
            r"
            SELECT 'teams/' || t.name AS name, t.created_time, t.file_keyspace_id
            FROM teams t
            WHERE t.name = $1
            LIMIT 100;
            ",
        )
        .bind(team_name.resource())
        .fetch_optional(&self.pool)
        .await
        .context("get team")?;

        Ok(team)
    }

    #[tracing::instrument]
    async fn create_team(&self, team: &models::CreateTeam) -> Result<models::Team> {
        let mut tx = self.pool.begin().await?;

        let team = self.create_team(&mut tx, &team).await?;

        tx.commit().await.context("create team (commit)")?;

        Ok(team)
    }

    #[tracing::instrument]
    async fn delete_team(&self, team_name: &models::TeamName) -> Result<()> {
        sqlx::query(
            r"
            DELETE FROM teams
            WHERE name = $1;
            ",
        )
        .bind(team_name.resource())
        .execute(&self.pool)
        .await
        .context("delete team")?;

        Ok(())
    }

    #[tracing::instrument]
    async fn list_team_members(
        &self,
        team_name: &models::TeamName,
    ) -> Result<Vec<models::TeamMember>> {
        let teams = sqlx::query_as(
            r"
            SELECT 'teams/' || t.name || '/members/' || u.name AS name, tm.created_time
            FROM team_members tm
            JOIN teams t
                ON t.id = tm.team_id
            JOIN users u
                ON u.id = tm.user_id
            WHERE u.name = $1
            LIMIT 100;
            ",
        )
        .bind(team_name.resource())
        .fetch_all(&self.pool)
        .await
        .context("list team members")?;

        Ok(teams)
    }

    #[tracing::instrument]
    async fn create_team_member(
        &self,
        team_member: &models::CreateTeamMember,
    ) -> Result<models::TeamMember> {
        let team_member = team_member.create();

        let query_result = sqlx::query(
            r"
            INSERT INTO team_members (team_id, user_id, created_time)
            SELECT t.pk, u.pk, $3
            FROM teams t
            JOIN users u
                ON 1 = 1
            WHERE t.name = $1 AND u.name = $2;
            ",
        )
        .bind(team_member.name.parent().resource())
        .bind(team_member.name.resource())
        .execute(&self.pool)
        .await
        .context("create team member")?;

        ensure!(
            query_result.rows_affected() == 1,
            "team member parent not found"
        );

        Ok(team_member)
    }

    #[tracing::instrument]
    async fn delete_team_member(&self, team_member_name: &models::TeamMemberName) -> Result<()> {
        sqlx::query(
            r"
            DELETE FROM team_members tm
            JOIN teams t
                ON t.id = tm.team_id
            JOIN users u
                ON u.id = tm.user_id
            WHERE t.name = $1 AND u.name = $2;
            ",
        )
        .bind(team_member_name.parent().resource())
        .bind(team_member_name.resource())
        .execute(&self.pool)
        .await
        .context("delete team member")?;

        Ok(())
    }
}

#[async_trait::async_trait]
impl database::SiteRepository for PostgresDatabase {
    async fn list_sites(&self, team_name: &models::TeamName) -> Result<Vec<models::Site>> {
        let sites = sqlx::query_as(
            r"
            SELECT 'teams/' || t.name || '/sites/' || s.name AS name, s.created_time,
                    'teams/' || t.name || '/source/' || src.name AS base_source
            FROM sites s
            JOIN teams t
                ON t.id = s.team_id
            JOIN sources src
                ON src.team_id = t.id AND src.id = s.base_source_id
            WHERE t.name = $1;
            ",
        )
        .bind(team_name.resource())
        .fetch_all(&self.pool)
        .await
        .context("list sites")?;

        Ok(sites)
    }

    async fn list_sites_with_base_source(
        &self,
        source_name: &models::SourceName,
    ) -> Result<Vec<models::Site>> {
        let sites = sqlx::query_as(
            r"
            SELECT 'teams/' || t.name || '/sites/' || s.name AS name, s.created_time,
                    'teams/' || t.name || '/source/' || src.name AS base_source
            FROM sites s
            JOIN teams t
                ON t.id = s.team_id
            JOIN sources src
                ON src.team_id = t.id AND src.id = s.base_source_id
            WHERE t.name = $1 AND src.name = $2;
            ",
        )
        .bind(source_name.parent().resource())
        .bind(source_name.resource())
        .fetch_all(&self.pool)
        .await
        .context("list sites with base source")?;

        Ok(sites)
    }

    async fn get_site(&self, site_name: &models::SiteName) -> Result<Option<models::Site>> {
        let site = sqlx::query_as(
            r"
            SELECT 'teams/' || t.name || '/sites/' || s.name AS name, s.created_time,
                    'teams/' || t.name || '/source/' || src.name AS base_source
            FROM sites s
            JOIN teams t
                ON t.id = s.team_id
            JOIN sources src
                ON src.team_id = t.id AND src.id = s.base_source_id
            WHERE t.name = $1 AND s.name = $2;
            ",
        )
        .bind(site_name.parent().resource())
        .bind(site_name.resource())
        .fetch_optional(&self.pool)
        .await
        .context("get site")?;

        Ok(site)
    }

    async fn create_site(&self, site: &models::CreateSite) -> Result<models::Site> {
        let site = site.create()?;
        let site_id = Uuid::new_v4();

        let query_result = sqlx::query(
            r"
            INSERT INTO sites (id, created_time, name, team_id, base_source_id)
            SELECT $1, $2, $3, src.team_id, src.id
            FROM sources src
            JOIN teams t
                ON t.id = src.team_id
            WHERE t.name = $4 AND src.name = $5;
            ",
        )
        .bind(&site_id)
        .bind(&site.created_time)
        .bind(site.name.resource())
        .bind(site.name.parent().resource())
        .bind(site.base_source.resource())
        .execute(&self.pool)
        .await
        .context("create site")?;

        ensure!(
            query_result.rows_affected() == 1,
            "site parent or base source not found"
        );

        Ok(site)
    }

    async fn delete_site(&self, _site_name: &models::SiteName) -> Result<()> {
        todo!();
    }
}

#[async_trait::async_trait]
impl database::SourceRepository for PostgresDatabase {
    async fn list_sources(&self, team_name: &models::TeamName) -> Result<Vec<models::Source>> {
        let sources = sqlx::query_as(
            r"
            SELECT 'teams/' || t.name || '/sources/' || src.name AS name,
                    src.created_time, src.hook_token, src.last_refresh_time,
                    src_git.repository_url AS git_repository_url,
                    src_git.id_ed25519_secret_key AS git_id_ed25519_secret_key
            FROM sources src
            LEFT JOIN source_git src_git
                ON src_git.source_id = src.id
            JOIN teams t
                ON t.id = src.team_id
            WHERE t.name = $1;
            ",
        )
        .bind(team_name.resource())
        .fetch_all(&self.pool)
        .await
        .context("list sources")?;

        Ok(sources)
    }

    async fn get_source(&self, source_name: &models::SourceName) -> Result<Option<models::Source>> {
        let source = sqlx::query_as(
            r"
            SELECT 'teams/' || t.name || '/sources/' || src.name AS name,
                    src.created_time, src.hook_token, src.last_refresh_time,
                    src_git.repository_url AS git_repository_url,
                    src_git.id_ed25519_secret_key AS git_id_ed25519_secret_key
            FROM sources src
            LEFT JOIN source_git src_git
                ON src_git.source_id = src.id
            JOIN teams t
                ON t.id = src.team_id
            WHERE t.name = $1 AND src.name = $2;
            ",
        )
        .bind(source_name.parent().resource())
        .bind(source_name.resource())
        .fetch_optional(&self.pool)
        .await
        .context("get source")?;

        Ok(source)
    }

    async fn create_source(&self, source: &models::CreateSource) -> Result<models::Source> {
        let source = source.create()?;
        let source_id = Uuid::new_v4();

        let mut tx = self.pool.begin().await?;

        let query_result = sqlx::query(
            r"
            INSERT INTO sources (id, created_time, name, team_id, hook_token)
            SELECT $1, $2, $3, t.id, $4
            FROM teams t
            WHERE t.name = $5;
            ",
        )
        .bind(&source_id)
        .bind(&source.created_time)
        .bind(source.name.resource())
        .bind(&source.hook_token)
        .bind(source.name.parent().resource())
        .execute(&mut tx)
        .await
        .context("create source")?;

        ensure!(query_result.rows_affected() == 1, "source parent not found");

        match source.kind {
            Some(models::SourceKind::GitSource(ref git_source)) => {
                sqlx::query(
                    r"
                    INSERT INTO source_git (source_id, repository_url, id_ed25519_secret_key)
                    VALUES ($1, $2, $3);
                    ",
                )
                .bind(&source_id)
                .bind(&git_source.repository_url.as_str())
                .bind(&git_source.id_ed25519.secret_key_to_slice())
                .execute(&mut tx)
                .await
                .context("create source (git)")?;
            }
            None => (),
        }

        tx.commit().await.context("create source (commit)")?;

        Ok(source)
    }
}

#[async_trait::async_trait]
impl database::DeploymentRepository for PostgresDatabase {
    async fn get_deployment(
        &self,
        _deployment_name: &models::DeploymentName,
    ) -> Result<Option<models::Deployment>> {
        todo!();
    }

    async fn create_deployment(
        &self,
        deployment: &models::CreateDeployment,
    ) -> Result<models::Deployment> {
        let (deployment, projections) = deployment.create()?;
        let deployment_id = Uuid::new_v4();

        let mut tx = self.pool.begin().await?;

        let query_result = sqlx::query(
            r"
            INSERT INTO deployments (id, created_time, name, site_id)
            SELECT $1, $2, $3, s.id
            FROM sites s
            JOIN teams t
                ON t.id = s.team_id
            WHERE t.name = $4 AND s.name = $5;
            ",
        )
        .bind(&deployment_id)
        .bind(&deployment.created_time)
        .bind(deployment.name.resource())
        .bind(deployment.name.parent().parent().resource())
        .bind(deployment.name.parent().resource())
        .execute(&mut tx)
        .await
        .context("create deployment")?;

        ensure!(
            query_result.rows_affected() == 1,
            "deployment parent not found: {}",
            deployment.name.name(),
        );

        for projection in projections.iter() {
            let projection_id = Uuid::new_v4();
            let query_result = sqlx::query(
                r"
                INSERT INTO deployment_projections (id, deployment_id, layer_set_id, layer_id, mount_path, sub_path)
                SELECT $1, $2, ls.id, $3, $4, $5
                FROM layer_sets ls
                JOIN sources src
                    ON src.id = ls.source_id
                JOIN teams t
                    ON t.id = src.team_id
                WHERE t.name = $6 AND src.name = $7 AND ls.name = $8;
                ",
            )
            .bind(&projection_id)
            .bind(&deployment_id)
            .bind(&projection.layer_id)
            .bind(&projection.mount_path)
            .bind(&projection.sub_path)
            .bind(projection.layer_set.parent().parent().resource())
            .bind(projection.layer_set.parent().resource())
            .bind(projection.layer_set.resource())
            .execute(&mut tx)
            .await
            .context("create deployment (projection)")?;

            ensure!(
                query_result.rows_affected() == 1,
                "deployment projection layer set not found: {}",
                projection.layer_set.name(),
            );
        }

        tx.commit().await.context("create deployment (commit)")?;

        Ok(deployment)
    }

    async fn get_deployment_by_host(
        &self,
        lookup: &models::DeploymentHostLookup,
    ) -> Result<Option<Vec<models::DeploymentProjectionAsdf>>> {
        let tail_labels = lookup.tail_labels();

        // TODO: turn this into a setting.
        if tail_labels == Some("localhost") {
            if let (Some(site), Some(deployment)) = (lookup.site(), lookup.deployment()) {
                let projections = sqlx::query_as(
                    r"
                    SELECT t.file_keyspace_id, dp.layer_set_id, dp.layer_id, dp.mount_path, dp.sub_path
                    FROM deployment_projections dp
                    JOIN deployments d
                        ON d.id = dp.deployment_id
                    JOIN sites s
                        ON s.id = d.site_id
                    JOIN teams t
                        ON t.id = s.team_id
                    WHERE s.name = $1 AND d.name = $2;
                    ",
                )
                .bind(site)
                .bind(deployment)
                .fetch_all(&self.pool)
                .await?;

                Ok(Some(projections))
            } else {
                Ok(None)
            }
        } else {
            // TODO: lookup registered domains
            Ok(None)
        }
    }
}

#[async_trait::async_trait]
impl database::LayerRepository for PostgresDatabase {
    async fn list_layer_sets(
        &self,
        source_name: &models::SourceName,
    ) -> Result<Vec<models::LayerSet>> {
        let layer_sets = sqlx::query_as(
            r"
            SELECT 'teams/' || t.name || '/sources/' || src.name || '/layersets/' || ls.name AS name,
                    ls.created_time
            FROM layer_sets ls
            JOIN sources src
                ON src.id = ls.source_id
            JOIN teams t
                ON t.id = src.team_id
            WHERE t.name = $1 AND src.name = $2;
            ",
        )
        .bind(source_name.parent().resource())
        .bind(source_name.resource())
        .fetch_all(&self.pool)
        .await
        .context("listing layer sets")?;

        Ok(layer_sets)
    }

    async fn get_layer_set(
        &self,
        layer_set_name: &models::LayerSetName,
    ) -> Result<Option<models::LayerSet>> {
        let layer_set = sqlx::query_as(
            r"
            SELECT ls.id, 'teams/' || t.name || '/sources/' || src.name || '/layersets/' || ls.name AS name,
                    ls.created_time
            FROM layer_sets ls
            JOIN sources src
                ON src.id = ls.source_id
            JOIN teams t
                ON t.id = src.team_id
            WHERE t.name = $1 AND src.name = $2 AND ls.name = $3;
            ",
        )
        .bind(layer_set_name.parent().parent().resource())
        .bind(layer_set_name.parent().resource())
        .bind(layer_set_name.resource())
        .fetch_optional(&self.pool)
        .await
        .context("getting layer set")?;

        Ok(layer_set)
    }

    async fn create_build(&self, build: &models::CreateBuild) -> Result<models::Build> {
        let mut tx = self.pool.begin().await?;

        let layer_set = models::CreateLayerSet {
            resource_id: build.parent.resource(),
            parent: build.parent.parent(),
        };

        let layer_set = layer_set.create().context("creating layer set")?;

        sqlx::query(
            r"
            INSERT INTO layer_sets (id, created_time, name, source_id)
            SELECT $1, $2, $3, src.id
            FROM sources src
            JOIN teams t
                ON t.id = src.team_id
            WHERE t.name = $4 AND src.name = $5
            ON CONFLICT (source_id, name) DO NOTHING;
            ",
        )
        .bind(layer_set.id)
        .bind(layer_set.created_time)
        .bind(layer_set.name.resource())
        .bind(layer_set.name.parent().parent().resource())
        .bind(layer_set.name.parent().resource())
        .execute(&mut tx)
        .await
        .context("inserting layer set")?;

        let build = build.create().context("creating build")?;

        sqlx::query(
            r"
            INSERT INTO builds (id, created_time, name, layer_set_id, layer_id, status, source_reference)
            SELECT $1, $2, $3, ls.id, $4, $5, $6
            FROM layer_sets ls
            JOIN sources src
                ON src.id = ls.source_id
            JOIN teams t
                ON t.id = src.team_id
            WHERE t.name = $7 AND src.name = $8 AND ls.name = $9;
            ",
        )
        .bind(uuid::Uuid::new_v4())
        .bind(build.created_time)
        .bind(build.name.resource())
        .bind(build.layer_id)
        .bind(build.status)
        .bind(&build.source_reference)
        .bind(build.name.parent().parent().parent().resource())
        .bind(build.name.parent().parent().resource())
        .bind(build.name.parent().resource())
        .execute(&mut tx)
        .await
        .context("inserting build")?;

        tx.commit().await?;

        Ok(build)
    }
}
