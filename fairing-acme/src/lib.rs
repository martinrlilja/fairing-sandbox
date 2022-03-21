use anyhow::{anyhow, ensure, Result};
use p256::{
    ecdsa::{signature::Signer, SigningKey},
    SecretKey,
};
use rand_core::OsRng;
use sha2::{Digest, Sha256};

#[derive(Clone, Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Directory {
    pub new_account: String,
    pub new_nonce: String,
    pub new_order: String,
    pub meta: DirectoryMeta,
}

#[derive(Clone, Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectoryMeta {
    pub terms_of_service: String,
    pub website: Option<String>,
    #[serde(default)]
    pub caa_identities: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct AccountId(pub String);

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NewAccount {
    pub terms_of_service_agreed: bool,
    pub contact: Vec<String>,
}

#[derive(Clone, Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    pub status: String,
    pub contact: Vec<String>,
    pub orders: String,
}

#[derive(Copy, Clone, Debug, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum AccountStatus {
    Valid,
    Deactivated,
    Revoked,
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NewOrder {
    pub identifiers: Vec<Identifier>,
}

#[derive(Clone, Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Order {
    pub status: OrderStatus,
    pub expires: String,
    pub authorizations: Vec<String>,
    pub finalize: String,
    #[serde(default)]
    pub certificate: Option<String>,
}

#[derive(Copy, Clone, Debug, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum OrderStatus {
    Pending,
    Ready,
    Processing,
    Valid,
    Invalid,
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Identifier {
    #[serde(rename = "type")]
    pub type_: IdentifierType,
    pub value: String,
}

#[derive(Copy, Clone, Debug, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum IdentifierType {
    Dns,
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FinalizeOrder {
    pub csr: String,
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Authorization {
    pub status: AuthorizationStatus,
    pub expires: String,
    pub identifier: Identifier,
    pub challenges: Vec<Challenge>,
}

#[derive(Copy, Clone, Debug, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum AuthorizationStatus {
    Pending,
    Valid,
    Invalid,
    Revoked,
    Deactivated,
    Expired,
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Challenge {
    #[serde(rename = "type")]
    pub type_: String,
    pub url: String,
    pub token: String,
}

#[derive(Copy, Clone, Debug, serde::Deserialize, serde::Serialize)]
pub enum ChallengeType {
    #[serde(rename = "dns-01")]
    Dns01,
    #[serde(rename = "http-01")]
    Http01,
    #[serde(rename = "tls-alpn-01")]
    TlsAlpn01,
}

#[derive(Clone, Debug)]
pub struct CreateResponse<T> {
    pub url: String,
    pub body: T,
}

#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Jose {
    pub protected: String,
    pub payload: String,
    pub signature: String,
}

pub type ES256Key = SecretKey;

impl Jose {
    pub fn sign<T>(
        key: &ES256Key,
        key_id: Option<&AccountId>,
        nonce: &str,
        url: &str,
        payload: T,
    ) -> Result<Jose>
    where
        T: serde::Serialize + for<'de> serde::Deserialize<'de>,
    {
        let payload = serde_json::to_vec(&payload)?;
        Self::sign_bytes(key, key_id, nonce, url, &payload)
    }

    pub fn sign_empty(
        key: &ES256Key,
        key_id: Option<&AccountId>,
        nonce: &str,
        url: &str,
    ) -> Result<Jose> {
        Self::sign_bytes(key, key_id, nonce, url, &[])
    }

    fn sign_bytes(
        key: &ES256Key,
        key_id: Option<&AccountId>,
        nonce: &str,
        url: &str,
        payload: &[u8],
    ) -> Result<Jose> {
        #[derive(Debug, serde::Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Protected<'a> {
            alg: &'a str,
            #[serde(skip_serializing_if = "Option::is_none")]
            jwk: Option<p256::elliptic_curve::JwkEcKey>,
            #[serde(skip_serializing_if = "Option::is_none")]
            kid: Option<&'a str>,
            nonce: &'a str,
            url: &'a str,
        }

        let payload = base64::encode_config(payload, base64::URL_SAFE_NO_PAD);

        let jwk = if key_id.is_none() {
            Some(key.public_key().to_jwk())
        } else {
            None
        };

        let protected = Protected {
            alg: "ES256",
            jwk,
            kid: key_id.map(|AccountId(key_id)| key_id.as_str()),
            nonce: &nonce,
            url,
        };

        let protected = serde_json::to_vec(&protected)?;
        let protected = base64::encode_config(protected, base64::URL_SAFE_NO_PAD);

        let signing_key = SigningKey::from(key);
        let signature = signing_key.sign(format!("{}.{}", protected, payload).as_bytes());
        let signature = base64::encode_config(&signature, base64::URL_SAFE_NO_PAD);

        Ok(Jose {
            protected,
            payload,
            signature,
        })
    }
}

#[async_trait::async_trait]
pub trait AcmeBackend {
    fn meta(&self) -> &DirectoryMeta;

    async fn new_account(
        &mut self,
        key: &ES256Key,
        new_account: NewAccount,
    ) -> Result<(AccountId, Account)>;

