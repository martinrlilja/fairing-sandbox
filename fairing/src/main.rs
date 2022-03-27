use anyhow::Result;
use fairing_core::{
    models::{self, prelude::*},
    services::{AcmeService, BuildServiceBuilder, Storage},
};
use std::net::SocketAddr;
use tokio::task;
use tracing_subscriber::prelude::*;

mod backends;
mod dns;
mod server;

#[derive(clap::Parser, Debug)]
#[clap(author, version, about, long_about = None)]
struct Args {
    /// Optional config file.
    #[clap(short, long)]
    config: Option<String>,

    #[clap(subcommand)]
    command: Commands,
}

#[derive(clap::Subcommand, Debug)]
enum Commands {
    /// Start the server.
    Server,
    Acme {
        #[clap(subcommand)]
        command: AcmeCommands,
    },
}

#[derive(clap::Subcommand, Debug)]
enum AcmeCommands {
    Create {
        #[clap(long)]
        mail_contact: Vec<String>,

        #[clap(long)]
        accept_terms_of_service: bool,
    },
}

#[derive(Debug, serde::Deserialize)]
struct Config {
    database: DatabaseConfig,
    acme: AcmeConfig,
    http: HttpConfig,
    https: HttpsConfig,
    api: ApiConfig,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "lowercase", tag = "type")]
enum DatabaseConfig {
    Postgres { url: String },
}

#[derive(Debug, serde::Deserialize)]
struct AcmeConfig {
    server: String,
    secret_key: Option<String>,
    secret_key_id: Option<String>,
    #[serde(default)]
    danger_accept_invalid_certs: bool,
    dns: AcmeDnsConfig,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "lowercase", tag = "type")]
enum AcmeDnsConfig {
    Server {
        udp_bind: Vec<SocketAddr>,
        tcp_bind: Vec<SocketAddr>,
        zone: String,
    },
}

#[derive(Debug, serde::Deserialize)]
struct HttpConfig {
    bind: Vec<SocketAddr>,
    #[serde(default = "default_true")]
    redirect_https: bool,
    #[serde(default)]
    redirect_https_port: Option<u16>,
}

#[derive(Debug, serde::Deserialize)]
struct HttpsConfig {
    bind: Vec<SocketAddr>,
}

#[derive(Debug, serde::Deserialize)]
struct ApiConfig {
    host: String,
}

fn default_true() -> bool {
    true
}

