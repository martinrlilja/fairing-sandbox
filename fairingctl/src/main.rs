use anyhow::{anyhow, Result};
use clap::{crate_version, App, AppSettings, Arg, ArgMatches, SubCommand};
use tonic::{
    metadata::{AsciiMetadataValue, MetadataValue},
    transport::Channel,
    Request,
};

mod config;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let matches = App::new("fairingctl")
        .version(crate_version!())
        .about("CLI for WebAssembly powered static sites.")
        .setting(AppSettings::SubcommandRequiredElseHelp)
        .subcommand(
            SubCommand::with_name("users")
                .about("Manage users")
                .setting(AppSettings::SubcommandRequiredElseHelp)
                .subcommand(SubCommand::with_name("create").about("Create a new user"))
                .subcommand(SubCommand::with_name("login").about("Login to a server")),
        )
        .subcommand(
            SubCommand::with_name("teams")
                .about("Team management")
                .setting(AppSettings::SubcommandRequiredElseHelp)
                .subcommand(SubCommand::with_name("list").about("List teams"))
                .subcommand(
                    SubCommand::with_name("create")
                        .about("Create a new team")
                        .arg(Arg::with_name("resource-id")),
                ),
        )
        .subcommand(
            SubCommand::with_name("sites")
                .about("Site management")
                .setting(AppSettings::SubcommandRequiredElseHelp)
                .subcommand(
                    SubCommand::with_name("list")
                        .about("List sites")
                        .arg(Arg::with_name("team")),
                )
                .subcommand(
                    SubCommand::with_name("create")
                        .about("Create a new site")
                        .arg(
                            Arg::with_name("team")
                                .help("Team name to create this site under. Format: teams/<team>"),
                        )
                        .arg(
                            Arg::with_name("resource-id")
                                .help("Resource ID of this site. RFC1035-like names are accepted. Must start with a letter, no double dashes."),
                        )
                        .arg(
                            Arg::with_name("source")
                                .long("source")
                                .value_name("source-name")
                                .takes_value(true)
                                .help("Use this source as the site's base source."),
                        )
                        .arg(
                            Arg::with_name("git")
                                .long("git")
                                .value_name("repository-url")
                                .takes_value(true)
                                .help("Creates a git source for the site's base source."),
                        ),
                ),
        )
        .subcommand(
            SubCommand::with_name("sources")
            .about("Source management")
            .setting(AppSettings::SubcommandRequiredElseHelp)
            .subcommand(
                SubCommand::with_name("refresh")
                .about("Refresh source")
                .arg(Arg::with_name("source")),
            )
        )
        .get_matches();

    let subscriber = tracing_subscriber::FmtSubscriber::builder()
        .with_max_level(tracing::Level::WARN)
        .finish();

    tracing::subscriber::set_global_default(subscriber).expect("setting default subscriber failed");

    if let Some(matches) = matches.subcommand_matches("users") {
        command_users(matches).await?;
    } else if let Some(matches) = matches.subcommand_matches("teams") {
        command_teams(&matches).await?;
    } else if let Some(matches) = matches.subcommand_matches("sites") {
        command_sites(&matches).await?;
    } else if let Some(matches) = matches.subcommand_matches("sources") {
        command_sources(&matches).await?;
    }

    Ok(())
}

async fn command_users(matches: &ArgMatches<'_>) -> Result<()> {
    use std::io::{stdin, stdout, Write};
    use termion::input::TermRead;

    let stdout = stdout();
    let mut stdout = stdout.lock();
    let stdin = stdin();
    let mut stdin = stdin.lock();

    let mut users_client = fairing_proto::users::v1beta1::users_client::UsersClient::connect(
        "http://api.localhost:8000",
    )
    .await?;

    if let Some(_matches) = matches.subcommand_matches("create") {
        stdout.write_all(b"Username: ")?;
        stdout.flush()?;

        let username = stdin.read_line()?;
        let username = match username {
            Some(username) => username,
            None => return Ok(()),
        };

        stdout.write_all(b"Password: ").unwrap();
        stdout.flush().unwrap();

        let password = stdin.read_passwd(&mut stdout)?;
        let password = match password {
            Some(password) => password,
            None => return Ok(()),
        };

        println!();

        let response = users_client
            .create_user(fairing_proto::users::v1beta1::CreateUserRequest {
                resource_id: username.clone(),
                password: password.clone(),
            })
            .await?;

        println!("New user: {}", response.get_ref().name);

        stdout.write_all(b"Do you want to save this user and use it as default? (Y/n) ")?;
        stdout.flush()?;

        let save_user = stdin.read_line()?;
        if let Some(save_user) = save_user {
            if save_user.is_empty()
                || save_user.eq_ignore_ascii_case("y")
                || save_user.eq_ignore_ascii_case("yes")
            {
                config::read_default()
                    .await?
                    .save_user(&username, &password)
                    .await?;
            }
        }
    } else if let Some(_matches) = matches.subcommand_matches("login") {
        stdout.write_all(b"Username: ").unwrap();
        stdout.flush().unwrap();

        let username = stdin.read_line()?;
        let username = match username {
            Some(username) => username,
            None => return Ok(()),
        };

        stdout.write_all(b"Password: ").unwrap();
        stdout.flush().unwrap();

        let password = stdin.read_passwd(&mut stdout)?;
        let password = match password {
            Some(password) => password,
            None => return Ok(()),
        };

        println!();

        config::read_default()
            .await?
            .save_user(&username, &password)
            .await?;
    }

    Ok(())
}

