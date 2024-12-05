use anyhow::Result;
use atoma_state::types::AtomaAtomaStateManagerEvent;
use blake2::{
    digest::{consts::U32, generic_array::GenericArray},
    Blake2b, Digest,
};
use chrono::{Duration, Utc};
use flume::Sender;
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use rand::Rng;
use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;

use crate::AtomaAuthConfig;

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
pub struct Auth {
    /// The secret key for JWT authentication.
    secret_key: String,
    /// The access token lifetime in minutes.
    access_token_lifetime: usize,
    /// The refresh token lifetime in days.
    refresh_token_lifetime: usize,
    /// The sender for the state manager
    state_manager_sender: Sender<AtomaAtomaStateManagerEvent>,
}

impl Auth {
    /// Constructor
    pub fn new(
        config: AtomaAuthConfig,
        state_manager_sender: Sender<AtomaAtomaStateManagerEvent>,
    ) -> Self {
        Self {
            secret_key: config.secret_key,
            access_token_lifetime: config.access_token_lifetime,
            refresh_token_lifetime: config.refresh_token_lifetime,
            state_manager_sender,
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
    fn generate_refresh_token(&self, user_id: i64) -> Result<String> {
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
    fn hash_string(&self, text: &str) -> String {
        let mut hasher = Blake2b::new();
        hasher.update(text);
        let hash_result: GenericArray<u8, U32> = hasher.finalize();
        hex::encode(hash_result)
    }

    /// Check the user password
    /// This method will check if the user password is correct
    /// The password is hashed and compared with the hashed password in the DB
    /// If the password is correct, the method will generate a new refresh and access token
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
        let refresh_token = self.generate_refresh_token(user_id as i64)?;
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
    pub async fn generate_api_token(&self, jwt: &str) -> Result<String> {
        let claims = self.validate_token(jwt, false)?;
        if !self
            .check_refresh_token_validity(
                claims.user_id,
                &claims
                    .refresh_token_hash
                    .expect("Access token should have refresh token hash"),
            )
            .await?
        {
            return Err(anyhow::anyhow!("Access token was revoked"));
        }
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
}

// TODO: Add more comprehensive tests, for now test the happy path only
#[cfg(test)]
mod test {
    use atoma_state::types::AtomaAtomaStateManagerEvent;
    use flume::Receiver;

    use crate::AtomaAuthConfig;

    use super::Auth;

    fn setup_test() -> (Auth, Receiver<AtomaAtomaStateManagerEvent>) {
        let config = AtomaAuthConfig::new("secret".to_string(), 1, 1);
        let (state_manager_sender, state_manager_receiver) = flume::unbounded();
        let auth = Auth::new(config, state_manager_sender);
        (auth, state_manager_receiver)
    }

    #[tokio::test]
    async fn test_access_token_regenerate() {
        let (auth, receiver) = setup_test();
        let user_id = 123;
        let refresh_token = auth.generate_refresh_token(user_id).unwrap();
        let refresh_token_hash = auth.hash_string(&refresh_token);
        let mock_handle = tokio::task::spawn(async move {
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
        let (auth, receiver) = setup_test();
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
