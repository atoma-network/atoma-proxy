use std::path::Path;

use anyhow::Result;
use atoma_sui::{events::StackCreatedEvent, AtomaSuiConfig};
use blake2::{
    digest::generic_array::{typenum::U32, GenericArray},
    Blake2b, Digest,
};
use serde_json::Value;
use sui_keys::keystore::{AccountKeystore, Keystore};
use sui_sdk::{
    json::SuiJsonValue,
    rpc_types::Page,
    types::{
        base_types::{ObjectID, SuiAddress},
        crypto::EncodeDecodeBase64,
        digests::TransactionDigest,
        SUI_RANDOMNESS_STATE_OBJECT_ID,
    },
    wallet_context::WalletContext,
};
use tracing::{error, info, instrument};

const GAS_BUDGET: u64 = 5_000_000; // 0.005 SUI

/// Response returned when acquiring a new stack entry
///
/// This struct contains both the transaction digest of the stack entry creation
/// and the event data generated when the stack was created.
#[derive(Debug)]
pub struct StackEntryResponse {
    /// The transaction digest from the stack entry creation transaction
    pub transaction_digest: TransactionDigest,
    /// The event data emitted when the stack was created
    pub stack_created_event: StackCreatedEvent,
    /// Timestamp of the stack entry creation
    pub timestamp_ms: Option<u64>,
}

/// The Sui client
///
/// This struct is used to interact with the Sui contract.
pub struct Sui {
    /// Sui wallet context
    wallet_ctx: WalletContext,
    /// USDC wallet object ID
    usdc_wallet_id: Option<ObjectID>,
    /// Atoma package object ID
    atoma_package_id: ObjectID,
    /// Atoma DB object ID
    atoma_db_id: ObjectID,
    /// USDC package object ID
    usdc_package_id: ObjectID,
}

impl Sui {
    /// Constructor
    pub async fn new(sui_config: &AtomaSuiConfig) -> Result<Self> {
        let sui_config_path = sui_config.sui_config_path();
        let sui_config_path = Path::new(&sui_config_path);
        let wallet_ctx = WalletContext::new(
            sui_config_path,
            sui_config.request_timeout(),
            sui_config.max_concurrent_requests(),
        )?;

        Ok(Self {
            wallet_ctx,
            usdc_wallet_id: None,
            atoma_package_id: sui_config.atoma_package_id(),
            atoma_db_id: sui_config.atoma_db(),
            usdc_package_id: sui_config.usdc_package_id(),
        })
    }

    /// Get the wallet address
    ///
    /// # Returns
    ///
    /// Returns the wallet address.
    ///
    /// # Errors
    ///
    /// Returns an error if the wallet context fails to get the active address.
    pub fn get_wallet_address(&mut self) -> Result<SuiAddress> {
        self.wallet_ctx.active_address()
    }

    /// Acquire a new stack entry
    ///
    /// # Arguments
    ///
    /// * `task_small_id` - The task small ID for which to acquire a new stack entry.
    /// * `num_compute_units` - The number of compute units to acquire.
    /// * `price_per_one_million_compute_units` - The price per one million compute units.
    ///
    /// # Returns
    ///
    /// Returns the selected node ID.
    ///
    /// # Errors
    ///
    /// Returns an error if the transaction fails.
    #[instrument(level = "info", skip_all, fields(address = %self.wallet_ctx.active_address().unwrap()))]
    pub async fn acquire_new_stack_entry(
        &mut self,
        task_small_id: u64,
        num_compute_units: u64,
        price_per_one_million_compute_units: u64,
    ) -> Result<StackEntryResponse> {
        let client = self.wallet_ctx.get_client().await?;
        let address = self.wallet_ctx.active_address()?;
        let usdc_wallet_id = self.get_or_load_usdc_wallet_object_id().await?;

        let tx = client
            .transaction_builder()
            .move_call(
                address,
                self.atoma_package_id,
                "db",
                "acquire_new_stack_entry",
                vec![],
                vec![
                    SuiJsonValue::from_object_id(self.atoma_db_id),
                    SuiJsonValue::from_object_id(usdc_wallet_id),
                    SuiJsonValue::new(task_small_id.to_string().into())?,
                    SuiJsonValue::new(num_compute_units.to_string().into())?,
                    SuiJsonValue::new(price_per_one_million_compute_units.to_string().into())?,
                    SuiJsonValue::from_object_id(SUI_RANDOMNESS_STATE_OBJECT_ID),
                ],
                None,
                GAS_BUDGET,
                None,
            )
            .await?;

        info!("Submitting acquire new stack entry transaction...");
        let tx = self.wallet_ctx.sign_transaction(&tx);
        let response = self.wallet_ctx.execute_transaction_must_succeed(tx).await;

        info!(
            "Acquire new stack entry transaction submitted successfully. Transaction digest: {:?}",
            response.digest
        );
        let stack_created_event = response
            .events
            .and_then(|event| event.data.first().cloned())
            .ok_or_else(|| anyhow::anyhow!("No stack created event"))?
            .parsed_json;
        Ok(StackEntryResponse {
            transaction_digest: response.digest,
            stack_created_event: serde_json::from_value(stack_created_event.clone())?,
            timestamp_ms: response.timestamp_ms,
        })
    }