async fn command_teams(matches: &ArgMatches<'_>) -> Result<()> {
    use fairing_proto::teams::v1beta1::{
        teams_client::TeamsClient, CreateTeamRequest, ListTeamsRequest,
    };

    let channel = Channel::from_static("http://api.localhost:8000")
        .connect()
        .await?;
    let auth = ConfigAuth::read().await?;

    let mut teams_client = TeamsClient::with_interceptor(channel, auth);

    if let Some(matches) = matches.subcommand_matches("create") {
        let resource_id = matches
            .value_of("resource-id")
            .expect("team resource id must be set");

        let response = teams_client
            .create_team(CreateTeamRequest {
                resource_id: resource_id.into(),
            })
            .await?;

        println!("New team: {}", response.get_ref().name);
    } else if let Some(_matches) = matches.subcommand_matches("list") {
        let response = teams_client.list_teams(ListTeamsRequest {}).await?;

        if response.get_ref().resources.is_empty() {
            println!("No teams found");
        } else {
            println!("Teams:");
        }

        for team in response.get_ref().resources.iter() {
            println!("{}", team.name);
        }
    }

    Ok(())
}

async fn command_sites(matches: &ArgMatches<'_>) -> Result<()> {
    use fairing_proto::sites::v1beta1::{
        sites_client::SitesClient, CreateSiteRequest, ListSitesRequest,
    };
    use fairing_proto::sources::v1beta1::{
        source, sources_client::SourcesClient, CreateSourceRequest, Source,
    };

    let channel = Channel::from_static("http://api.localhost:8000")
        .connect()
        .await?;
    let auth = ConfigAuth::read().await?;

    let mut sites_client = SitesClient::with_interceptor(channel.clone(), auth.clone());
    let mut sources_clinent = SourcesClient::with_interceptor(channel, auth);

    if let Some(matches) = matches.subcommand_matches("create") {
        let parent = matches.value_of("team").expect("team name must be set");

        let resource_id = matches
            .value_of("resource-id")
            .expect("site resource id must be set");

        let base_source = if let Some(source_name) = matches.value_of("source") {
            source_name.to_owned()
        } else if let Some(repository_url) = matches.value_of("git") {
            let git_source = source::GitSource {
                repository_url: repository_url.to_owned(),
                ..Default::default()
            };

            let response = sources_clinent
                .create_source(CreateSourceRequest {
                    parent: parent.into(),
                    resource_id: format!("{}-git", resource_id),
                    source: Some(Source {
                        kind: Some(source::Kind::GitSource(git_source)),
                        ..Default::default()
                    }),
                })
                .await?;

            println!("Created site source: {}", response.get_ref().name);
            println!("Hook URL: {}", response.get_ref().hook_url);

            if let Some(source::Kind::GitSource(ref git_source)) = response.get_ref().kind {
                println!("id_ed25519.pub: {}", git_source.id_ed25519_pub.trim());
            }

            response.get_ref().name.clone()
        } else {
            return Err(anyhow!(
                "A base source must be set, use --source or --git, for help use --help."
            ));
        };

        let response = sites_client
            .create_site(CreateSiteRequest {
                resource_id: resource_id.into(),
                parent: parent.into(),
                base_source,
            })
            .await?;

        println!("Created site: {}", response.get_ref().name);
    } else if let Some(matches) = matches.subcommand_matches("list") {
        let parent = matches.value_of("team").expect("team name must be set");

        let response = sites_client
            .list_sites(ListSitesRequest {
                parent: parent.into(),
            })
            .await?;

        if response.get_ref().resources.is_empty() {
            println!("No sites found");
        } else {
            println!("Sites:");
        }

        for site in response.get_ref().resources.iter() {
            println!("{}", site.name);
        }
    }

    Ok(())
}

async fn command_sources(matches: &ArgMatches<'_>) -> Result<()> {
    use fairing_proto::sources::v1beta1::{sources_client::SourcesClient, RefreshSourceRequest};

    let channel = Channel::from_static("http://api.localhost:8000")
        .connect()
        .await?;
    let auth = ConfigAuth::read().await?;

    let mut sources_client = SourcesClient::with_interceptor(channel, auth);

    if let Some(matches) = matches.subcommand_matches("refresh") {
        let name = matches.value_of("source").expect("source name must be set");

        let _response = sources_client
            .refresh_source(RefreshSourceRequest { name: name.into() })
            .await?;

        println!("Refreshed source");
    }

    Ok(())
}

#[derive(Clone)]
struct ConfigAuth {
    token: AsciiMetadataValue,
}

impl ConfigAuth {
    pub async fn read() -> Result<ConfigAuth> {
        let config = config::read_default().await?;
        let (user, password) = config.get_user(None).await.unwrap();

        let token = base64::encode_config(format!("{}:{}", user, password), base64::URL_SAFE);
        let token = format!("Basic {}", token);
        let token = MetadataValue::from_str(&token).expect("failed to create authorization token");

        Ok(ConfigAuth { token })
    }
}

impl tonic::service::Interceptor for ConfigAuth {
    fn call(&mut self, mut request: Request<()>) -> Result<Request<()>, tonic::Status> {
        request
            .metadata_mut()
            .insert("authorization", self.token.clone());
        Ok(request)
    }
}
