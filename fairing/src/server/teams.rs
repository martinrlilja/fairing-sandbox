use tonic::{Request, Response, Status};

use fairing_core::{
    backends::Database,
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
}

impl TeamsService {
    pub fn new(database: &Database) -> TeamsService {
        TeamsService {
            database: database.clone(),
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

        let team = self
            .database
            .get_team(&user_name, &team_name)
            .await
            .map_err(|err| {
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

        let team = models::CreateTeam {
            resource_id: &request.get_ref().resource_id,
            user_name,
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
        request: Request<DeleteTeamRequest>,
    ) -> Result<Response<DeleteTeamResponse>, Status> {
        todo!();
    }

    #[tracing::instrument]
    async fn list_team_members(
        &self,
        request: Request<ListTeamMembersRequest>,
    ) -> Result<Response<ListTeamMembersResponse>, Status> {
        todo!();
    }

    #[tracing::instrument]
    async fn create_team_member(
        &self,
        request: Request<CreateTeamMemberRequest>,
    ) -> Result<Response<TeamMember>, Status> {
        todo!();
    }

    #[tracing::instrument]
    async fn delete_team_member(
        &self,
        request: Request<DeleteTeamMemberRequest>,
    ) -> Result<Response<DeleteTeamMemberResponse>, Status> {
        todo!();
    }
}
