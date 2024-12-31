use std::{str::FromStr, sync::Arc};

use anyhow::Result;
use atoma_state::types::AtomaAtomaStateManagerEvent;
use atoma_utils::hashing::blake2b_hash;
use blake2::{
    digest::{consts::U32, generic_array::GenericArray},
    Blake2b, Digest,
};
use chrono::{Duration, Utc};
use fastcrypto::{
    ed25519::{Ed25519PublicKey, Ed25519Signature},
    secp256k1::{Secp256k1PublicKey, Secp256k1Signature},
    secp256r1::{Secp256r1PublicKey, Secp256r1Signature},
    traits::{ToFromBytes, VerifyingKey},
};
use flume::Sender;
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use rand::Rng;
use serde::{Deserialize, Serialize};
use shared_crypto::intent::{Intent, IntentMessage, PersonalMessage};
use sui_sdk::types::{
    base_types::SuiAddress,
    crypto::{PublicKey, Signature, SignatureScheme, SuiSignature},
    object::Owner,
    TypeTag,
};
use tokio::sync::{oneshot, RwLock};
use tracing::{error, instrument};

use crate::{AtomaAuthConfig, Sui};

/// The length of the API token
const API_TOKEN_LENGTH: usize = 30;

/// The claims struct for the JWT token
#[derive(Debug, Serialize, Deserialize)]
pub struct Claims {
    /// The user id from the DB
    user_id: i64,
    /// The expiration time of the token
    exp: usize,
    // If this token is a refresh token, this will be empty, in case of access token the refresh will be the hash of the refresh token
    refresh_token_hash: Option<String>,
}

/// The Auth struct
#[derive(Clone)]
pub struct Auth {
    /// The secret key for JWT authentication.
    secret_key: String,
    /// The access token lifetime in minutes.
    access_token_lifetime: usize,
    /// The refresh token lifetime in days.
    refresh_token_lifetime: usize,
    /// The sender for the state manager
    state_manager_sender: Sender<AtomaAtomaStateManagerEvent>,
    /// The sui client
    sui: Arc<RwLock<Sui>>,
}

impl Auth {
    /// Constructor
    pub fn new(
        config: AtomaAuthConfig,
        state_manager_sender: Sender<AtomaAtomaStateManagerEvent>,
        sui: Arc<RwLock<Sui>>,
    ) -> Self {
        Self {
            secret_key: config.secret_key,
            access_token_lifetime: config.access_token_lifetime,
            refresh_token_lifetime: config.refresh_token_lifetime,
            state_manager_sender,
            sui,
        }
    }

    /// Generate a new refresh token
    /// This method will generate a new refresh token for the user
    ///
    /// # Arguments
    ///
    /// * `user_id` - The user id for which the token is generated
    ///
    /// # Returns
    ///
    /// * `Result<String>` - The generated refresh token
    ///
    /// # Errors
    ///
    /// * If the token generation fails
    #[instrument(level = "trace", skip(self))]
    async fn generate_refresh_token(&self, user_id: i64) -> Result<String> {
        let expiration = Utc::now() + Duration::days(self.refresh_token_lifetime as i64);
        let claims = Claims {
            user_id,
            exp: expiration.timestamp() as usize,
            refresh_token_hash: None,
        };
        let token = encode(
            &Header::default(),
            &claims,
            &EncodingKey::from_secret(self.secret_key.as_ref()),
        )?;
        self.state_manager_sender
            .send(AtomaAtomaStateManagerEvent::StoreRefreshToken {
                user_id,
                refresh_token_hash: self.hash_string(&token),
            })?;
        Ok(token)
    }