#[tokio::main]
async fn main() -> Result<()> {
    let args: Args = clap::Parser::parse();

    let config: Config = {
        let mut config = config::Config::builder();

        if let Some(config_file) = args.config {
            config = config.add_source(config::File::with_name(&config_file));
        }

        const ENV_MAP: &[(&str, &str)] = &[
            ("FAIRING_DATABASE_TYPE", "database.type"),
            ("FAIRING_DATABASE_URL", "database.url"),
            ("FAIRING_ACME_SERVER", "acme.server"),
            ("FAIRING_ACME_DNS_TYPE", "acme.dns.type"),
            ("FAIRING_ACME_DNS_ZONE", "acme.dns.zone"),
            ("FAIRING_API_HOST", "api.host"),
        ];

        for (env, key) in ENV_MAP.iter() {
            if let Ok(value) = std::env::var(env) {
                config = config.set_override(key, value)?;
            }
        }

        const ENV_MAP_LIST: &[(&str, &str)] = &[
            ("FAIRING_ACME_UDP_BIND", "acme.dns.udp_bind"),
            ("FAIRING_ACME_TCP_BIND", "acme.dns.tcp_bind"),
            ("FAIRING_HTTP_BIND", "http.bind"),
            ("FAIRING_HTTPS_BIND", "https.bind"),
        ];

        for (env, key) in ENV_MAP_LIST.iter() {
            if let Ok(value) = std::env::var(env) {
                let values = value.split(',').collect::<Vec<_>>();
                config = config.set_override(key, values)?;
            }
        }

        config.build()?.try_deserialize()?
    };

    tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                .and_then(tracing_subscriber::EnvFilter::from_default_env()),
        )
        .with(console_subscriber::spawn())
        .init();

    if let Commands::Server = args.command {
        let DatabaseConfig::Postgres { url: database_url } = config.database;
        let database = backends::PostgresDatabase::connect(&database_url).await?;
        database.migrate().await?;

        let file_storage = backends::LocalFileStorage::open(".data").await?;

        let remote_source = backends::GenericRemoteSource::new();

        let storage = Storage::new(file_storage.clone(), database.file_metadata());

        {
            // Setup system components.
            use rand::distributions::{Alphanumeric, DistString};

            let file_metadata = database.file_metadata();
            let database = database.database();

            let password = Alphanumeric.sample_string(&mut rand::thread_rng(), 32);

            let res = database
                .create_user(&models::CreateUser {
                    resource_id: "fairing-admin",
                    password: &password,
                })
                .await;

            match res {
                Ok(user) => tracing::info!("created admin user {}", user.name.name()),
                Err(err) => {
                    tracing::trace!("error creating admin user (probably safe to ignore): {err}")
                }
            }

            let file_keyspace = file_metadata
                .create_file_keyspace(&models::CreateFileKeyspace)
                .await?;

            let res = database
                .create_team(&models::CreateTeam {
                    resource_id: "fairing-system",
                    user_name: models::UserName::parse("users/fairing-admin")?,
                    file_keyspace_id: file_keyspace.id,
                })
                .await;

            match res {
                Ok(team) => tracing::info!("created system team {}", team.name.name()),
                Err(err) => {
                    tracing::trace!("error creating admin user (probably safe to ignore): {err}")
                }
            }

            let res = database
                .create_domain(&models::CreateDomain {
                    parent: models::TeamName::parse("teams/fairing-system")?,
                    resource_id: &config.api.host,
                })
                .await;

            match res {
                Ok(domain) => {
                    let AcmeDnsConfig::Server {
                        zone: ref dns_zone, ..
                    } = config.acme.dns;

                    tracing::info!("created api domain {}", domain.name.name());
                    tracing::info!(
                        "add the following CNAME _acme-challenge = {}.{dns_zone}",
                        domain.acme_label
                    );
                }
                Err(err) => {
                    tracing::trace!("error creating api domain (probably safe to ignore): {err}")
                }
            }
        }

        let build_service = BuildServiceBuilder::new().concurrent_builds(4).build(
            database.build_queue(),
            database.database(),
            database.file_metadata(),
            remote_source,
            storage.clone(),
        );

        task::spawn(async move {
            let res = build_service.run().await;
            if let Err(err) = res {
                tracing::error!("build service: {err:?}");
            }
        });

        tracing::info!("starting server");

        if let (Some(secret_key), Some(secret_key_id)) =
            (config.acme.secret_key, config.acme.secret_key_id)
        {
            let AcmeDnsConfig::Server {
                udp_bind: dns_udp_addr,
                tcp_bind: dns_tcp_addr,
                zone: dns_zone,
            } = config.acme.dns;

            let secret_key = fairing_acme::ES256SecretKey::parse_key(&secret_key)?;

            let dns_server = dns::serve(
                database.database(),
                dns_zone,
                dns_udp_addr,
                dns_tcp_addr,
                secret_key.public_key(),
            );

            tokio::spawn(async move {
                let res = dns_server.await;
                if let Err(err) = res {
                    tracing::error!("dns: {err:?}");
                }
            });

            let http_client = reqwest::Client::builder()
                .https_only(true)
                .danger_accept_invalid_certs(config.acme.danger_accept_invalid_certs)
                .timeout(std::time::Duration::from_secs(30))
                .build()?;

            let client = fairing_acme::AcmeClient::connect(http_client, &config.acme.server)
                .await?
                .with_account(secret_key, &secret_key_id)?;

            let acme_service = AcmeService::new(database.database(), client);

            tokio::spawn(async move {
                let res = acme_service.run().await;
                if let Err(err) = res {
                    tracing::error!("acme: {err:?}");
                }
            });
        } else {
            tracing::info!("not starting acme dns server because private_key is not set");
        }

        server::serve(
            database.database(),
            database.file_metadata(),
            file_storage,
            config.http.bind,
            config.http.redirect_https,
            config.http.redirect_https_port,
            config.https.bind,
            config.api.host,
        )
        .await?;
    } else if let Commands::Acme { command } = args.command {
        let AcmeCommands::Create {
            mail_contact,
            accept_terms_of_service,
        } = command;

        let contact = mail_contact
            .into_iter()
            .map(|mail| format!("mailto:{mail}"))
            .collect::<Vec<_>>();

        let http_client = reqwest::Client::builder()
            .https_only(true)
            .danger_accept_invalid_certs(config.acme.danger_accept_invalid_certs)
            .timeout(std::time::Duration::from_secs(30))
            .build()?;

        let client = fairing_acme::AcmeClient::connect(http_client, &config.acme.server).await?;

        match client.meta().terms_of_service {
            Some(ref terms_of_service) if !accept_terms_of_service => {
                println!(
                    "Rerun this command with --accept-terms-of-service to accept the terms of service below."
                );
                println!("{}", terms_of_service);
            }
            _ => {
                let client = client
                    .create_account(&fairing_acme::CreateAccount {
                        terms_of_service_agreed: accept_terms_of_service,
                        contact,
                    })
                    .await?;

                let secret_key = client.secret_key().to_string()?;
                let secret_key_id = client.secret_key_id();

                let server = config.acme.server;

                println!("Add the following to your configuration.");
                println!();

                println!("[acme]");
                println!("server = \"{server}\"");
                println!("secret_key = \"{secret_key}\"");
                println!("secret_key_id = \"{secret_key_id}\"");
            }
        }
    }

    Ok(())
}
