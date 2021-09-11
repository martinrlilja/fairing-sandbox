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
            INSERT INTO teams (id, name, created_time)
            VALUES ($1, $2, $3);
            ",
        )
        .bind(&team_id)
        .bind(team.name.resource())
        .bind(&team.created_time)
        .execute(&mut *tx)
        .await?;

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
        .await?;

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
        .await?;

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
        .await?;

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
        .await?;

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
        .await?;

        Ok(teams)
    }

    #[tracing::instrument]
    async fn get_team(
        &self,
        user_name: &models::UserName,
        team_name: &models::TeamName,
    ) -> Result<Option<models::Team>> {
        let team = sqlx::query_as(
            r"
            SELECT 'teams/' || t.name AS name, t.created_time
            FROM teams t
            JOIN team_members tmname TEXT UNIQUE NOT
                ON t.id = tm.team_id
            JOIN users u
                ON u.id = tm.user_id
            WHERE u.name = $1 AND t.name = $2
            LIMIT 100;
            ",
        )
        .bind(user_name.resource())
        .bind(team_name.resource())
        .fetch_optional(&self.pool)
        .await?;

        Ok(team)
    }

    #[tracing::instrument]
    async fn create_team(&self, team: &models::CreateTeam) -> Result<models::Team> {
        let mut tx = self.pool.begin().await?;

        let team = self.create_team(&mut tx, &team).await?;

        tx.commit().await?;

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
        .await?;

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
        .await?;

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
        .await?;

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
        .await?;

        Ok(())
    }
}

#[async_trait::async_trait]
impl database::SiteRepository for PostgresDatabase {
    async fn list_sites(&self, team_name: &models::TeamName) -> Result<Vec<models::Site>> {
        let sites = sqlx::query_as(
            r"
            SELECT 'teams/' || t.name || '/sites/' || s.name AS name, s.created_time
            FROM sites s
            JOIN teams t
                ON t.id = s.team_id
            WHERE t.name = $1;
            ",
        )
        .bind(team_name.resource())
        .fetch_all(&self.pool)
        .await?;

        Ok(sites)
    }

    async fn get_site(&self, site_name: &models::SiteName) -> Result<Option<models::Site>> {
        let site = sqlx::query_as(
            r"
            SELECT 'teams/' || t.name || '/sites/' || s.name AS name, s.created_time
            FROM sites s
            JOIN teams t
                ON t.id = s.team_id
            WHERE t.name = $1 AND s.name = $2;
            ",
        )
        .bind(site_name.parent().resource())
        .bind(site_name.resource())
        .fetch_optional(&self.pool)
        .await?;

        Ok(site)
    }

    async fn create_site(&self, site: &models::CreateSite) -> Result<models::Site> {
        let site = site.create()?;
        let site_id = Uuid::new_v4();

        let query_result = sqlx::query(
            r"
            INSERT INTO sites (id, created_time, name, team_id)
            SELECT $1, $2, $3, t.id
            FROM teams t
            WHERE t.name = $4;
            ",
        )
        .bind(&site_id)
        .bind(&site.created_time)
        .bind(site.name.resource())
        .bind(site.name.parent().resource())
        .execute(&self.pool)
        .await?;

        ensure!(query_result.rows_affected() == 1, "site parent not found");

        Ok(site)
    }

    async fn delete_site(&self, site_name: &models::SiteName) -> Result<()> {
        todo!();
    }