    /// This method validates a JWT token
    /// The method will check if the token is expired and if the token is a refresh token or an access token
    ///
    /// # Arguments
    ///
    /// * `token` - The token to be validated
    /// * `is_refresh` - If the token is a refresh token
    ///
    /// # Returns
    ///
    /// * `Result<Claims>` - The claims of the token
    #[instrument(level = "trace", skip(self))]
    pub fn validate_token(&self, token: &str, is_refresh: bool) -> Result<Claims> {
        let mut validation = Validation::default();
        validation.validate_exp = true; // Enforce expiration validation

        let token_data = decode::<Claims>(
            token,
            &DecodingKey::from_secret(self.secret_key.as_ref()),
            &validation,
        )?;

        let claims = token_data.claims;
        if claims.refresh_token_hash.is_none() != is_refresh {
            Err(anyhow::anyhow!("Invalid token type"))
        } else {
            Ok(claims)
        }
    }

    /// Check the validity of the refresh token
    /// This method will check if the refresh token is valid (was not revoked) and it's not expired
    ///
    /// # Arguments
    ///
    /// * `refresh_token` - The refresh token to be checked
    ///
    /// # Returns
    ///
    /// * `Result<bool>` - If the refresh token is valid
    #[instrument(level = "trace", skip(self))]
    async fn check_refresh_token_validity(
        &self,
        user_id: i64,
        refresh_token_hash: &str,
    ) -> Result<bool> {
        let (result_sender, result_receiver) = oneshot::channel();
        self.state_manager_sender
            .send(AtomaAtomaStateManagerEvent::IsRefreshTokenValid {
                user_id,
                refresh_token_hash: refresh_token_hash.to_string(),
                result_sender,
            })?;
        Ok(result_receiver.await??)
    }

    /// Generate a new access token from a refresh token
    /// The refresh token is hashed and used in the access token, so we can check the validity of the access token based on the refresh token.
    ///
    /// # Arguments
    ///
    /// * `refresh_token` - The refresh token to be used to generate a new access token
    ///
    /// # Returns
    ///
    /// * `Result<String>` - The new access token
    #[instrument(level = "trace", skip(self))]
    pub async fn generate_access_token(&self, refresh_token: &str) -> Result<String> {
        let claims = self.validate_token(refresh_token, true)?;
        let refresh_token_hash = self.hash_string(refresh_token);

        if !self
            .check_refresh_token_validity(claims.user_id, &refresh_token_hash)
            .await?
        {
            return Err(anyhow::anyhow!("Refresh token is not valid"));
        }
        let expiration = Utc::now() + Duration::days(self.access_token_lifetime as i64);

        let claims = Claims {
            user_id: claims.user_id,
            exp: expiration.timestamp() as usize,
            refresh_token_hash: Some(refresh_token_hash),
        };
        let token = encode(
            &Header::default(),
            &claims,
            &EncodingKey::from_secret(self.secret_key.as_ref()),
        )?;
        Ok(token)
    }

    /// Used for hashing password / refresh tokens
    /// This method will hash the input using the Blake2b algorithm
    ///
    /// # Arguments
    ///
    /// * `password` - The password to be hashed
    ///
    /// # Returns
    ///
    /// * `String` - The hashed password
    pub fn hash_string(&self, text: &str) -> String {
        let mut hasher = Blake2b::new();
        hasher.update(text);
        let hash_result: GenericArray<u8, U32> = hasher.finalize();
        hex::encode(hash_result)
    }

    /// Register user with username/password.
    /// This method will register a new user with a username and password
    /// The password is hashed and stored in the DB
    /// The method will generate a new refresh and access token
    #[instrument(level = "info", skip(self))]
    pub async fn register(&self, username: &str, password: &str) -> Result<(String, String)> {
        let (result_sender, result_receiver) = oneshot::channel();
        self.state_manager_sender
            .send(AtomaAtomaStateManagerEvent::RegisterUserWithPassword {
                username: username.to_string(),
                password: self.hash_string(password),
                result_sender,
            })?;
        let user_id = result_receiver
            .await??
            .map(|user_id| user_id as u64)
            .ok_or_else(|| anyhow::anyhow!("User already registred"))?;
        let refresh_token = self.generate_refresh_token(user_id as i64).await?;
        let access_token = self.generate_access_token(&refresh_token).await?;
        Ok((refresh_token, access_token))
    }

