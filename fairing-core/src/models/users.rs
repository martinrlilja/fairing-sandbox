use anyhow::{anyhow, Result};
use chrono::{DateTime, Utc};

use crate::models::resource_name::{validators, ResourceName, ResourceNameInner};

crate::impl_resource_name! {
    pub struct UserName<'n>;
}

impl<'n> ResourceName<'n> for UserName<'n> {
    const COLLECTION: &'static str = "users";

    type Validator = validators::UnicodeIdentifierValidator;
}

#[derive(Debug, sqlx::FromRow)]
pub struct User {
    pub name: UserName<'static>,
    pub created_time: DateTime<Utc>,
}

pub struct CreateUser<'a> {
    pub resource_id: &'a str,
    pub password: &'a str,
}

impl<'a> CreateUser<'a> {
    pub fn create(&self) -> Result<(User, Password)> {
        let resource_id = validators::UnicodeIdentifierValidator::normalize(self.resource_id);
        let name = UserName::parse(format!("users/{}", resource_id))?;

        let user = User {
            name,
            created_time: Utc::now(),
        };

        let password = Password::new(self.password);

        Ok((user, password))
    }
}

pub struct Password(String);

impl Password {
    pub fn new(password: &str) -> Password {
        use unicode_normalization::UnicodeNormalization;

        let password = password.nfc().collect::<String>();

        Password(password)
    }

    pub fn hash(self) -> String {
        use argon2::{
            password_hash::{PasswordHasher, SaltString},
            Argon2,
        };

        let salt = SaltString::generate(&mut rand::rngs::OsRng);
        let argon2 = Argon2::default();

        let password_hash = argon2
            .hash_password_simple(self.0.as_bytes(), salt.as_ref())
            .unwrap()
            .to_string();

        password_hash
    }

    pub fn verify(self, hash: &str) -> Result<()> {
        use argon2::{Argon2, PasswordHash, PasswordVerifier};

        let argon2 = Argon2::default();

        let hash =
            PasswordHash::new(hash).map_err(|err| anyhow!("invalid password hash: {:?}", err))?;

        argon2
            .verify_password(self.0.as_bytes(), &hash)
            .map_err(|err| anyhow!("invalid password: {:?}", err))?;

        Ok(())
    }
}

pub struct UpdatePassword {
    pub password: Option<Password>,
}