    async fn new_order(
        &mut self,
        key: &ES256Key,
        account_id: &AccountId,
        new_order: NewOrder,
    ) -> Result<CreateResponse<Order>>;

    async fn get_order(
        &mut self,
        key: &ES256Key,
        account_id: &AccountId,
        order_url: &str,
    ) -> Result<Order>;

    async fn finalize_order(
        &mut self,
        key: &ES256Key,
        account_id: &AccountId,
        order: &Order,
        finalize_order: FinalizeOrder,
    ) -> Result<Order>;

    async fn get_authorizations(
        &mut self,
        key: &ES256Key,
        account_id: &AccountId,
        order: &Order,
    ) -> Result<Vec<Authorization>>;

    async fn respond_challenge(
        &mut self,
        key: &ES256Key,
        account_id: &AccountId,
        challenge: &Challenge,
    ) -> Result<()>;

    async fn download_certificate(
        &mut self,
        key: &ES256Key,
        account_id: &AccountId,
        order_url: &str,
    ) -> Result<String>;
}

pub struct ReqwestAcmeBackend {
    client: reqwest::Client,
    directory: Directory,
    nonce: String,
}

impl ReqwestAcmeBackend {
    pub async fn connect(api_url: &str) -> Result<ReqwestAcmeBackend> {
        let client = reqwest::Client::builder()
            .user_agent("fairing")
            .https_only(true)
            .danger_accept_invalid_certs(true)
            .build()?;

        let directory_res = client.get(api_url).send().await?;
        let directory: Directory = directory_res.json().await?;

        let nonce_res = client.head(&directory.new_nonce).send().await?;
        let nonce = nonce_res
            .headers()
            .get("replay-nonce")
            .ok_or_else(|| anyhow!("couldn't get a new nonce"))?
            .to_str()?
            .to_owned();

        Ok(ReqwestAcmeBackend {
            client,
            directory,
            nonce,
        })
    }
}

#[async_trait::async_trait]
impl AcmeBackend for ReqwestAcmeBackend {
    fn meta(&self) -> &DirectoryMeta {
        &self.directory.meta
    }

    async fn new_account(
        &mut self,
        key: &ES256Key,
        new_account: NewAccount,
    ) -> Result<(AccountId, Account)> {
        let payload = Jose::sign(
            &key,
            None,
            &self.nonce,
            &self.directory.new_account,
            new_account,
        )?;

        let res = self
            .client
            .post(&self.directory.new_account)
            .header(reqwest::header::CONTENT_TYPE, "application/jose+json")
            .body(serde_json::to_vec(&payload)?)
            .send()
            .await?;

        ensure!(
            res.status() == reqwest::StatusCode::CREATED,
            "unexpected status code: {}\n{}",
            res.status(),
            res.text().await?,
        );

        self.nonce = res
            .headers()
            .get("replay-nonce")
            .ok_or_else(|| anyhow!("couldn't get the new nonce"))?
            .to_str()?
            .to_owned();

        let account_id = res
            .headers()
            .get("location")
            .ok_or_else(|| anyhow!("couldn't get account url"))?
            .to_str()?
            .to_owned();

        let account = res.json().await?;

        Ok((AccountId(account_id), account))
    }

    async fn new_order(
        &mut self,
        key: &ES256Key,
        account_id: &AccountId,
        new_order: NewOrder,
    ) -> Result<CreateResponse<Order>> {
        let payload = Jose::sign(
            &key,
            Some(account_id),
            &self.nonce,
            &self.directory.new_order,
            new_order,
        )?;

        let res = self
            .client
            .post(&self.directory.new_order)
            .header(reqwest::header::CONTENT_TYPE, "application/jose+json")
            .body(serde_json::to_vec(&payload)?)
            .send()
            .await?;

        ensure!(
            res.status() == reqwest::StatusCode::CREATED,
            "unexpected status code: {}\n{}",
            res.status(),
            res.text().await?,
        );

        self.nonce = res
            .headers()
            .get("replay-nonce")
            .ok_or_else(|| anyhow!("couldn't get the new nonce"))?
            .to_str()?
            .to_owned();

        let url = res
            .headers()
            .get("location")
            .ok_or_else(|| anyhow!("couldn't get the order's location"))?
            .to_str()?
            .to_owned();

        let order: Order = res.json().await?;

        Ok(CreateResponse { url, body: order })
    }

    async fn get_order(
        &mut self,
        key: &ES256Key,
        account_id: &AccountId,
        order_url: &str,
    ) -> Result<Order> {
        let payload = Jose::sign_empty(&key, Some(account_id), &self.nonce, &order_url)?;

        let res = self
            .client
            .post(order_url)
            .header(reqwest::header::CONTENT_TYPE, "application/jose+json")
            .body(serde_json::to_vec(&payload)?)
            .send()
            .await?;

        ensure!(
            res.status() == reqwest::StatusCode::OK,
            "unexpected status code: {}\n{}",
            res.status(),
            res.text().await?,
        );

        self.nonce = res
            .headers()
            .get("replay-nonce")
            .ok_or_else(|| anyhow!("couldn't get the new nonce"))?
            .to_str()?
            .to_owned();

        let order = res.json().await?;

        Ok(order)
    }