    /// Check the user password
    /// This method will check if the user password is correct
    /// The password is hashed and compared with the hashed password in the DB
    /// If the password is correct, the method will generate a new refresh and access token
    #[instrument(level = "info", skip(self, password))]
    pub async fn check_user_password(
        &self,
        username: &str,
        password: &str,
    ) -> Result<(String, String)> {
        let (result_sender, result_receiver) = oneshot::channel();
        self.state_manager_sender.send(
            AtomaAtomaStateManagerEvent::GetUserIdByUsernamePassword {
                username: username.to_string(),
                password: self.hash_string(password),
                result_sender,
            },
        )?;
        let user_id = result_receiver
            .await??
            .map(|user_id| user_id as u64)
            .ok_or_else(|| anyhow::anyhow!("User not found"))?;
        let refresh_token = self.generate_refresh_token(user_id as i64).await?;
        let access_token = self.generate_access_token(&refresh_token).await?;
        Ok((refresh_token, access_token))
    }

    /// Generate a new API token
    /// This method will generate a new API token for the user
    /// The method will check if the access token and its corresponding refresh token is valid and store the new API token in the state manager
    ///
    /// # Arguments
    ///
    /// * `jwt` - The access token to be used to generate the API token
    ///
    /// # Returns
    ///
    /// * `Result<String>` - The generated API token
    #[instrument(level = "info", skip(self))]
    pub async fn generate_api_token(&self, jwt: &str) -> Result<String> {
        let claims = self.get_claims_from_token(jwt).await?;
        let api_token: String = rand::thread_rng()
            .sample_iter(&rand::distributions::Alphanumeric)
            .take(API_TOKEN_LENGTH)
            .map(char::from)
            .collect();
        self.state_manager_sender
            .send(AtomaAtomaStateManagerEvent::StoreNewApiToken {
                user_id: claims.user_id,
                api_token: api_token.clone(),
            })?;
        Ok(api_token)
    }

    /// Revoke an API token
    /// This method will revoke an API token for the user
    /// The method will check if the access token and its corresponding refresh token is valid and revoke the API token in the state manager
    ///
    /// # Arguments
    ///
    /// * `jwt` - The access token to be used to revoke the API token
    /// * `api_token` - The API token to be revoked
    ///
    /// # Returns
    ///
    /// * `Result<()>` - If the API token was revoked
    #[instrument(level = "info", skip(self))]
    pub async fn revoke_api_token(&self, jwt: &str, api_token: &str) -> Result<()> {
        let claims = self.get_claims_from_token(jwt).await?;
        self.state_manager_sender
            .send(AtomaAtomaStateManagerEvent::RevokeApiToken {
                user_id: claims.user_id,
                api_token: api_token.to_string(),
            })?;
        Ok(())
    }

    /// Get all API tokens for a user
    /// This method will get all API tokens for a user
    /// The method will check if the access token and its corresponding refresh token is valid
    ///
    /// # Arguments
    ///
    /// * `jwt` - The access token to be used to get the API tokens
    ///
    /// # Returns
    ///
    /// * `Result<Vec<String>>` - The list of API tokens
    #[instrument(level = "info", skip(self))]
    pub async fn get_all_api_tokens(&self, jwt: &str) -> Result<Vec<String>> {
        let claims = self.get_claims_from_token(jwt).await?;

        let (result_sender, result_receiver) = oneshot::channel();
        self.state_manager_sender
            .send(AtomaAtomaStateManagerEvent::GetApiTokensForUser {
                user_id: claims.user_id,
                result_sender,
            })?;
        Ok(result_receiver.await??)
    }

