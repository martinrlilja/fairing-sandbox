use anyhow::{anyhow, ensure, Result};
use p256::ecdsa::{signature::Signer, SigningKey};
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
    #[serde(default)]
    pub terms_of_service: Option<String>,
    #[serde(default)]
    pub website: Option<String>,
    #[serde(default)]
    pub caa_identities: Vec<String>,
    #[serde(default)]
    pub external_account_required: bool,
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateAccount {
    pub terms_of_service_agreed: bool,
    pub contact: Vec<String>,
}

#[derive(Clone, Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    pub url: String,
    pub status: AccountStatus,
    pub contact: Vec<String>,
    #[serde(default)]
    pub terms_of_service_agreed: Option<bool>,
    // Let's encrypt's servers don't always return this field.
    #[serde(default)]
    pub orders: Option<String>,
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
pub struct CreateOrder {
    pub identifiers: Vec<Identifier>,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct AcmeOrder {
    status: OrderStatus,
    #[serde(default)]
    expires: Option<String>,
    #[serde(default)]
    error: Option<serde_json::Value>,
    authorizations: Vec<String>,
    finalize: String,
    #[serde(default)]
    certificate: Option<String>,
}

impl AcmeOrder {
    fn with_url(self, url: impl Into<String>) -> Order {
        Order {
            url: url.into(),
            status: self.status,
            expires: self.expires,
            error: self.error,
            authorizations: self.authorizations,
            finalize: self.finalize,
            certificate: self.certificate,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Order {
    pub url: String,
    pub status: OrderStatus,
    pub expires: Option<String>,
    pub error: Option<serde_json::Value>,
    pub authorizations: Vec<String>,
    pub finalize: String,
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

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AcmeError {
    #[serde(rename = "type")]
    pub type_: String,
    pub detail: String,
    #[serde(default)]
    pub subproblems: Vec<AcmeSubproblem>,
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AcmeSubproblem {
    #[serde(rename = "type")]
    pub type_: String,
    pub detail: String,
    #[serde(default)]
    pub identifier: Option<Identifier>,
}

pub struct Response {
    pub status: http::StatusCode,
    pub location: Option<String>,
    pub replay_nonce: Option<String>,
    pub body: Vec<u8>,
}

impl Into<anyhow::Error> for Response {
    fn into(self) -> anyhow::Error {
        let body = serde_json::from_slice(&self.body)
            .map(|error: AcmeError| {
                let mut s = String::new();
                s.push_str(&error.type_);
                s.push_str(": ");
                s.push_str(&error.detail);

                for subproblem in error.subproblems {
                    s.push('\n');
                    s.push_str(&subproblem.type_);

                    if let Some(identifier) = subproblem.identifier {
                        s.push_str(" (");
                        s.push_str(&identifier.value);
                        s.push(')');
                    }

                    s.push_str(": ");
                    s.push_str(&subproblem.detail);
                }

                s
            })
            .or_else(|_err| String::from_utf8(self.body))
            .ok();

        if let Some(body) = body {
            anyhow!("unexpected status ({}): {body}", self.status)
        } else {
            anyhow!("unexpected status ({})", self.status)
        }
    }
}

#[derive(Clone)]
pub struct ES256SecretKey(p256::SecretKey);

#[derive(Copy, Clone)]
pub struct ES256PublicKey(p256::PublicKey);

impl ES256SecretKey {
    pub fn generate() -> Self {
        let key = p256::SecretKey::random(&mut OsRng);
        Self(key)
    }

    pub fn public_key(&self) -> ES256PublicKey {
        ES256PublicKey(self.0.public_key())
    }

    pub fn parse_key(private_key: &str) -> Result<Self> {
        let private_key = base64::decode_config(private_key, base64::URL_SAFE_NO_PAD)?;
        let private_key = p256::SecretKey::from_sec1_der(&private_key)?;
        Ok(Self(private_key))
    }

    pub fn to_string(&self) -> Result<String> {
        let private_key = self
            .0
            .to_sec1_der()
            .map_err(|_err| anyhow!("cannot convert private key to der"))?;
        let private_key = base64::encode_config(&private_key[..], base64::URL_SAFE_NO_PAD);
        Ok(private_key)
    }
}

impl ES256PublicKey {
    pub fn key_authorization(&self, token: &str) -> String {
        use std::collections::BTreeMap;

        // Sort the keys before serializing.
        let jwk = serde_json::to_value(&self.to_jwk()).unwrap();
        let jwk = if let serde_json::Value::Object(jwk) = jwk {
            let jwk = jwk.into_iter().collect::<BTreeMap<_, _>>();
            serde_json::to_vec(&jwk).unwrap()
        } else {
            unreachable!("jwk must be an object");
        };

        let mut hasher = Sha256::new();
        hasher.update(&jwk);

        let thumbprint = hasher.finalize();
        let thumbprint = base64::encode_config(thumbprint, base64::URL_SAFE_NO_PAD);

        format!("{token}.{thumbprint}")
    }

    pub fn dns_key_authorization(&self, token: &str) -> String {
        let key_authorization = self.key_authorization(token);

        let mut hasher = Sha256::new();
        hasher.update(key_authorization.as_bytes());

        let dns_key_authorization = hasher.finalize();
        base64::encode_config(dns_key_authorization, base64::URL_SAFE_NO_PAD)
    }

    pub fn to_jwk(&self) -> p256::elliptic_curve::JwkEcKey {
        self.0.to_jwk()
    }
}

#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Jose {
    pub protected: String,
    pub payload: String,
    pub signature: String,
}

impl Jose {
    pub fn sign(
        key: &ES256SecretKey,
        key_id: Option<&str>,
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
            kid: key_id,
            nonce: &nonce,
            url,
        };

        let protected = serde_json::to_vec(&protected)?;
        let protected = base64::encode_config(protected, base64::URL_SAFE_NO_PAD);

        let signing_key = SigningKey::from(key.0.clone());
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
pub trait AcmeClientBackend {
    async fn get_directory(&mut self, url: &str) -> Result<Directory>;

    async fn get_nonce(&mut self, url: &str) -> Result<String>;

    async fn post(&mut self, url: &str, body: Jose) -> Result<Response>;
}

pub struct WithoutAccount;

pub struct WithAccount {
    key: ES256SecretKey,
    key_id: Option<String>,
}

pub struct AcmeClient<Backend, Account> {
    backend: Backend,
    directory: Directory,
    nonce: String,
    account: Account,
}

pub type AcmeClientWithoutAccount<Backend> = AcmeClient<Backend, WithoutAccount>;
pub type AcmeClientWithAccount<Backend> = AcmeClient<Backend, WithAccount>;

impl<Backend: AcmeClientBackend, Account> AcmeClient<Backend, Account> {
    pub fn meta(&self) -> &DirectoryMeta {
        &self.directory.meta
    }
}

impl<Backend: AcmeClientBackend> AcmeClient<Backend, WithoutAccount> {
    pub async fn connect(
        mut backend: Backend,
        api_url: &str,
    ) -> Result<AcmeClient<Backend, WithoutAccount>> {
        let directory = backend.get_directory(api_url).await?;
        let nonce = backend.get_nonce(&directory.new_nonce).await?;

        Ok(AcmeClient {
            backend,
            directory,
            nonce,
            account: WithoutAccount,
        })
    }

    pub async fn create_account(
        mut self,
        new_account: &CreateAccount,
    ) -> Result<AcmeClient<Backend, WithAccount>> {
        let key = ES256SecretKey::generate();

        let account = WithAccount { key, key_id: None };

        let new_account = serde_json::to_vec(new_account)?;
        let response = retry_post(
            &mut self.backend,
            &mut self.nonce,
            &account,
            &self.directory.new_account,
            &new_account,
        )
        .await?;

        if response.status == http::StatusCode::CREATED {
            let key_id = response
                .location
                .ok_or_else(|| anyhow!("response does not have a location header"))?;

            Ok(AcmeClient {
                backend: self.backend,
                directory: self.directory,
                nonce: self.nonce,
                account: WithAccount {
                    key: account.key,
                    key_id: Some(key_id),
                },
            })
        } else {
            Err(response.into())
        }
    }

    pub fn with_account(
        self,
        key: ES256SecretKey,
        key_id: &str,
    ) -> Result<AcmeClient<Backend, WithAccount>> {
        Ok(AcmeClient {
            backend: self.backend,
            directory: self.directory,
            nonce: self.nonce,
            account: WithAccount {
                key,
                key_id: Some(key_id.to_owned()),
            },
        })
    }
}

impl<Backend: AcmeClientBackend> AcmeClient<Backend, WithAccount> {
    pub fn secret_key(&self) -> &ES256SecretKey {
        &self.account.key
    }

    pub fn secret_key_id(&self) -> &str {
        &self.account.key_id.as_ref().map(String::as_ref).unwrap()
    }

    pub async fn create_order(&mut self, new_order: &CreateOrder) -> Result<Order> {
        let new_order = serde_json::to_vec(new_order)?;
        let response = retry_post(
            &mut self.backend,
            &mut self.nonce,
            &self.account,
            &self.directory.new_order,
            &new_order,
        )
        .await?;

        if response.status == http::StatusCode::CREATED {
            let url = response
                .location
                .ok_or_else(|| anyhow!("response does not have a location header"))?;

            let order: AcmeOrder = serde_json::from_slice(&response.body)?;
            Ok(order.with_url(url))
        } else {
            Err(response.into())
        }
    }

    pub async fn get_order(&mut self, order_url: &str) -> Result<Order> {
        let response = retry_post(
            &mut self.backend,
            &mut self.nonce,
            &self.account,
            &order_url,
            &[],
        )
        .await?;

        if response.status == http::StatusCode::OK {
            let order: AcmeOrder = serde_json::from_slice(&response.body)?;
            Ok(order.with_url(order_url))
        } else {
            Err(response.into())
        }
    }

    pub async fn finalize_order(
        &mut self,
        finalize_order_url: &str,
        finalize_order: &FinalizeOrder,
    ) -> Result<Order> {
        let finalize_order = serde_json::to_vec(finalize_order)?;
        let response = retry_post(
            &mut self.backend,
            &mut self.nonce,
            &self.account,
            &finalize_order_url,
            &finalize_order,
        )
        .await?;

        if response.status == http::StatusCode::OK {
            let url = response
                .location
                .ok_or_else(|| anyhow!("response does not have a location header"))?;

            let order: AcmeOrder = serde_json::from_slice(&response.body)?;
            Ok(order.with_url(url))
        } else {
            Err(response.into())
        }
    }

    pub async fn get_authorization(&mut self, authorization_url: &str) -> Result<Authorization> {
        let response = retry_post(
            &mut self.backend,
            &mut self.nonce,
            &self.account,
            &authorization_url,
            &[],
        )
        .await?;

        if response.status == http::StatusCode::OK {
            let authorization: Authorization = serde_json::from_slice(&response.body)?;
            Ok(authorization)
        } else {
            Err(response.into())
        }
    }

    pub async fn accept_challenge(&mut self, challenge_url: &str) -> Result<()> {
        let response = retry_post(
            &mut self.backend,
            &mut self.nonce,
            &self.account,
            &challenge_url,
            b"{}",
        )
        .await?;

        if response.status == http::StatusCode::OK {
            Ok(())
        } else {
            Err(response.into())
        }
    }

    pub async fn download_certificate(&mut self, certificate_url: &str) -> Result<String> {
        let response = retry_post(
            &mut self.backend,
            &mut self.nonce,
            &self.account,
            &certificate_url,
            &[],
        )
        .await?;

        if response.status == http::StatusCode::OK {
            let certificate = String::from_utf8(response.body)
                .map_err(|_err| anyhow!("certificate is not valid utf8"))?;
            Ok(certificate)
        } else {
            Err(response.into())
        }
    }
}

async fn retry_post<Backend: AcmeClientBackend>(
    backend: &mut Backend,
    nonce: &mut String,
    account: &WithAccount,
    url: &str,
    payload: &[u8],
) -> Result<Response> {
    let mut response = post(backend, nonce, account, url, payload).await?;

    while response.status.is_client_error() {
        let error: AcmeError = serde_json::from_slice(&response.body)?;

        // Retry the post if we used a bad nonce according to RFC 8555 6.5 Replay Protection.
        if error.type_ == "urn:ietf:params:acme:error:badNonce" {
            let nonce = response
                .replay_nonce
                .ok_or_else(|| anyhow!("no replay-nonce"))?;

            response = post(backend, &nonce, account, url, payload).await?;
        } else {
            break;
        }
    }

    if let Some(new_nonce) = response.replay_nonce.take() {
        *nonce = new_nonce;
    }

    Ok(response)
}

async fn post<Backend: AcmeClientBackend>(
    backend: &mut Backend,
    nonce: &str,
    account: &WithAccount,
    url: &str,
    payload: &[u8],
) -> Result<Response> {
    let body = Jose::sign(
        &account.key,
        account.key_id.as_ref().map(String::as_ref),
        &nonce,
        url,
        payload,
    )?;

    backend.post(url, body).await
}

#[async_trait::async_trait]
impl AcmeClientBackend for reqwest::Client {
    async fn get_directory(&mut self, url: &str) -> Result<Directory> {
        let res = self.get(url).send().await?;
        ensure!(
            res.status() == reqwest::StatusCode::OK,
            "unexpected status code: {}",
            res.status(),
        );

        let directory = res.json().await?;
        Ok(directory)
    }

    async fn get_nonce(&mut self, url: &str) -> Result<String> {
        let res = self.head(url).send().await?;
        let nonce = res
            .headers()
            .get("replay-nonce")
            .ok_or_else(|| anyhow!("couldn't get a new nonce"))?
            .to_str()?
            .to_owned();

        Ok(nonce)
    }

    async fn post(&mut self, url: &str, body: Jose) -> Result<Response> {
        let res = reqwest::Client::post(&self, url)
            .header(reqwest::header::CONTENT_TYPE, "application/jose+json")
            .body(serde_json::to_vec(&body)?)
            .send()
            .await?;

        let status = res.status();

        let location = res
            .headers()
            .get(http::header::LOCATION)
            .and_then(|header| header.to_str().ok())
            .map(str::to_owned);

        let replay_nonce = res
            .headers()
            .get("replay-nonce")
            .and_then(|header| header.to_str().ok())
            .map(str::to_owned);

        let body = res.bytes().await?.to_vec();

        Ok(Response {
            status,
            location,
            replay_nonce,
            body,
        })
    }
}
