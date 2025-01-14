use tonic::{Request, Response, Status};

use fairing_core::{
    backends::{Database, FileMetadata},
    models::{self, prelude::*},
};
use fairing_proto::teams::v1beta1::{
    teams_server::Teams, CreateTeamMemberRequest, CreateTeamRequest, DeleteTeamMemberRequest,
    DeleteTeamMemberResponse, DeleteTeamRequest, DeleteTeamResponse, GetTeamRequest,
    ListTeamMembersRequest, ListTeamMembersResponse, ListTeamsRequest, ListTeamsResponse, Team,
    TeamMember,
};

#[derive(Debug)]
pub struct TeamsService {
    database: Database,
    file_metadata: FileMetadata,
}

impl TeamsService {
    pub fn new(database: &Database, file_metadata: &FileMetadata) -> TeamsService {
        TeamsService {
            database: database.clone(),
            file_metadata: file_metadata.clone(),
        }
    }
}

#[tonic::async_trait]
impl Teams for TeamsService {
    #[tracing::instrument]
    async fn list_teams(
        &self,
        request: Request<ListTeamsRequest>,
    ) -> Result<Response<ListTeamsResponse>, Status> {
        let user_name = super::auth(&self.database, &request).await?;

        let teams = self.database.list_teams(&user_name).await.map_err(|err| {
            tracing::error!("error: {:?}", err);
            Status::internal("internal error")
        })?;

        let teams = teams
            .into_iter()
            .map(|team| Team {
                name: team.name.name().into(),
            })
            .collect();

        let reply = ListTeamsResponse { resources: teams };

        Ok(Response::new(reply))
    }

    #[tracing::instrument]
    async fn get_team(&self, request: Request<GetTeamRequest>) -> Result<Response<Team>, Status> {
        let user_name = super::auth(&self.database, &request).await?;

        let team_name = models::TeamName::parse(&request.get_ref().name)
            .map_err(|_err| Status::invalid_argument("invalid team name"))?;

        let team = self.database.get_team(&team_name).await.map_err(|err| {
            tracing::error!("error: {:?}", err);
            Status::internal("internal error")
        })?;

        if let Some(team) = team {
            let reply = Team {
                name: team.name.name().into(),
            };

            Ok(Response::new(reply))
        } else {
            Err(Status::not_found(
                "team does not exist or user is not a member",
            ))
        }
    }

    #[tracing::instrument]
    async fn create_team(
        &self,
        request: Request<CreateTeamRequest>,
    ) -> Result<Response<Team>, Status> {
        let user_name = super::auth(&self.database, &request).await?;

        let file_keyspace = models::CreateFileKeyspace;

        let file_keyspace = self
            .file_metadata
            .create_file_keyspace(&file_keyspace)
            .await
            .map_err(|err| {
                tracing::error!("error: {:?}", err);
                Status::internal("internal error")
            })?;

        let team = models::CreateTeam {
            resource_id: &request.get_ref().resource_id,
            user_name,
            file_keyspace_id: file_keyspace.id,
        };

        let team = self.database.create_team(&team).await.map_err(|err| {
            tracing::error!("error: {:?}", err);
            Status::internal("internal error")
        })?;

        let reply = Team {
            name: team.name.name().into(),
        };

        Ok(Response::new(reply))
    }

    #[tracing::instrument]
    async fn delete_team(
        &self,
        _request: Request<DeleteTeamRequest>,
    ) -> Result<Response<DeleteTeamResponse>, Status> {
        todo!();
    }

    #[tracing::instrument]
    async fn list_team_members(
        &self,
        _request: Request<ListTeamMembersRequest>,
    ) -> Result<Response<ListTeamMembersResponse>, Status> {
        todo!();
    }

    #[tracing::instrument]
    async fn create_team_member(
        &self,
        _request: Request<CreateTeamMemberRequest>,
    ) -> Result<Response<TeamMember>, Status> {
        todo!();
    }

    #[tracing::instrument]
    async fn delete_team_member(
        &self,
        _request: Request<DeleteTeamMemberRequest>,
    ) -> Result<Response<DeleteTeamMemberResponse>, Status> {
        todo!();
    }
}