    /// Get the claims from the token
    /// This method will get the claims from the token and check if the refresh token is valid
    /// The method will return the claims if the token is valid
    ///
    /// # Arguments
    ///
    /// * `jwt` - The access token to be used to get the claims
    ///
    /// # Returns
    ///
    /// * `Result<Claims>` - The claims from the token
    async fn get_claims_from_token(&self, jwt: &str) -> Result<Claims> {
        let claims = self.validate_token(jwt, false)?;
        if !self
            .check_refresh_token_validity(
                claims.user_id,
                &claims
                    .refresh_token_hash
                    .clone()
                    .expect("access token should have refresh token hash"),
            )
            .await?
        {
            return Err(anyhow::anyhow!("Access token was revoked"));
        }
        Ok(claims)
    }

    /// Get the user id from the token
    /// This method will get the user id from the token
    /// The method will check if the token is valid
    ///
    /// # Arguments
    ///
    /// * `jwt` - The access token to be used to get the user id
    ///
    /// # Returns
    ///
    /// * `Result<i64>` - The user id from the token
    pub async fn get_user_id_from_token(&self, jwt: &str) -> Result<i64> {
        let claims = self.get_claims_from_token(jwt).await?;
        Ok(claims.user_id)
    }

    /// Stores the wallet address for the user. The user needs to send a signed message to prove ownership of the wallet.
    /// The wallet address is stored in the signature.
    ///
    /// # Arguments
    /// * `jwt` - The access token to be used to store the wallet address
    /// * `signature` - The signature of the message
    ///
    /// # Returns
    ///
    /// * `Result<()>` - If the wallet address was stored
    #[instrument(level = "info", skip(self))]
    pub async fn update_sui_address(&self, jwt: &str, signature: &str) -> Result<()> {
        let claims = self.validate_token(jwt, false)?;
        let signature = Signature::from_str(signature).map_err(|e| {
            error!("Failed to parse signature: {}", e);
            anyhow::anyhow!("Failed to parse signature {e}")
        })?;
        let signature_bytes = signature.signature_bytes();
        let public_key_bytes = signature.public_key_bytes();
        let signature_scheme = signature.scheme();
        let intent_msg = IntentMessage::new(
            Intent::personal_message(),
            PersonalMessage {
                message: format!(
                    "Sign this message to prove you are the owner of this wallet. User ID: {}",
                    claims.user_id
                )
                .as_bytes()
                .to_vec(),
            },
        );

        let intent_bcs = bcs::to_bytes(&intent_msg)?;
        let message_hash = blake2b_hash(&intent_bcs);

        match signature_scheme {
            SignatureScheme::ED25519 => {
                let public_key = Ed25519PublicKey::from_bytes(public_key_bytes)?;
                let signature = Ed25519Signature::from_bytes(signature_bytes)?;
                public_key.verify(message_hash.as_slice(), &signature)?
            }
            SignatureScheme::Secp256k1 => {
                let public_key = Secp256k1PublicKey::from_bytes(public_key_bytes)?;
                let signature = Secp256k1Signature::from_bytes(signature_bytes)?;
                public_key.verify(message_hash.as_slice(), &signature)?;
            }
            SignatureScheme::Secp256r1 => {
                let public_key = Secp256r1PublicKey::from_bytes(public_key_bytes)?;
                let signature = Secp256r1Signature::from_bytes(signature_bytes)?;
                public_key.verify(message_hash.as_slice(), &signature)?;
            }
            _ => {
                error!("Currently unsupported signature scheme");
                return Err(anyhow::anyhow!("Currently unsupported signature scheme"));
            }
        }
        let public_key = PublicKey::try_from_bytes(signature_scheme, public_key_bytes).unwrap();
        let sui_address = SuiAddress::from(&public_key);
        self.state_manager_sender
            .send(AtomaAtomaStateManagerEvent::UpdateSuiAddress {
                user_id: claims.user_id,
                sui_address: sui_address.to_string(),
            })?;
        Ok(())
    }