    async fn list_site_sources(
        &self,
        site_name: &models::SiteName,
    ) -> Result<Vec<models::SiteSource>> {
        let site_sources = sqlx::query_as(
            r"
            SELECT 'teams/' || t.name || '/sites/' || s.name || '/sources/' || ss.name AS name,
                    ss.created_time, ss.hook_token, ss.last_refresh_time,
                    ss_git.repository_url AS git_repository_url,
                    ss_git.id_ed25519_secret_key AS git_id_ed25519_secret_key
            FROM site_sources ss
            LEFT JOIN site_source_git ss_git
                ON ss_git.site_source_id = ss.id
            JOIN sites s
                ON s.id = ss.site_id
            JOIN teams t
                ON t.id = s.team_id
            WHERE t.name = $1 AND s.name = $2;
            ",
        )
        .bind(site_name.parent().resource())
        .bind(site_name.resource())
        .fetch_all(&self.pool)
        .await?;

        Ok(site_sources)
    }

    async fn get_site_source(
        &self,
        site_source_name: &models::SiteSourceName,
    ) -> Result<Option<models::SiteSource>> {
        let site_source = sqlx::query_as(
            r"
            SELECT 'teams/' || t.name || '/sites/' || s.name || '/sources/' || ss.name AS name,
                    ss.created_time, ss.hook_token, ss.last_refresh_time,
                    ss_git.repository_url AS git_repository_url,
                    ss_git.id_ed25519_secret_key AS git_id_ed25519_secret_key
            FROM site_sources ss
            LEFT JOIN site_source_git ss_git
                ON ss_git.site_source_id = ss.id
            JOIN sites s
                ON s.id = ss.site_id
            JOIN teams t
                ON t.id = s.team_id
            WHERE t.name = $1 AND s.name = $2 AND ss.name = $3;
            ",
        )
        .bind(site_source_name.parent().parent().resource())
        .bind(site_source_name.parent().resource())
        .bind(site_source_name.resource())
        .fetch_optional(&self.pool)
        .await?;

        Ok(site_source)
    }

    async fn create_site_source(
        &self,
        site_source: &models::CreateSiteSource,
    ) -> Result<models::SiteSource> {
        let site_source = site_source.create()?;
        let site_source_id = Uuid::new_v4();

        let mut tx = self.pool.begin().await?;

        let query_result = sqlx::query(
            r"
            INSERT INTO site_sources (id, created_time, name, site_id, hook_token)
            SELECT $1, $2, $3, s.id, $4
            FROM sites s
            JOIN teams t
                ON t.id = s.team_id
            WHERE t.name = $5 AND s.name = $6;
            ",
        )
        .bind(&site_source_id)
        .bind(&site_source.created_time)
        .bind(site_source.name.resource())
        .bind(&site_source.hook_token)
        .bind(site_source.name.parent().parent().resource())
        .bind(site_source.name.parent().resource())
        .execute(&mut tx)
        .await?;

        ensure!(
            query_result.rows_affected() == 1,
            "site source parent not found"
        );

        match site_source.kind {
            Some(models::SiteSourceKind::GitSource(ref git_source)) => {
                sqlx::query(
                    r"
                    INSERT INTO site_source_git (site_source_id, repository_url, id_ed25519_secret_key)
                    VALUES ($1, $2, $3);
                    ",
                )
                .bind(&site_source_id)
                .bind(&git_source.repository_url.as_str())
                .bind(&git_source.id_ed25519.secret_key_to_slice())
                .execute(&mut tx)
                .await?;
            }
            None => (),
        }

        tx.commit().await?;

        Ok(site_source)
    }
}

#[async_trait::async_trait]
impl database::TreeRepository for PostgresDatabase {
    async fn list_trees(
        &self,
        site_source_name: &models::SiteSourceName,
    ) -> Result<Vec<models::Tree>> {
        let trees = sqlx::query_as(
            r"
            SELECT 'teams/' || t.name || '/sites/' || s.name || '/sources/' || ss.name || '/trees/' || tr.name AS name,
                    tr.created_time, tr.version
            FROM trees tr
            JOIN site_sources ss
                ON ss.id = tr.site_source_id
            JOIN sites s
                ON s.id = ss.site_id
            JOIN teams t
                ON t.id = s.team_id
            WHERE t.name = $1 AND s.name = $2 AND ss.name = $3;
            ",
        )
        .bind(site_source_name.parent().parent().resource())
        .bind(site_source_name.parent().resource())
        .bind(site_source_name.resource())
        .fetch_all(&self.pool)
        .await?;

        Ok(trees)
    }

