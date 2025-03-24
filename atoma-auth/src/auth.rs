#[cfg(feature = "google-oauth")]
use std::collections::HashMap;
use std::{str::FromStr, sync::Arc};

#[cfg(feature = "google-oauth")]
use crate::google::{self, fetch_google_public_keys};
use crate::{AtomaAuthConfig, Sui};
use anyhow::anyhow;
use atoma_state::{
    types::{AtomaAtomaStateManagerEvent, TokenResponse, UserProfile},
    AtomaStateManagerError,
};
use atoma_utils::hashing::blake2b_hash;
use blake2::{
    digest::{consts::U32, generic_array::GenericArray},
    Blake2b, Digest,
};
use chrono::{Duration, Utc};
use fastcrypto::{
    ed25519::{Ed25519PublicKey, Ed25519Signature},
    error::FastCryptoError,
    secp256k1::{Secp256k1PublicKey, Secp256k1Signature},
    secp256r1::{Secp256r1PublicKey, Secp256r1Signature},
    traits::{ToFromBytes, VerifyingKey},
};
use fastcrypto_zkp::zk_login_utils::Bn254FrElement;
use flume::Sender;
#[cfg(feature = "google-oauth")]
use jsonwebtoken::Algorithm;
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use rand::Rng;
use regex::Regex;
use serde::{Deserialize, Serialize};
use shared_crypto::intent::{Intent, IntentMessage, PersonalMessage};
use sui_sdk::types::crypto::ZkLoginPublicIdentifier;
use sui_sdk::types::{
    base_types::SuiAddress,
    crypto::{PublicKey, SignatureScheme},
    error::SuiError,
    object::Owner,
    TypeTag,
};
use sui_sdk_types::{SimpleSignature, UserSignature};
use thiserror::Error;
use tokio::sync::{oneshot, RwLock};
use tracing::{error, instrument};

/// The length of the API token
const API_TOKEN_LENGTH: usize = 30;

const SUI_BALANCE_RETRY_COUNT: usize = 5; // How many times to retry the Sui call for the balance
const SUI_BALANCE_RETRY_PAUSE: u64 = 500; // In milliseconds

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

