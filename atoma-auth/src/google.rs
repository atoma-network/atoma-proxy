use std::collections::{HashMap, HashSet};

use base64::{prelude::BASE64_URL_SAFE_NO_PAD, Engine};
use jsonwebtoken::{decode, decode_header, Algorithm, DecodingKey, Validation};
use reqwest::Client;
use rsa::{pkcs1::EncodeRsaPublicKey, BigUint, RsaPublicKey};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;
use tracing::instrument;

pub const ISS: &str = "https://accounts.google.com";
pub const JWKS_URL: &str = "https://www.googleapis.com/oauth2/v3/certs";
pub const KEYS: &str = "keys";
pub const KID: &str = "kid";
pub const N: &str = "n";
pub const E: &str = "e";

/// Claims struct for Google ID tokens
#[derive(Debug, Serialize, Deserialize)]
pub struct Claims {
    /// The issuer of the token
    iss: String,
    /// The subject of the token
    pub sub: String,
    /// The audience of the token
    aud: String,
    /// The expiration time of the token
    exp: usize,
    /// The email of the user
    pub email: Option<String>,
}

#[cfg(test)]
impl Claims {
    pub fn new(iss: &str, sub: &str, aud: &str, exp: usize, email: Option<&str>) -> Self {
        Self {
            iss: iss.to_string(),
            sub: sub.to_string(),
            aud: aud.to_string(),
            exp,
            email: email.map(|s| s.to_string()),
        }
    }
}

#[derive(Error, Debug)]
pub enum GoogleError {
    #[error("Base64 decode error: {0}")]
    Base64DecodeError(#[from] base64::DecodeError),
    #[error("RSA public key error: {0}")]
    RsaPublicKeyError(#[from] rsa::errors::Error),
    #[error("Decoding key error: {0}")]
    DecodingKeyError(#[from] jsonwebtoken::errors::Error),
    #[error("Request error: {0}")]
    RequestError(#[from] reqwest::Error),
    #[error("Serde JSON error: {0}")]
    SerdeJsonError(#[from] serde_json::Error),
    #[error("PEM error: {0}")]
    PemError(#[from] rsa::pkcs1::Error),
    #[error("Missing kid in token header")]
    MissingKid,
    #[error("Public keys malformed")]
    MalformedPublicKeys,
    #[error("Email not found in token claims")]
    EmailNotFound,
}

type Result<T> = std::result::Result<T, GoogleError>;

/// Convert a JWK to a DecodingKey
#[instrument(level = "debug")]
fn jwk_to_decoding_key(n: &str, e: &str) -> Result<DecodingKey> {
    // Decode `n` and `e` from Base64URL
    let n_bytes = BASE64_URL_SAFE_NO_PAD.decode(n)?;
    let e_bytes = BASE64_URL_SAFE_NO_PAD.decode(e)?;

    // Convert `n` and `e` into BigUint
    let n = BigUint::from_bytes_be(&n_bytes);
    let e = BigUint::from_bytes_be(&e_bytes);

    // Construct the RSA public key
    let public_key = RsaPublicKey::new(n, e)?;

    // Convert the RSA key to PEM format
    let pem = public_key.to_pkcs1_pem(rsa::pkcs1::LineEnding::LF)?;

    // Create the DecodingKey
    let decoding_key = DecodingKey::from_rsa_pem(pem.as_bytes())?;

    Ok(decoding_key)
}

/// Fetch Google's public keys from the JWKS endpoint
#[instrument(level = "debug")]
pub async fn fetch_google_public_keys() -> Result<HashMap<String, (DecodingKey, Algorithm)>> {
    let client = Client::new();
    let res = client.get(JWKS_URL).send().await?;
    let jwks: Value = res.json().await?;
    let keys = jwks
        .get(KEYS)
        .and_then(Value::as_array)
        .ok_or(GoogleError::MalformedPublicKeys)?;

    let mut public_keys = HashMap::new();
    for key in keys {
        let kid = key
            .get(KID)
            .and_then(Value::as_str)
            .ok_or(GoogleError::MalformedPublicKeys)?
            .to_string();
        let n = key
            .get(N)
            .and_then(Value::as_str)
            .ok_or(GoogleError::MalformedPublicKeys)?
            .to_string();
        let e = key
            .get(E)
            .and_then(Value::as_str)
            .ok_or(GoogleError::MalformedPublicKeys)?
            .to_string();
        public_keys.insert(kid, (jwk_to_decoding_key(&n, &e)?, Algorithm::RS256));
    }

    Ok(public_keys)
}

/// Verify a Google ID token
///
/// # Arguments
///
/// * `id_token` - The ID token to verify
/// * `audience` - The expected audience of the token
///
/// # Returns
///
/// A `Result` containing the verified claims if successful, or an error if verification failed
#[instrument(level = "debug", skip(public_keys))]
pub fn verify_google_id_token(
    id_token: &str,
    audience: &str,
    public_keys: &HashMap<String, (DecodingKey, Algorithm)>,
) -> Result<Claims> {
    // Decode the header to extract the kid
    let header = decode_header(id_token)?;
    let kid = header.kid.ok_or(GoogleError::MissingKid)?;

    // Find the matching public key
    let (decoding_key, algorithm) = public_keys.get(&kid).ok_or(GoogleError::DecodingKeyError(
        jsonwebtoken::errors::Error::from(jsonwebtoken::errors::ErrorKind::InvalidToken),
    ))?;

    // Set validation rules
    let mut validation = Validation::new(*algorithm);
    validation.set_audience(&[audience]);
    let mut issuers = HashSet::new();
    issuers.insert(ISS.to_string());
    validation.iss = Some(issuers);

    // Verify the token
    let token_data = decode::<Claims>(id_token, decoding_key, &validation)?;

    Ok(token_data.claims)
}