    async fn get_authorizations(
        &mut self,
        key: &ES256Key,
        account_id: &AccountId,
        order: &Order,
    ) -> Result<Vec<Authorization>> {
        let mut authorizations = Vec::with_capacity(order.authorizations.len());

        for authorization_url in order.authorizations.iter() {
            let payload =
                Jose::sign_empty(&key, Some(account_id), &self.nonce, &authorization_url)?;

            let res = self
                .client
                .post(authorization_url)
                .header(reqwest::header::CONTENT_TYPE, "application/jose+json")
                .body(serde_json::to_vec(&payload)?)
                .send()
                .await?;

            ensure!(
                res.status() == reqwest::StatusCode::OK,
                "unexpected status code: {}\n{}",
                res.status(),
                res.text().await?,
            );

            self.nonce = res
                .headers()
                .get("replay-nonce")
                .ok_or_else(|| anyhow!("couldn't get the new nonce"))?
                .to_str()?
                .to_owned();

            let authorization = res.json().await?;
            authorizations.push(authorization);
        }

        Ok(authorizations)
    }

    async fn finalize_order(
        &mut self,
        key: &ES256Key,
        account_id: &AccountId,
        order: &Order,
        finalize_order: FinalizeOrder,
    ) -> Result<Order> {
        let payload = Jose::sign(
            &key,
            Some(account_id),
            &self.nonce,
            &order.finalize,
            finalize_order,
        )?;

        let res = self
            .client
            .post(&order.finalize)
            .header(reqwest::header::CONTENT_TYPE, "application/jose+json")
            .body(serde_json::to_vec(&payload)?)
            .send()
            .await?;

        ensure!(
            res.status() == reqwest::StatusCode::OK,
            "unexpected status code: {}\n{}",
            res.status(),
            res.text().await?,
        );

        self.nonce = res
            .headers()
            .get("replay-nonce")
            .ok_or_else(|| anyhow!("couldn't get the new nonce"))?
            .to_str()?
            .to_owned();

        let order: Order = res.json().await?;

        Ok(order)
    }

    async fn respond_challenge(
        &mut self,
        key: &ES256Key,
        account_id: &AccountId,
        challenge: &Challenge,
    ) -> Result<()> {
        #[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
        struct RespondChallenge {}

        let payload = Jose::sign(
            &key,
            Some(account_id),
            &self.nonce,
            &challenge.url,
            RespondChallenge {},
        )?;

        let res = self
            .client
            .post(&challenge.url)
            .header(reqwest::header::CONTENT_TYPE, "application/jose+json")
            .body(serde_json::to_vec(&payload)?)
            .send()
            .await?;

        ensure!(
            res.status() == reqwest::StatusCode::OK,
            "unexpected status code: {}\n{}",
            res.status(),
            res.text().await?,
        );

        self.nonce = res
            .headers()
            .get("replay-nonce")
            .ok_or_else(|| anyhow!("couldn't get the new nonce"))?
            .to_str()?
            .to_owned();

        Ok(())
    }

    async fn download_certificate(
        &mut self,
        key: &ES256Key,
        account_id: &AccountId,
        certificate_url: &str,
    ) -> Result<String> {
        let payload = Jose::sign_empty(&key, Some(account_id), &self.nonce, certificate_url)?;

        let res = self
            .client
            .post(certificate_url)
            .header(reqwest::header::CONTENT_TYPE, "application/jose+json")
            .body(serde_json::to_vec(&payload)?)
            .send()
            .await?;

        ensure!(
            res.status() == reqwest::StatusCode::OK,
            "unexpected status code: {}\n{}",
            res.status(),
            res.text().await?,
        );

        self.nonce = res
            .headers()
            .get("replay-nonce")
            .ok_or_else(|| anyhow!("couldn't get the new nonce"))?
            .to_str()?
            .to_owned();

        let certificate = res.text().await?;

        Ok(certificate)
    }
}

pub async fn new_account(
    backend: &mut impl AcmeBackend,
    new_account: NewAccount,
) -> Result<(ES256Key, AccountId, Account)> {
    let key = ES256Key::random(&mut OsRng);

    let (account_id, account) = backend.new_account(&key, new_account).await?;

    Ok((key, account_id, account))
}

pub fn parse_key(private_key: &str) -> Result<ES256Key> {
    let private_key = base64::decode_config(private_key, base64::URL_SAFE_NO_PAD)?;
    let private_key = ES256Key::from_sec1_der(&private_key)?;
    Ok(private_key)
}

pub fn key_authorization(key: &ES256Key, token: &str) -> String {
    let jwk = key.to_jwk_string();

    let mut hasher = Sha256::new();
    hasher.update(jwk.as_bytes());

    let thumbprint = hasher.finalize();
    let thumbprint = base64::encode_config(thumbprint, base64::URL_SAFE_NO_PAD);

    format!("{token}.{thumbprint}")
}