#[derive(Error, Debug)]
pub enum AuthError {
    #[error("Json Web Token error: {0}")]
    JsonWebTokenError(#[from] jsonwebtoken::errors::Error),
    #[error("Flume error: {0}")]
    FlumeError(#[from] flume::SendError<AtomaAtomaStateManagerEvent>),
    #[error("Token is not refresh token")]
    NotRefreshToken,
    #[error("Error: {0}")]
    OneShotReceiveError(#[from] tokio::sync::oneshot::error::RecvError),
    #[error("AtomaStateManagerError: {0}")]
    AtomaStateManagerError(#[from] AtomaStateManagerError),
    #[error("Refresh token is not valid")]
    InvalidRefreshToken,
    #[error("Reqwest error: {0}")]
    ReqwestError(#[from] reqwest::Error),
    #[error("User already registered")]
    UserAlreadyRegistered,
    #[error("Password not valid or user not found")]
    PasswordNotValidOrUserNotFound,
    #[error("Revoked token")]
    RevokedToken,
    #[error("Failed to parse signature: {0}")]
    FailedToParseSignature(String),
    #[error("Error: {0}")]
    BcsError(#[from] bcs::Error),
    #[error("Error: {0}")]
    FastCryptoError(#[from] FastCryptoError),
    #[error("Unsupported signature scheme {0}")]
    UnsupportedSignatureScheme(SignatureScheme),
    #[error("Error: {0}")]
    AnyhowError(#[from] anyhow::Error),
    #[error("No balance changes found")]
    NoBalanceChangesFound,
    #[error("Multiple senders")]
    MultipleSenders,
    #[error("Multiple receivers")]
    MultipleReceivers,
    #[error("Sender or receiver not found")]
    SenderOrReceiverNotFound,
    #[error("The payment is not for this user")]
    PaymentNotForThisUser,
    #[cfg(feature = "google-oauth")]
    #[error("Google error: {0}")]
    GoogleError(#[from] crate::google::GoogleError),
    #[cfg(not(feature = "google-oauth"))]
    #[error("ZkLogin not enabled")]
    ZkLoginNotEnabled,
    #[error("Sui error: {0}")]
    SuiError(#[from] SuiError),
    #[error("Failed to convert integer: {0}")]
    IntConversionError(#[from] std::num::TryFromIntError),
    #[error("Failed to convert timestamp")]
    TimestampConversionError,
}

type Result<T> = std::result::Result<T, AuthError>;
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
    #[cfg(feature = "google-oauth")]
    /// GooglePublicKeys
    google_public_keys: HashMap<String, (DecodingKey, Algorithm)>,
    #[cfg(feature = "google-oauth")]
    /// Google client id
    google_client_id: String,
}

impl Auth {
    /// Constructor for the Auth struct.
    ///
    /// # Arguments
    /// * `config` - Authentication configuration
    /// * `state_manager_sender` - Channel for sending state manager events
    /// * `sui` - Sui blockchain client instance
    ///
    /// # Errors
    /// Returns an error if:
    /// - Failed to fetch Google public keys (when google-oauth feature is enabled)
    #[allow(clippy::unused_async)]
    pub async fn new(
        config: AtomaAuthConfig,
        state_manager_sender: Sender<AtomaAtomaStateManagerEvent>,
        sui: Arc<RwLock<Sui>>,
    ) -> Result<Self> {
        #[cfg(feature = "google-oauth")]
        let google_public_keys = fetch_google_public_keys().await?;
        Ok(Self {
            secret_key: config.secret_key,
            access_token_lifetime: config.access_token_lifetime,
            refresh_token_lifetime: config.refresh_token_lifetime,
            state_manager_sender,
            sui,
            #[cfg(feature = "google-oauth")]
            google_public_keys,
            #[cfg(feature = "google-oauth")]
            google_client_id: config.google_client_id,
        })
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
            exp: usize::try_from(expiration.timestamp())
                .map_err(|_| AuthError::TimestampConversionError)
                .unwrap(),
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
            return Err(AuthError::NotRefreshToken);
        }
        Ok(claims)
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
            return Err(AuthError::InvalidRefreshToken);
        }
        let expiration = Utc::now() + Duration::days(self.access_token_lifetime as i64);

        let claims = Claims {
            user_id: claims.user_id,
            exp: usize::try_from(expiration.timestamp())
                .map_err(|_| AuthError::TimestampConversionError)
                .unwrap(),
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
    #[must_use]
    pub fn hash_string(&self, text: &str) -> String {
        let mut hasher = Blake2b::new();
        hasher.update(text);
        let hash_result: GenericArray<u8, U32> = hasher.finalize();
        hex::encode(hash_result)
    }

    /// Register user with email/password.
    /// This method will register a new user with a email and password
    /// The password is hashed and stored in the DB
    /// The method will generate a new refresh and access token
    #[instrument(level = "info", skip(self, password))]
    pub async fn register(
        &self,
        user_profile: &UserProfile,
        password: &str,
    ) -> Result<(String, String)> {
        let (result_sender, result_receiver) = oneshot::channel();
        let password_salt = rand::thread_rng()
            .sample_iter(&rand::distributions::Alphanumeric)
            .take(30)
            .map(char::from)
            .collect::<String>();

        self.state_manager_sender
            .send(AtomaAtomaStateManagerEvent::RegisterUserWithPassword {
                user_profile: user_profile.clone(),
                password: self.hash_string(&format!("{password_salt}:{password}")),
                password_salt,
                result_sender,
            })?;
        let user_id = result_receiver
            .await??
            .map(|user_id| user_id as u64)
            .ok_or_else(|| AuthError::UserAlreadyRegistered)?;
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
        email: &str,
        password: &str,
    ) -> Result<(String, String)> {
        let (result_sender, result_receiver) = oneshot::channel();
        self.state_manager_sender
            .send(AtomaAtomaStateManagerEvent::GetPasswordSalt {
                email: email.to_string(),
                result_sender,
            })?;
        let password_salt = result_receiver.await??;

        let password_salt =
            password_salt.ok_or_else(|| AuthError::PasswordNotValidOrUserNotFound)?;

        let (result_sender, result_receiver) = oneshot::channel();
        self.state_manager_sender
            .send(AtomaAtomaStateManagerEvent::GetUserIdByEmailPassword {
                email: email.to_string(),
                password: self.hash_string(&format!("{password_salt}:{password}")),
                result_sender,
            })?;
        let user_id = result_receiver
            .await??
            .map(|user_id| user_id as u64)
            .ok_or_else(|| AuthError::PasswordNotValidOrUserNotFound)?;
        let refresh_token = self.generate_refresh_token(user_id as i64).await?;
        let access_token = self.generate_access_token(&refresh_token).await?;
        Ok((refresh_token, access_token))
    }

    /// Check the google oauth token
    /// This method will check the google oauth token and generate a new refresh and access token
    /// The method will check if the email is present in the claims and store the user in the DB
    /// The method will generate a new refresh and access tokens
    #[cfg(feature = "google-oauth")]
    #[instrument(level = "info", skip(self))]
    pub async fn check_google_id_token(&self, id_token: &str) -> Result<(String, String)> {
        let claims = google::verify_google_id_token(
            id_token,
            &self.google_client_id,
            &self.google_public_keys,
        )?;

        let (result_sender, result_receiver) = oneshot::channel();
        let email = match claims.email {
            Some(email) => email,
            None => {
                return Err(google::GoogleError::EmailNotFound)?;
            }
        };
        self.state_manager_sender
            .send(AtomaAtomaStateManagerEvent::OAuth {
                email,
                result_sender,
            })?;
        let user_id = result_receiver.await??;
        let refresh_token = self.generate_refresh_token(user_id).await?;
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
    /// * `name` - The name of the API token
    ///
    /// # Returns
    ///
    /// * `Result<String>` - The generated API token
    #[instrument(level = "info", skip(self))]
    pub async fn generate_api_token(&self, jwt: &str, name: String) -> Result<String> {
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
                name,
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
    pub async fn revoke_api_token(&self, jwt: &str, api_token_id: i64) -> Result<()> {
        let claims = self.get_claims_from_token(jwt).await?;
        self.state_manager_sender
            .send(AtomaAtomaStateManagerEvent::RevokeApiToken {
                user_id: claims.user_id,
                api_token_id,
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
    pub async fn get_all_api_tokens(&self, jwt: &str) -> Result<Vec<TokenResponse>> {
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
            return Err(AuthError::RevokedToken);
        }
        Ok(claims)
    }

    /// Get the user id from the token.
    /// This method will get the user id from the token
    /// and check if the token is valid.
    ///
    /// # Arguments
    /// * `jwt` - The access token to be used to get the user id
    ///
    /// # Returns
    /// * `Result<i64>` - The user id from the token
    ///
    /// # Errors
    /// Returns an error if:
    /// - The JWT token is invalid or expired
    /// - The token's claims cannot be decoded
    /// - The refresh token is invalid or revoked
    /// - The token is not an access token
    pub async fn get_user_id_from_token(&self, jwt: &str) -> Result<i64> {
        let claims = self.get_claims_from_token(jwt).await?;
        Ok(claims.user_id)
    }

    /// Generate a new personal message from a string.
    fn personal_message_hash(msg: &str) -> Result<GenericArray<u8, U32>> {
        let personal_message = IntentMessage::new(
            Intent::personal_message(),
            PersonalMessage {
                message: msg.as_bytes().to_vec(),
            },
        );
        let intent_bcs = bcs::to_bytes(&personal_message)?;
        Ok(blake2b_hash(&intent_bcs))
    }

    /// NOTE: These function are copied (with comments) from the original implementation in `sui-sdk` crate. https://github.com/MystenLabs/sui-rust-sdk/blob/master/crates/sui-crypto/src/zklogin/mod.rs#L142
    /// I just changed the errors to match ours.
    /// If they publish them as a public function, we can use them directly. But for now this is the only way how to get the ISS from the signature.
    ///
    /// Map a base64 string to a bit array by taking each char's index and convert it to binary form with one bit per u8
    /// element in the output. Returns SignatureError if one of the characters is not in the base64 charset.
    fn base64_to_bitarray(input: &str) -> Result<Vec<u8>> {
        use itertools::Itertools;
        const BASE64_URL_CHARSET: &str =
            "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

        input
            .chars()
            .map(|c| {
                BASE64_URL_CHARSET
                    .find(c)
                    .map(|index| u8::try_from(index).map_err(AuthError::IntConversionError))
                    .unwrap()
                    .map(|index| (0..6).rev().map(move |i| index >> i & 1))
            })
            .flatten_ok()
            .collect()
    }

    /// Convert a bitarray (each bit is represented by a u8) to a byte array by taking each 8 bits as a
    /// byte in big-endian format.
    fn bitarray_to_bytearray(bits: &[u8]) -> Result<Vec<u8>> {
        if bits.len() % 8 != 0 {
            return Err(anyhow!("bitarray_to_bytearray invalid input"))?;
        }
        Ok(bits
            .chunks(8)
            .map(|chunk| {
                let mut byte = 0u8;
                for (i, bit) in chunk.iter().rev().enumerate() {
                    byte |= bit << i;
                }
                byte
            })
            .collect())
    }

    /// Parse the base64 string, add paddings based on offset, and convert to a bytearray.
    fn decode_base64_url(s: &str, index_mod_4: u8) -> Result<String> {
        if s.len() < 2 {
            return Err(anyhow!("Base64 string smaller than 2"))?;
        }
        let mut bits = Self::base64_to_bitarray(s)?;
        match index_mod_4 {
            0 => {}
            1 => {
                bits.drain(..2);
            }
            2 => {
                bits.drain(..4);
            }
            _ => {
                return Err(anyhow!("Invalid first_char_offset"))?;
            }
        }

        let last_char_offset = (index_mod_4
            + u8::try_from(s.len())
                .map_err(|e| AuthError::AnyhowError(anyhow!("Failed to convert length: {e}")))
                .unwrap()
            - 1)
            % 4;
        match last_char_offset {
            3 => {}
            2 => {
                bits.drain(bits.len() - 2..);
            }
            1 => {
                bits.drain(bits.len() - 4..);
            }
            _ => {
                return Err(anyhow!("Invalid last_char_offset"))?;
            }
        }

        if bits.len() % 8 != 0 {
            return Err(anyhow!("Invalid bits length"))?;
        }

        Ok(std::str::from_utf8(&Self::bitarray_to_bytearray(&bits)?)
            .map_err(|_| anyhow!("Invalid UTF8 string"))?
            .to_owned())
    }

    /// Get Sui address from signature. We do support zk_login and simple signatures.
    /// The signature has to be for the message.
    ///
    /// # Arguments
    ///
    /// * `signature` - The signature to be used to get the Sui address
    /// * `message` - The message to be used check the signature
    ///
    /// # Returns
    ///
    /// * `Result<SuiAddress>` - The Sui address from the signature
    ///
    /// # Errors
    ///
    /// * If the signature is not valid
    /// * If the signature is not supported
    /// * If the public key is not found
    /// * If the public key is not verified
    #[instrument(level = "info")]
    fn get_sui_address_from_signature(signature: &str, message: &str) -> Result<SuiAddress> {
        let user_signature = UserSignature::from_base64(signature).map_err(|e| {
            error!("Failed to parse signature: {}", e);
            AuthError::FailedToParseSignature(signature.to_string())
        })?;
        let message_hash = Self::personal_message_hash(message)?;

        let (signature, mut public_key) = match user_signature {
            UserSignature::ZkLogin(zk_login_authenticator) => {
                let address_seed = Bn254FrElement::from_str(
                    &zk_login_authenticator.inputs.address_seed.to_string(),
                )?;
                let iss = Self::decode_base64_url(
                    &zk_login_authenticator.inputs.iss_base64_details.value,
                    zk_login_authenticator.inputs.iss_base64_details.index_mod_4,
                )?;
                let re = Regex::new(r#""iss":"(.*)".*"#).unwrap();
                let captures = re.captures(&iss).unwrap();
                let iss = captures
                    .get(1)
                    .ok_or_else(|| AuthError::FailedToParseSignature("No iss".to_string()))?
                    .as_str();
                let public_id = ZkLoginPublicIdentifier::new(iss, &address_seed)?;
                let public_key = PublicKey::ZkLogin(public_id);
                (zk_login_authenticator.signature, Some(public_key))
            }
            UserSignature::Simple(simple_signature) => (simple_signature, None),
            _ => {
                return Err(AuthError::FailedToParseSignature(
                    "Unsupported signature".to_string(),
                ))
            }
        };
        match signature {
            SimpleSignature::Ed25519 {
                signature,
                public_key: pk,
            } => {
                let pk = Ed25519PublicKey::from_bytes(pk.as_ref())?;
                let signature = Ed25519Signature::from_bytes(signature.as_ref())?;
                pk.verify(message_hash.as_slice(), &signature)?;
                if public_key.is_none() {
                    public_key = Some(
                        PublicKey::try_from_bytes(SignatureScheme::ED25519, pk.as_bytes()).unwrap(),
                    );
                }
            }
            SimpleSignature::Secp256k1 {
                signature,
                public_key: pk,
            } => {
                let pk = Secp256k1PublicKey::from_bytes(pk.as_ref())?;
                let signature = Secp256k1Signature::from_bytes(signature.as_ref())?;
                pk.verify(message_hash.as_slice(), &signature)?;
                if public_key.is_none() {
                    public_key = Some(
                        PublicKey::try_from_bytes(SignatureScheme::Secp256k1, pk.as_bytes())
                            .unwrap(),
                    );
                }
            }
            SimpleSignature::Secp256r1 {
                signature,
                public_key: pk,
            } => {
                let pk = Secp256r1PublicKey::from_bytes(pk.as_ref())?;
                let signature = Secp256r1Signature::from_bytes(signature.as_ref())?;
                pk.verify(message_hash.as_slice(), &signature)?;
                if public_key.is_none() {
                    public_key = Some(
                        PublicKey::try_from_bytes(SignatureScheme::Secp256r1, pk.as_bytes())
                            .unwrap(),
                    );
                }
            }
        };
        let sui_address = SuiAddress::from(
            &public_key
                .ok_or_else(|| AuthError::FailedToParseSignature("No public key".to_string()))?,
        );
        Ok(sui_address)
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
        let sui_address = Self::get_sui_address_from_signature(
            signature,
            &format!(
                "Sign this message to prove you are the owner of this wallet. User ID: {}",
                claims.user_id
            ),
        )?;

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
    /// * `proof_signature` - The proof signature is present for the zk_login signature. We don't store the zk address in the DB, the user confirms every usdc
    ///                       transaction by signing the hash of the transaction digest with their zk_login signature. This is used to confirm that the payment is for this user.
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
    /// * If the user provided invalid zk_proof_signature
    #[instrument(level = "info", skip(self))]
    pub async fn usdc_payment(
        &self,
        jwt: &str,
        transaction_digest: &str,
        zk_proof_signature: Option<String>,
    ) -> Result<()> {
        let claims = self.validate_token(jwt, false)?;

        let (result_sender, result_receiver) = oneshot::channel();
        self.state_manager_sender.send(
            AtomaAtomaStateManagerEvent::InsertNewUsdcPaymentDigest {
                digest: transaction_digest.to_string(),
                result_sender,
            },
        )?;
        result_receiver.await??;
        let mut balance_changes = Err(anyhow!("No balance changes found"));
        for _ in 0..SUI_BALANCE_RETRY_COUNT {
            balance_changes = self
                .sui
                .read()
                .await
                .get_balance_changes(transaction_digest)
                .await;
            if balance_changes.is_ok() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(SUI_BALANCE_RETRY_PAUSE)).await;
        }
        let balance_changes = balance_changes?.ok_or_else(|| AuthError::NoBalanceChangesFound)?;
        let mut sender = None;
        let mut receiver = None;
        let mut money_in = None;
        for balance_change in balance_changes {
            if let TypeTag::Struct(tag) = balance_change.coin_type {
                if tag.address.to_hex() == self.sui.read().await.usdc_package_id.to_hex() {
                    if balance_change.amount < 0 {
                        if sender.is_some() {
                            return Err(AuthError::MultipleSenders);
                        }
                        if let Owner::AddressOwner(owner) = &balance_change.owner {
                            sender = Some(*owner);
                        }
                    } else {
                        if receiver.is_some() {
                            return Err(AuthError::MultipleReceivers);
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
            return Err(AuthError::SenderOrReceiverNotFound);
        }
        let sender = sender.unwrap();
        let receiver = receiver.unwrap();
        let own_address = self.sui.write().await.get_wallet_address()?;
        if receiver == own_address {
            // The signature is coming from the frontend where the user used his zk credentials to sign the transaction digest as a personal message (digest of the transaction).
            // Now we need to prove that it was the digest he is trying to claim. And if the sui address from the signature is matching the address in the usdc payment transaction.
            match zk_proof_signature {
                #[cfg(feature = "google-oauth")]
                Some(signature) => {
                    if sender.to_string()
                        != Self::get_sui_address_from_signature(&signature, transaction_digest)?
                            .to_string()
                    {
                        return Err(AuthError::PaymentNotForThisUser);
                    }
                }
                #[cfg(not(feature = "google-oauth"))]
                Some(_) => {
                    return Err(AuthError::ZkLoginNotEnabled);
                }
                None => {
                    let (result_sender, result_receiver) = oneshot::channel();
                    self.state_manager_sender
                        .send(AtomaAtomaStateManagerEvent::ConfirmUser {
                            sui_address: sender.to_string(),
                            user_id: claims.user_id,
                            result_sender,
                        })?;
                    let is_their_wallet = result_receiver.await??;

                    if !is_their_wallet {
                        return Err(AuthError::PaymentNotForThisUser);
                    }
                }
            }

            // We are the receiver and we know the sender
            self.state_manager_sender
                .send(AtomaAtomaStateManagerEvent::TopUpBalance {
                    user_id: claims.user_id,
                    amount: i64::try_from(money_in.unwrap()).map_err(|e| {
                        AuthError::AnyhowError(anyhow!("Failed to convert amount: {e}"))
                    })?,
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
    use atoma_sui::config::Config;
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
            sui_config_path.to_str().unwrap().replace('\\', "\\\\"),
            sui_keystore_path.to_str().unwrap().replace('\\', "\\\\")
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
                    sui_keystore_path.to_str().unwrap().replace('\\', "\\\\")
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
        let config = AtomaAuthConfig::new(
            "secret".to_string(),
            1,
            1,
            #[cfg(feature = "google-oauth")]
            "google_client_id".to_string(),
        );
        let (state_manager_sender, state_manager_receiver) = flume::unbounded();

        let sui_config = Config::from_file_path(get_config_path());
        let sui = crate::Sui::new(&sui_config).unwrap();
        let auth = Auth::new(config, state_manager_sender, Arc::new(RwLock::new(sui)))
            .await
            .unwrap();
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
        let email = "email";
        let salt = "salt";
        let password = "top_secret";
        let (auth, receiver) = setup_test().await;
        let hash_password = auth.hash_string(&format!("{salt}:{password}"));
        let mock_handle = tokio::task::spawn(async move {
            // First event is for the user to log in to get the tokens
            let event = receiver.recv_async().await.unwrap();
            match event {
                AtomaAtomaStateManagerEvent::GetPasswordSalt {
                    email: event_email,
                    result_sender,
                } => {
                    assert_eq!(email, event_email);
                    result_sender.send(Ok(Some(salt.to_string()))).unwrap();
                }
                _ => panic!("Unexpected event"),
            }
            let event = receiver.recv_async().await.unwrap();
            match event {
                AtomaAtomaStateManagerEvent::GetUserIdByEmailPassword {
                    email: event_email,
                    password: event_password,
                    result_sender,
                } => {
                    assert_eq!(email, event_email);
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
                    name: _name,
                } => {
                    assert_eq!(event_user_id, user_id);
                    // assert_eq!(event_api_token, api_token);
                }
                _ => panic!("Unexpected event"),
            }
        });
        let (refresh_token, access_token) =
            auth.check_user_password(email, password).await.unwrap();
        // Refresh token should not have refresh token hash
        let claims = auth.validate_token(&refresh_token, true).unwrap();
        assert_eq!(claims.user_id, user_id);
        assert_eq!(claims.refresh_token_hash, None);
        // Access token should have refresh token hash
        let claims = auth.validate_token(&access_token, false).unwrap();
        assert_eq!(claims.user_id, user_id);
        assert!(claims.refresh_token_hash.is_some());
        // Generate api token
        let _api_token = auth
            .generate_api_token(&access_token, "test".to_string())
            .await
            .unwrap();
        if tokio::time::timeout(std::time::Duration::from_secs(1), mock_handle)
            .await
            .is_err()
        {
            panic!("mock_handle did not finish within 1 second");
        }
    }

    #[cfg(feature = "google-oauth")]
    #[tokio::test]
    async fn google_login() {
        use crate::google::{Claims, ISS};
        use chrono::Utc;
        use jsonwebtoken::{encode, Algorithm, DecodingKey, EncodingKey, Header};
        let (mut auth, receiver) = setup_test().await;
        let mock_handle = tokio::task::spawn(async move {
            // First event is for the user to log in to get the tokens
            let event = receiver.recv_async().await.unwrap();
            match event {
                AtomaAtomaStateManagerEvent::OAuth {
                    email: event_email,
                    result_sender,
                } => {
                    assert_eq!(event_email, "email");
                    result_sender.send(Ok(1)).unwrap();
                }
                _ => panic!("Unexpected event"),
            }
            let event = receiver.recv_async().await.unwrap();
            match event {
                AtomaAtomaStateManagerEvent::StoreRefreshToken { .. } => {}
                _ => panic!("Unexpected event"),
            }
            let event = receiver.recv_async().await.unwrap();
            match event {
                AtomaAtomaStateManagerEvent::IsRefreshTokenValid {
                    user_id: event_user_id,
                    refresh_token_hash: _refresh_token,
                    result_sender,
                } => {
                    assert_eq!(event_user_id, 1);
                    result_sender.send(Ok(true)).unwrap();
                }
                _ => panic!("Unexpected event"),
            }
        });
        let encoding_key = EncodingKey::from_secret("fake secret".as_bytes());
        auth.google_public_keys.insert(
            "kid".to_string(),
            (
                DecodingKey::from_secret("fake secret".as_bytes()),
                Algorithm::HS256,
            ),
        );
        let header = Header {
            kid: Some("kid".to_string()),
            alg: Algorithm::HS256,
            ..Default::default()
        };
        let id_token = encode(
            &header,
            &Claims::new(
                ISS,
                "sub",
                "google_client_id",
                Utc::now().timestamp() as usize + 1000,
                Some("email"),
            ),
            &encoding_key,
        )
        .unwrap();
        auth.check_google_id_token(&id_token).await.unwrap();
        if tokio::time::timeout(std::time::Duration::from_secs(1), mock_handle)
            .await
            .is_err()
        {
            panic!("mock_handle did not finish within 1 second");
        }
    }
}
