use std::path::Path;

use anyhow::Result;
use atoma_sui::AtomaSuiConfig;
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
        SUI_RANDOMNESS_STATE_OBJECT_ID,
    },
    wallet_context::WalletContext,
    SuiClient,
};
use tracing::{info, instrument};

const GAS_BUDGET: u64 = 5_000_000; // 0.005 SUI

pub struct Sui {
    wallet_ctx: WalletContext,
    toma_wallet_id: Option<ObjectID>,
    atoma_package_id: ObjectID,
    atoma_db_id: ObjectID,
    toma_package_id: ObjectID,
}

impl Sui {
    /// Constructor
    pub async fn new(sui_config: &AtomaSuiConfig) -> Result<Self> {
        // let keystore = FileBasedKeystore::new(&config.sui.sui_keystore_path().into())
        //     .context("Failed to initialize keystore")
        //     .expect("Failed to initialize keystore");
        let sui_config_path = sui_config.sui_config_path();
        let sui_config_path = Path::new(&sui_config_path);
        let wallet_ctx = WalletContext::new(
            sui_config_path,
            sui_config.request_timeout(),
            sui_config.max_concurrent_requests(),
        )?;

        Ok(Self {
            wallet_ctx,
            toma_wallet_id: None,
            atoma_package_id: sui_config.atoma_package_id(),
            atoma_db_id: sui_config.atoma_db(),
            toma_package_id: sui_config.toma_package_id(),
        })
    }

    #[instrument(level = "info", skip_all, fields(
      endpoint = "acquire_new_stack_entry",
      address = %self.wallet_ctx.active_address().unwrap()
    ))]
    pub async fn acquire_new_stack_entry(
        &mut self,
        task_small_id: u64,
        num_compute_units: u64,
        price: u64,
    ) -> Result<i64> {
        let client = self.wallet_ctx.get_client().await?;
        let address = self.wallet_ctx.active_address()?;
        let toma_wallet_id = self.get_or_load_toma_wallet_object_id().await?;

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
                    SuiJsonValue::from_object_id(toma_wallet_id),
                    SuiJsonValue::new(task_small_id.to_string().into())?,
                    SuiJsonValue::new(num_compute_units.to_string().into())?,
                    SuiJsonValue::new(price.to_string().into())?,
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
        response
            .events
            .and_then(|event| {
                event.data.get(0).and_then(|event| {
                    event
                        .parsed_json
                        .get("selected_node_id")
                        .and_then(|node_id| node_id.as_i64())
                })
            })
            .ok_or_else(|| anyhow::anyhow!("No node was selected"))
    }

    /// Get or load the TOMA wallet object ID
    ///
    /// This method checks if the TOMA wallet object ID is already loaded and returns it if so.
    /// Otherwise, it loads the TOMA wallet object ID by finding the most balance TOMA coin for the active address.
    ///
    /// # Returns
    ///
    /// Returns the TOMA wallet object ID.
    ///
    /// # Errors
    ///
    /// Returns an error if no TOMA wallet is found for the active address.
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// let mut client = AtomaProxy::new(config).await?;
    /// let toma_wallet_id = client.get_or_load_toma_wallet_object_id().await?;
    /// ```
    #[instrument(level = "info", skip_all, fields(
      endpoint = "get_or_load_toma_wallet_object_id",
      address = %self.wallet_ctx.active_address().unwrap()
    ))]
    pub async fn get_or_load_toma_wallet_object_id(&mut self) -> Result<ObjectID> {
        if let Some(toma_wallet_id) = self.toma_wallet_id {
            Ok(toma_wallet_id)
        } else {
            let toma_wallet = self.find_toma_token_wallet(self.toma_package_id).await;
            if let Ok(toma_wallet) = toma_wallet {
                self.toma_wallet_id = Some(toma_wallet);
                Ok(toma_wallet)
            } else {
                anyhow::bail!("No TOMA wallet found")
            }
        }
    }

    /// Send a request to an OpenAI API node
    ///
    /// # Arguments
    ///
    /// * `request` - The openai request to send to the OpenAI API.
    /// * `endpoint` - The endpoint to send the request to.
    ///
    /// # Returns
    ///
    /// Returns the response from the OpenAI API.
    ///
    /// # Errors
    ///
    /// Returns an error if the request to the OpenAI API fails.
    #[instrument(level = "info", skip_all, fields(
      endpoint = "send_openai_api_request",
      address = %self.wallet_ctx.active_address().unwrap()
  ))]
    pub async fn send_openai_api_request(
        &mut self,
        request: Value,
        node_address: String,
    ) -> Result<Value> {
        let active_address = self.wallet_ctx.active_address()?;
        let mut blake2b = Blake2b::new();
        blake2b.update(request.to_string().as_bytes());
        let hash: GenericArray<u8, U32> = blake2b.finalize();
        let signature = match &self.wallet_ctx.config.keystore {
            Keystore::File(keystore) => keystore.sign_hashed(&active_address, &hash)?,
            Keystore::InMem(keystore) => keystore.sign_hashed(&active_address, &hash)?,
        };
        let base64_signature = signature.encode_base64();

        let client = reqwest::Client::new();
        let response = client
            .post(&node_address)
            .header("X-Signature", base64_signature)
            .json(&request)
            .send()
            .await?;
        Ok(response.json::<Value>().await?)
    }

    /// Find the TOMA token wallet for the given address
    ///
    /// # Returns
    ///
    /// Returns the TOMA token wallet object ID.
    ///
    /// # Errors
    ///
    /// Returns an error if no TOMA wallet is found for the active address.
    #[instrument(level = "info", skip_all, fields(
      endpoint = "find_toma_token_wallet",
      address = %self.wallet_ctx.active_address().unwrap()
    ))]
    async fn find_toma_token_wallet(&mut self, toma_package: ObjectID) -> Result<ObjectID> {
        let client = self.wallet_ctx.get_client().await?;
        let active_address = self.wallet_ctx.active_address()?;
        let Page { data: coins, .. } = client
            .coin_read_api()
            .get_coins(
                active_address,
                Some(format!("{toma_package}::toma::TOMA")),
                None,
                None,
            )
            .await?;
        coins
            .into_iter()
            .max_by_key(|coin| coin.balance)
            .map(|coin| coin.coin_object_id)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "No TOMA coins for {active_address}. \
                    Have you just received them? \
                    It may take a few seconds for cache to refresh. \
                    Double check that your address owns TOMA coins and try again."
                )
            })
    }
}