    /// Updates the balance of the user
    ///
    /// # Arguments
    ///
    /// * `jwt` - The access token to be used to update the balance
    /// * `transaction_digest` - The transaction digest to be used to update the balance
    ///
    /// # Returns
    ///
    /// * `Result<()>` - If the balance was updated
    ///
    /// # Errors
    ///
    /// * If the balance changes are not found
    /// * If the sender or receiver is not found
    /// * If the payment is not for this user
    /// * If the user is not found
    /// * If the user balance is not updated
    #[instrument(level = "info", skip(self))]
    pub async fn usdc_payment(&self, jwt: &str, transaction_digest: &str) -> Result<()> {
        let claims = self.validate_token(jwt, false)?;

        let (timestamp, balance_changes) = self
            .sui
            .read()
            .await
            .get_balance_changes(transaction_digest)
            .await?;
        let balance_changes =
            balance_changes.ok_or_else(|| anyhow::anyhow!("No balance changes found"))?;
        let timestamp = timestamp.ok_or_else(|| anyhow::anyhow!("No timestamp found"))?;
        let mut sender = None;
        let mut receiver = None;
        let mut money_in = None;
        for balance_change in balance_changes {
            if let TypeTag::Struct(tag) = balance_change.coin_type {
                if tag.address.to_hex() == self.sui.read().await.usdc_package_id.to_hex() {
                    if balance_change.amount < 0 {
                        if sender.is_some() {
                            return Err(anyhow::anyhow!("Multiple senders"));
                        }
                        if let Owner::AddressOwner(owner) = &balance_change.owner {
                            sender = Some(*owner);
                        }
                    } else {
                        if receiver.is_some() {
                            return Err(anyhow::anyhow!("Multiple receivers"));
                        }
                        money_in = Some(balance_change.amount);
                        if let Owner::AddressOwner(owner) = &balance_change.owner {
                            receiver = Some(*owner);
                        }
                    }
                }
            }
        }
        if sender.is_none() || receiver.is_none() {
            return Err(anyhow::anyhow!("No sender or receiver found"));
        }
        let sender = sender.unwrap();
        let receiver = receiver.unwrap();
        let own_address = self.sui.write().await.get_wallet_address()?;
        if receiver == own_address {
            let (result_sender, result_receiver) = oneshot::channel();
            self.state_manager_sender
                .send(AtomaAtomaStateManagerEvent::GetUserId {
                    sui_address: sender.to_string(),
                    result_sender,
                })?;
            let user_id = result_receiver
                .await??
                .ok_or_else(|| anyhow::anyhow!("User not found"))?;
            if claims.user_id != user_id {
                return Err(anyhow::anyhow!("The payment is not for this user"));
            }
            // We are the receiver and we know the sender
            self.state_manager_sender
                .send(AtomaAtomaStateManagerEvent::TopUpBalance {
                    user_id,
                    amount: money_in.unwrap() as i64,
                    timestamp: timestamp as i64,
                })?;
        }
        Ok(())
    }

    /// Get the Sui address for the user
    ///
    /// # Arguments
    ///
    /// * `jwt` - The access token to be used to get the Sui address
    ///
    /// # Returns
    ///
    /// * `Result<Option<String>>` - The Sui address
    ///
    /// # Errors
    ///
    /// * If the verification fails
    pub async fn get_sui_address(&self, jwt: &str) -> Result<Option<String>> {
        let claims = self.validate_token(jwt, false)?;
        let (result_sender, result_receiver) = oneshot::channel();
        self.state_manager_sender
            .send(AtomaAtomaStateManagerEvent::GetSuiAddress {
                user_id: claims.user_id,
                result_sender,
            })?;
        let sui_address = result_receiver.await;
        Ok(sui_address??)
    }
}

// TODO: Add more comprehensive tests, for now test the happy path only
#[cfg(test)]
mod test {
    use std::{path::PathBuf, sync::Arc};

    use atoma_state::types::AtomaAtomaStateManagerEvent;
    use atoma_sui::AtomaSuiConfig;
    use flume::Receiver;
    use tokio::sync::RwLock;

    use crate::AtomaAuthConfig;

    use super::Auth;
    use std::env;
    use std::fs::File;
    use std::io::Write;