    async fn get_tree(&self, tree_name: &models::TreeName) -> Result<Option<models::Tree>> {
        let tree = sqlx::query_as(
            r"
            SELECT 'teams/' || t.name || '/sites/' || s.name || '/sources/' || ss.name || '/trees/' || tr.name AS name,
                    tr.created_time, tr.version
            FROM trees tr
            JOIN site_sources ss
                ON ss.id = tr.site_source_id
            JOIN sites s
                ON s.id = ss.site_id
            JOIN teams t
                ON t.id = s.team_id
            WHERE t.name = $1 AND s.name = $2 AND ss.name = $3 AND tr.name = $4;
            ",
        )
        .bind(tree_name.parent().parent().parent().resource())
        .bind(tree_name.parent().parent().resource())
        .bind(tree_name.parent().resource())
        .fetch_optional(&self.pool)
        .await?;

        Ok(tree)
    }

    async fn create_tree_revision(
        &self,
        tree_revision: &models::CreateTreeRevision,
    ) -> Result<Option<models::TreeRevision>> {
        let mut tx = self.pool.begin().await?;

        let tree = models::CreateTree {
            resource_id: tree_revision.parent.resource(),
            parent: tree_revision.parent.parent(),
        };
        let tree = tree.create().context("creating tree")?;

        let query_result = sqlx::query(
            r"
            INSERT INTO trees (id, created_time, name, site_source_id, version)
            SELECT $1, $2, $3, ss.id, 1
            FROM site_sources ss
            JOIN sites s
                ON s.id = ss.site_id
            JOIN teams t
                ON t.id = s.team_id
            WHERE t.name = $4 AND s.name = $5 AND ss.name = $6
            ON CONFLICT (site_source_id, name) DO UPDATE SET version = trees.version + 1;
            ",
        )
        .bind(Uuid::new_v4())
        .bind(tree.created_time)
        .bind(tree.name.resource())
        .bind(tree.name.parent().parent().parent().resource())
        .bind(tree.name.parent().parent().resource())
        .bind(tree.name.parent().resource())
        .execute(&mut tx)
        .await
        .context("inserting tree")?;

        ensure!(
            query_result.rows_affected() == 1,
            "tree parent not found ({})",
            tree.name.parent().name(),
        );

        let tree_revision = tree_revision.create().context("creating tree revision")?;

        let name: Option<String> = sqlx::query_scalar(
            r"
            INSERT INTO tree_revisions (tree_id, version, created_time, name, status)
            SELECT tr.id, tr.version, $1, $2, $3
            FROM trees tr
            JOIN site_sources ss
                ON ss.id = tr.site_source_id
            JOIN sites s
                ON s.id = ss.site_id
            JOIN teams t
                ON t.id = s.team_id
            WHERE t.name = $4 AND s.name = $5 AND ss.name = $6 AND tr.name = $7
            ON CONFLICT (tree_id, name) DO NOTHING
            RETURNING name;
            ",
        )
        .bind(tree_revision.created_time)
        .bind(tree_revision.name.resource())
        .bind(tree_revision.status)
        .bind(
            tree_revision
                .name
                .parent()
                .parent()
                .parent()
                .parent()
                .resource(),
        )
        .bind(tree_revision.name.parent().parent().parent().resource())
        .bind(tree_revision.name.parent().parent().resource())
        .bind(tree_revision.name.parent().resource())
        .fetch_optional(&mut tx)
        .await
        .context("inserting tree revision")?;

        // TODO: handle version conflicts on commit.
        tx.commit().await?;

        if let Some(_name) = name {
            Ok(Some(tree_revision))
        } else {
            Ok(None)
        }
    }
}