    /// Get or load the USDC wallet object ID
    ///
    /// This method checks if the USDC wallet object ID is already loaded and returns it if so.
    /// Otherwise, it loads the USDC wallet object ID by finding the most balance USDC coin for the active address.
    ///
    /// # Returns
    ///
    /// Returns the USDC wallet object ID.
    ///
    /// # Errors
    ///
    /// Returns an error if no USDC wallet is found for the active address.
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// let mut client = AtomaProxy::new(config).await?;
    /// let usdc_wallet_id = client.get_or_load_usdc_wallet_object_id().await?;
    /// ```
    #[instrument(level = "info", skip_all, fields(address = %self.wallet_ctx.active_address().unwrap()))]
    pub async fn get_or_load_usdc_wallet_object_id(&mut self) -> Result<ObjectID> {
        if let Some(usdc_wallet_id) = self.usdc_wallet_id {
            Ok(usdc_wallet_id)
        } else {
            let usdc_wallet = self.find_usdc_token_wallet(self.usdc_package_id).await;
            if let Ok(usdc_wallet) = usdc_wallet {
                self.usdc_wallet_id = Some(usdc_wallet);
                Ok(usdc_wallet)
            } else {
                anyhow::bail!("No USDC wallet found")
            }
        }
    }

    /// Sign the openai request.
    ///
    /// # Arguments
    ///
    /// * `request` - The openai request that needs to be signed.
    ///
    /// # Returns
    ///
    /// Returns the response from the OpenAI API.
    ///
    /// # Errors
    ///
    /// Returns an error if it fails to get the active address.
    #[instrument(level = "info", skip_all, fields(address = %self.wallet_ctx.active_address().unwrap()))]
    pub fn get_sui_signature(&mut self, request: &Value) -> Result<String> {
        let active_address = self.wallet_ctx.active_address()?;
        let mut blake2b = Blake2b::new();
        blake2b.update(request.to_string().as_bytes());
        let hash: GenericArray<u8, U32> = blake2b.finalize();
        let signature = match &self.wallet_ctx.config.keystore {
            Keystore::File(keystore) => keystore.sign_hashed(&active_address, &hash)?,
            Keystore::InMem(keystore) => keystore.sign_hashed(&active_address, &hash)?,
        };
        Ok(signature.encode_base64())
    }

    /// Sign a hash using the wallet's private key
    ///
    /// # Arguments
    ///
    /// * `hash` - The byte array to be signed
    ///
    /// # Returns
    ///
    /// Returns the base64-encoded signature string.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// * It fails to get the active address
    /// * The signing operation fails
    #[instrument(level = "info", skip_all, fields(address = %self.wallet_ctx.active_address().unwrap()))]
    pub fn sign_hash(&mut self, hash: &[u8]) -> Result<String> {
        let active_address = self.wallet_ctx.active_address()?;
        let signature = match &self.wallet_ctx.config.keystore {
            Keystore::File(keystore) => keystore.sign_hashed(&active_address, hash)?,
            Keystore::InMem(keystore) => keystore.sign_hashed(&active_address, hash)?,
        };
        Ok(signature.encode_base64())
    }

    /// Find the USDC token wallet for the given address
    ///
    /// # Returns
    ///
    /// Returns the USDC token wallet object ID.
    ///
    /// # Errors
    ///
    /// Returns an error if no USDC wallet is found for the active address.
    #[instrument(level = "info", skip_all, fields(address = %self.wallet_ctx.active_address().unwrap()))]
    async fn find_usdc_token_wallet(&mut self, usdc_package: ObjectID) -> Result<ObjectID> {
        let client = self.wallet_ctx.get_client().await?;
        let active_address = self.wallet_ctx.active_address()?;
        let Page { data: coins, .. } = client
            .coin_read_api()
            .get_coins(
                active_address,
                Some(format!("{usdc_package}::usdc::USDC")),
                None,
                None,
            )
            .await?;
        coins
            .into_iter()
            .max_by_key(|coin| coin.balance)
            .map(|coin| coin.coin_object_id)
            .ok_or_else(|| {
                error!("No USDC coins found for {active_address}");
                anyhow::anyhow!(
                    "No USDC coins for {active_address}. \
                    Have you just received them? \
                    It may take a few seconds for cache to refresh. \
                    Double check that your address owns USDC coins and try again."
                )
            })
    }
}