    fn get_config_path() -> PathBuf {
        let manifest_dir = env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is not set");
        let workspace_path = PathBuf::from(manifest_dir);
        let workspace_path = workspace_path.parent().unwrap();
        let workspace_cargo_toml_path = workspace_path.join("config.test.toml");
        let sui_config_path = workspace_path.join("client.yaml");
        let sui_keystore_path = workspace_path.join("sui.keystore");
        let config_content = format!(
            r#"
[atoma_sui]
http_rpc_node_addr = "https://fullnode.testnet.sui.io:443"
atoma_db = "0x741693fc00dd8a46b6509c0c3dc6a095f325b8766e96f01ba73b668df218f859"
atoma_package_id = "0x0c4a52c2c74f9361deb1a1b8496698c7e25847f7ad9abfbd6f8c511e508c62a0"
usdc_package_id = "0xa1ec7fc00a6f40db9693ad1415d0c193ad3906494428cf252621037bd7117e29"
request_timeout = {{ secs = 300, nanos = 0 }}
max_concurrent_requests = 10
limit = 100
sui_config_path = "{}"
sui_keystore_path = "{}"
cursor_path = "./cursor.toml"

[atoma_state]
database_url = "postgresql://POSTGRES_USER:POSTGRES_PASSWORD@db:5432/POSTGRES_DB"

[atoma_service]
service_bind_address = "0.0.0.0:8080"
password = "password"
models = [
    "meta-llama/Llama-3.2-3B-Instruct",
    "meta-llama/Llama-3.2-1B-Instruct",
]
revisions = ["main", "main"]
hf_token = "<API_KEY>"

[atoma_proxy_service]
service_bind_address = "0.0.0.0:8081"

[atoma_auth]
secret_key = "secret_key"
access_token_lifetime = 1
refresh_token_lifetime = 1
        "#,
            sui_config_path.to_str().unwrap().replace("\\", "\\\\"),
            sui_keystore_path.to_str().unwrap().replace("\\", "\\\\")
        );

        let mut file = File::create(&workspace_cargo_toml_path).unwrap();
        file.write_all(config_content.as_bytes()).unwrap();

        let mut sui_config_file = File::create(&sui_config_path).unwrap();
        sui_config_file
            .write_all(
                format!(
                    r#"---
keystore:
  File: "{}"
envs:
  - alias: testnet
    rpc: "https://fullnode.testnet.sui.io:443"
    ws: ~
    basic_auth: ~
active_env: testnet
active_address: "0x939cfcc7fcbc71ce983203bcb36fa498901932ab9293dfa2b271203e7160381b"
"#,
                    sui_keystore_path.to_str().unwrap().replace("\\", "\\\\")
                )
                .as_bytes(),
            )
            .unwrap();

        let mut sui_keystore_file = File::create(&sui_keystore_path).unwrap();
        sui_keystore_file
            .write_all(
                br#"[
    "AEz/bWMHAhWXHSc2jaDxYAIWmlvKAosthvaAEe/JzrWd"
]
        "#,
            )
            .unwrap();
        workspace_cargo_toml_path
    }

    async fn setup_test() -> (Auth, Receiver<AtomaAtomaStateManagerEvent>) {
        let config = AtomaAuthConfig::new("secret".to_string(), 1, 1);
        let (state_manager_sender, state_manager_receiver) = flume::unbounded();

        let sui_config = AtomaSuiConfig::from_file_path(get_config_path());
        let sui = crate::Sui::new(&sui_config).await.unwrap();
        let auth = Auth::new(config, state_manager_sender, Arc::new(RwLock::new(sui)));
        (auth, state_manager_receiver)
    }

    #[tokio::test]
    async fn test_access_token_regenerate() {
        let (auth, receiver) = setup_test().await;
        let user_id = 123;
        let refresh_token = auth.generate_refresh_token(user_id).await.unwrap();
        let refresh_token_hash = auth.hash_string(&refresh_token);
        let mock_handle = tokio::task::spawn(async move {
            let event = receiver.recv_async().await.unwrap();
            match event {
                AtomaAtomaStateManagerEvent::StoreRefreshToken { .. } => {}
                _ => panic!("Unexpected event"),
            }
            let event = receiver.recv_async().await.unwrap();
            match event {
                AtomaAtomaStateManagerEvent::IsRefreshTokenValid {
                    user_id: event_user_id,
                    refresh_token_hash: event_refresh_token,
                    result_sender,
                } => {
                    assert_eq!(event_user_id, user_id);
                    assert_eq!(refresh_token_hash, event_refresh_token);
                    result_sender.send(Ok(true)).unwrap();
                }
                _ => panic!("Unexpected event"),
            }
        });
        let access_token = auth.generate_access_token(&refresh_token).await.unwrap();
        let claims = auth.validate_token(&access_token, false).unwrap();
        assert_eq!(claims.user_id, user_id);
        assert!(claims.refresh_token_hash.is_some());
        if tokio::time::timeout(std::time::Duration::from_secs(1), mock_handle)
            .await
            .is_err()
        {
            panic!("mock_handle did not finish within 1 second");
        }
    }

    #[tokio::test]
    async fn test_token_flow() {
        let user_id = 123;
        let username = "user";
        let password = "top_secret";
        let (auth, receiver) = setup_test().await;
        let hash_password = auth.hash_string(password);
        let mock_handle = tokio::task::spawn(async move {
            // First event is for the user to log in to get the tokens
            let event = receiver.recv_async().await.unwrap();
            match event {
                AtomaAtomaStateManagerEvent::GetUserIdByUsernamePassword {
                    username: event_username,
                    password: event_password,
                    result_sender,
                } => {
                    assert_eq!(username, event_username);
                    assert_eq!(hash_password, event_password);
                    result_sender.send(Ok(Some(user_id))).unwrap();
                }
                _ => panic!("Unexpected event"),
            }
            let event = receiver.recv_async().await.unwrap();
            match event {
                AtomaAtomaStateManagerEvent::StoreRefreshToken { .. } => {}
                _ => panic!("Unexpected event"),
            }
            for _ in 0..2 {
                // During the token generation, the refresh token is checked for validity
                // 1) when the user logs in
                // 2) when the api token is generated
                let event = receiver.recv_async().await.unwrap();
                match event {
                    AtomaAtomaStateManagerEvent::IsRefreshTokenValid {
                        user_id: event_user_id,
                        refresh_token_hash: _refresh_token,
                        result_sender,
                    } => {
                        assert_eq!(event_user_id, user_id);
                        result_sender.send(Ok(true)).unwrap();
                    }
                    _ => panic!("Unexpected event"),
                }
            }
            // Last event is for storing the new api token
            let event = receiver.recv_async().await.unwrap();
            match event {
                AtomaAtomaStateManagerEvent::StoreNewApiToken {
                    user_id: event_user_id,
                    api_token: _api_token,
                } => {
                    assert_eq!(event_user_id, user_id);
                    // assert_eq!(event_api_token, api_token);
                }
                _ => panic!("Unexpected event"),
            }
        });
        let (refresh_token, access_token) =
            auth.check_user_password(username, password).await.unwrap();
        // Refresh token should not have refresh token hash
        let claims = auth.validate_token(&refresh_token, true).unwrap();
        assert_eq!(claims.user_id, user_id);
        assert_eq!(claims.refresh_token_hash, None);
        // Access token should have refresh token hash
        let claims = auth.validate_token(&access_token, false).unwrap();
        assert_eq!(claims.user_id, user_id);
        assert!(claims.refresh_token_hash.is_some());
        // Generate api token
        let _api_token = auth.generate_api_token(&access_token).await.unwrap();
        if tokio::time::timeout(std::time::Duration::from_secs(1), mock_handle)
            .await
            .is_err()
        {
            panic!("mock_handle did not finish within 1 second");
        }
    }
}
