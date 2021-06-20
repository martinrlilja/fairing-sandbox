use anyhow::Result;
use clap::{crate_version, App, AppSettings, Arg, ArgMatches, SubCommand};
use tonic::{metadata::MetadataValue, transport::Channel, Request};

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
                            Arg::with_name("git")
                                .long("git")
                                .value_name("repository-url")
                                .takes_value(true)
                                .help("Creates a git source for the site."),
                        ),
                )
                .subcommand(
                    SubCommand::with_name("sources")
                        .about("Site source management")
                        .setting(AppSettings::SubcommandRequiredElseHelp)
                        .subcommand(
                            SubCommand::with_name("refresh")
                            .about("Refresh site source")
                            .arg(Arg::with_name("site-source")),
                        )
                ),
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

    let mut users_client =
        fairing_proto::users::v1beta1::users_client::UsersClient::connect("http://[::1]:8000")
            .await?;

    if let Some(_matches) = matches.subcommand_matches("create") {
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

        let response = users_client
            .create_user(fairing_proto::users::v1beta1::CreateUserRequest {
                resource_id: username,
                password,
            })
            .await?;

        println!("New user: {}", response.get_ref().name);
    }

    Ok(())
}

async fn command_teams(matches: &ArgMatches<'_>) -> Result<()> {
    use fairing_proto::teams::v1beta1::{
        teams_client::TeamsClient, CreateTeamRequest, ListTeamsRequest,
    };

    let channel = Channel::from_static("http://[::1]:8000").connect().await?;

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
        site_source, sites_client::SitesClient, CreateSiteRequest, CreateSiteSourceRequest,
        ListSitesRequest, RefreshSiteSourceRequest, SiteSource,
    };

    let channel = Channel::from_static("http://[::1]:8000").connect().await?;

    let mut sites_client = SitesClient::with_interceptor(channel, auth);

    if let Some(matches) = matches.subcommand_matches("create") {
        let resource_id = matches
            .value_of("resource-id")
            .expect("team resource id must be set");

        let parent = matches.value_of("team").expect("team name must be set");

        let response = sites_client
            .create_site(CreateSiteRequest {
                resource_id: resource_id.into(),
                parent: parent.into(),
            })
            .await?;

        println!("Created site: {}", response.get_ref().name);

        if let Some(repository_url) = matches.value_of("git") {
            let git_source = site_source::GitSource {
                repository_url: repository_url.to_owned(),
                ..Default::default()
            };

            let response = sites_client
                .create_site_source(CreateSiteSourceRequest {
                    parent: response.get_ref().name.clone(),
                    // TODO: consider using the domain name of the repository here.
                    resource_id: "git".to_owned(),
                    site_source: Some(SiteSource {
                        kind: Some(site_source::Kind::GitSource(git_source)),
                        ..Default::default()
                    }),
                })
                .await?;

            println!("Created site source: {}", response.get_ref().name);
            println!("Hook URL: {}", response.get_ref().hook_url);

            if let Some(site_source::Kind::GitSource(ref git_source)) = response.get_ref().kind {
                println!("id_ed25519.pub: {}", git_source.id_ed25519_pub.trim());
            }
        }
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
    } else if let Some(matches) = matches.subcommand_matches("sources") {
        if let Some(matches) = matches.subcommand_matches("refresh") {
            let name = matches
                .value_of("site-source")
                .expect("site source name must be set");

            let _response = sites_client
                .refresh_site_source(RefreshSiteSourceRequest { name: name.into() })
                .await?;

            println!("Refreshed site source");
        }
    }

    Ok(())
}

fn auth(mut req: Request<()>) -> Result<Request<()>, tonic::Status> {
    let user = std::env::var("FAIRING_USER");
    let password = std::env::var("FAIRING_PASSWORD");

    match (user, password) {
        (Ok(user), Ok(password)) => {
            let token = base64::encode_config(format!("{}:{}", user, password), base64::URL_SAFE);
            let token = format!("Basic {}", token);
            let token =
                MetadataValue::from_str(&token).expect("failed to create authorization token");

            req.metadata_mut().insert("authorization", token);
        }
        _ => println!("Missing FAIRING_USER or FAIRING_PASSWORD environment variables."),
    }

    Ok(req)
}
