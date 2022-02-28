use anyhow::Result;
use clap::{App, AppSettings, Arg, SubCommand};
use fairing_acme::AcmeBackend;
use fairing_core::services::{BuildServiceBuilder, Storage};
use tokio::task;
use tracing_subscriber::prelude::*;

mod backends;
mod dns;
mod server;

#[tokio::main]
async fn main() -> Result<()> {
    let matches = App::new("fairing")
        .about("WebAssembly powered static sites.")
        .setting(AppSettings::SubcommandRequiredElseHelp)
        .subcommand(SubCommand::with_name("server").about("Start the server"))
        .subcommand(
            SubCommand::with_name("acme")
                .about("Manage ACME accounts")
                .subcommand(
                    SubCommand::with_name("create")
                        .about("Create a new account")
                        .arg(
                            Arg::new("mail-contact")
                                .long("mail-contact")
                                .multiple_occurrences(true)
                                .takes_value(true)
                                .required(true),
                        )
                        .arg(Arg::new("accept-terms-of-service").long("accept-terms-of-service")),
                ),
        )
        .get_matches();

    tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                .and_then(tracing_subscriber::EnvFilter::from_default_env()),
        )
        .with(console_subscriber::spawn())
        .init();

    if let Some(_matches) = matches.subcommand_matches("server") {
        let database =
            backends::PostgresDatabase::connect("psql://postgres:password@localhost:5432/postgres")
                .await?;
        database.migrate().await?;

        let file_storage = backends::LocalFileStorage::open(".data").await?;

        let remote_source = backends::GenericRemoteSource::new();

        let storage = Storage::new(file_storage.clone(), database.file_metadata());

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
                tracing::error!("build service: {:?}", err);
            }
        });

        tracing::info!("starting server");

        let dns_addr = "0.0.0.0:8053".parse().unwrap();

        tracing::info!("dns listening on {}", dns_addr);

        let dns_server = dns::serve(database.database(), dns_addr);

        tokio::spawn(async move {
            let res = dns_server.await;
            if let Err(err) = res {
                tracing::error!("dns: {:?}", err);
            }
        });

        let http_addr = "[::1]:8080".parse().unwrap();
        let https_addr = "[::1]:8443".parse().unwrap();

        tracing::info!("http listening on {}", http_addr,);

        tracing::info!("https listening on {}", https_addr);

        server::serve(
            database.database(),
            database.file_metadata(),
            file_storage,
            http_addr,
            https_addr,
        )
        .await?;
    } else if let Some(matches) = matches.subcommand_matches("acme") {
        if let Some(matches) = matches.subcommand_matches("create") {
            let contact = matches
                .values_of("mail-contact")
                .expect("there should be at least one mail contact")
                .map(|mail| format!("mailto:{}", mail))
                .collect::<Vec<_>>();

            let mut backend = fairing_acme::ReqwestAcmeBackend::connect(
                //"https://acme-staging-v02.api.letsencrypt.org/directory",
                "https://0.0.0.0:14000/dir",
            )
            .await?;

            let terms_of_service_agreed = matches.is_present("accept-terms-of-service");

            if !terms_of_service_agreed {
                let meta = backend.meta();

                println!(
                    "Rerun this command with --accept-terms-of-service to accept the terms of service below."
                );
                println!("{}", meta.termsOfService);
            } else {
                let (key, account_id, account) = fairing_acme::new_account(
                    &mut backend,
                    fairing_acme::NewAccount {
                        terms_of_service_agreed,
                        contact,
                    },
                )
                .await?;

                println!("{:#?}", account);

                let encoded_key =
                    base64::encode_config(&*key.to_sec1_der().unwrap(), base64::URL_SAFE_NO_PAD);
                println!("[acme]");
                println!("private_key = \"{}\"", encoded_key);

                let order = backend
                    .new_order(
                        &key,
                        &account_id,
                        fairing_acme::NewOrder {
                            identifiers: vec![fairing_acme::Identifier {
                                type_: fairing_acme::IdentifierType::Dns,
                                value: "example.com".into(),
                            }],
                        },
                    )
                    .await?;

                println!("{:#?}", order);

                let authorizations = backend
                    .get_authorizations(&key, &account_id, &order)
                    .await?;

                println!("{:#?}", authorizations);
            }
        }
    }

    Ok(())
}
