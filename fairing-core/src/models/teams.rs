use anyhow::Result;
use chrono::{DateTime, Utc};

use crate::models::{
    self,
    resource_name::{validators, ParentedResourceName, ResourceName, ResourceNameInner},
};

crate::impl_resource_name! {
    pub struct TeamName<'n>;
    pub struct TeamMemberName<'n>;
}

impl<'n> ResourceName<'n> for TeamName<'n> {
    const COLLECTION: &'static str = "teams";

    type Validator = validators::UnicodeIdentifierValidator;
}

impl<'n> ParentedResourceName<'n> for TeamMemberName<'n> {
    const COLLECTION: &'static str = "members";

    type Validator = validators::UnicodeIdentifierValidator;

    type Parent = crate::models::TeamName<'static>;
}

#[derive(sqlx::FromRow)]
pub struct Team {
    pub name: TeamName<'static>,
    pub created_time: DateTime<Utc>,
}

#[derive(Debug)]
pub struct CreateTeam<'a> {
    pub resource_id: &'a str,
    pub user_name: models::UserName<'static>,
}

impl<'a> CreateTeam<'a> {
    pub fn create(&self) -> Result<(Team, TeamMember)> {
        let name = TeamName::parse(format!("teams/{}", self.resource_id))?;

        let team = Team {
            name,
            created_time: Utc::now(),
        };

        let team_member = CreateTeamMember {
            team_name: team.name.clone(),
            user_name: self.user_name.clone(),
        };

        Ok((team, team_member.create()))
    }
}

#[derive(Debug, sqlx::FromRow)]
pub struct TeamMember {
    pub name: TeamMemberName<'static>,
    pub created_time: DateTime<Utc>,
}

#[derive(Debug)]
pub struct CreateTeamMember {
    pub team_name: TeamName<'static>,
    pub user_name: models::UserName<'static>,
}

impl CreateTeamMember {
    pub fn create(&self) -> TeamMember {
        let name = TeamMemberName::parse(format!(
            "{}/members/{}",
            self.team_name.name(),
            self.user_name.resource()
        ))
        .unwrap();

        TeamMember {
            name,
            created_time: Utc::now(),
        }
    }
}
