use anyhow::{anyhow, Result};
use directories::ProjectDirs;
use std::path::PathBuf;

const QUALIFIER: &str = "io";
const ORGANIZATION: &str = "Fairing";
const APPLICATION: &str = "fairingctl";
const CONFIG_FILE: &str = "config.toml";

#[derive(Clone, Debug, Default, serde::Deserialize, serde::Serialize)]
pub struct Config {
    #[serde(default)]
    pub auth: ConfigAuth,
}

#[derive(Clone, Debug, Default, serde::Deserialize, serde::Serialize)]
pub struct ConfigAuth {
    pub user: Option<String>,
    pub password: Option<String>,
}

impl Config {
    pub async fn get_user(&self, user: Option<String>) -> Result<(String, String)> {
        match (user, self.auth.user.as_ref(), self.auth.password.as_ref()) {
            (Some(user), _, _) => resolve_password(user),
            (None, Some(user), Some(password)) => Ok((user.clone(), password.clone())),
            (None, Some(user), None) => resolve_password(user.clone()),
            (None, None, _) => Err(anyhow!("no user set in config or in the command line")),
        }
    }

    pub async fn save_user(&mut self, user: &str, password: &str) -> Result<&mut Config> {
        use keyring::{Keyring, KeyringError};

        self.auth.user = Some(user.to_owned());

        let keyring = Keyring::new(APPLICATION, user);
        match keyring.set_password(password) {
            Ok(()) => (),
            Err(KeyringError::NoBackendFound) => {
                tracing::warn!("no keyring found, saving password config file");
                self.auth.password = Some(password.to_owned());
            }
            Err(err) => Err(anyhow!("keyring error: {:?}", err))?,
        }

        self.save().await.map(|_| self)
    }

    pub async fn save(&self) -> Result<()> {
        if let Some(config_file_path) = get_config_file_path() {
            let buffer = toml::to_vec(&self)?;

            let dir = config_file_path.parent().unwrap();
            tokio::fs::create_dir(dir).await?;

            tokio::fs::write(config_file_path, &buffer).await?;
            Ok(())
        } else {
            Err(anyhow!(
                "there is no default location for the config file on this platform"
            ))
        }
    }
}

fn get_config_file_path() -> Option<PathBuf> {
    ProjectDirs::from(QUALIFIER, ORGANIZATION, APPLICATION)
        .map(|project_dirs| project_dirs.config_dir().join(CONFIG_FILE))
}

pub async fn read_default() -> Result<Config> {
    use std::io::ErrorKind;

    if let Some(config_file_path) = get_config_file_path() {
        match tokio::fs::read(config_file_path).await {
            Ok(data) => {
                let config = toml::from_slice(&data)?;
                Ok(config)
            }
            Err(err) if err.kind() == ErrorKind::NotFound => Ok(Config::default()),
            Err(err) => Err(err)?,
        }
    } else {
        tracing::warn!("there is no default location for the config file on this platform");
        tracing::info!("using the default config instead");
        Ok(Config::default())
    }
}

fn resolve_password(user: String) -> Result<(String, String)> {
    use keyring::{Keyring, KeyringError};

    let keyring = Keyring::new(APPLICATION, &user);

    match keyring.get_password() {
        Ok(password) => Ok((user, password)),
        Err(KeyringError::NoPasswordFound) => {
            let password = std::env::var("FAIRING_PASSWORD")?;
            Ok((user, password))
        }
        Err(err) => Err(anyhow!("keyring error: {:?}", err)),
    }
}
