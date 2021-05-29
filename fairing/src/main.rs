use anyhow::Result;
use clap::{crate_version, App, AppSettings, Arg, SubCommand};

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
        .with_max_level(tracing::Level::TRACE)
        .finish();

    tracing::subscriber::set_global_default(subscriber).expect("setting default subscriber failed");

    if let Some(_matches) = matches.subcommand_matches("server") {
        let database =
            backends::PostgresDatabase::connect("psql://postgres:password@localhost:5432/postgres")
                .await?;

        database.migrate().await?;

        let database = database.into_database();

        tracing::info!("starting server");

        let addr = "[::1]:8000".parse().unwrap();

        tracing::info!("server listening on {}", addr);

        server::api_server(&database, addr).await?;
    }

    Ok(())
}
