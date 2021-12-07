use anyhow::Result;
use clap::{crate_version, App, AppSettings, SubCommand};
use fairing_core::services::{BuildServiceBuilder, Storage};
use tokio::task;

mod backends;
mod server;

#[tokio::main]
async fn main() -> Result<()> {
    let matches = App::new("fairing")
        .version(crate_version!())
        .about("WebAssembly powered static sites.")
        .setting(AppSettings::SubcommandRequiredElseHelp)
        .subcommand(SubCommand::with_name("server").about("Start the server"))
        .get_matches();

    let subscriber = tracing_subscriber::FmtSubscriber::builder()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .finish();

    tracing::subscriber::set_global_default(subscriber).expect("setting default subscriber failed");

    if let Some(_matches) = matches.subcommand_matches("server") {
        let database =
            backends::PostgresDatabase::connect("psql://postgres:password@localhost:5432/postgres")
                .await?;
        database.migrate().await?;

        let file_storage = backends::LocalFileStorage::open(".data").await?;

        let remote_source = backends::GenericRemoteSource::new();

        let storage = Storage::new(file_storage, database.file_metadata());

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

        let addr = "[::1]:8000".parse().unwrap();

        tracing::info!("server listening on {}", addr);

        server::serve(database.database(), database.file_metadata(), addr).await?;
    }

    Ok(())
}
