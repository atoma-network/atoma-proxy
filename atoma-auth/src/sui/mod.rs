use std::{path::Path, str::FromStr};

use anyhow::Result;
use atoma_sui::{events::StackCreatedEvent, AtomaSuiConfig};
use blake2::{
    digest::generic_array::{typenum::U32, GenericArray},
    Blake2b, Digest,
};
use serde_json::Value;
use sui_keys::keystore::{AccountKeystore, Keystore};
use sui_sdk::{
    rpc_types::{
        BalanceChange, Coin, Page, SuiObjectDataOptions, SuiTransactionBlockResponseOptions,
    },
    types::{
        base_types::{ObjectID, SequenceNumber, SuiAddress},
        crypto::EncodeDecodeBase64,
        digests::TransactionDigest,
        object::Owner,
        programmable_transaction_builder::ProgrammableTransactionBuilder,
        transaction::{Argument, CallArg, Command, ObjectArg, TransactionData},
        Identifier, SUI_RANDOMNESS_STATE_OBJECT_ID,
    },
    wallet_context::WalletContext,
};
use tracing::{info, instrument};

const GAS_BUDGET: u64 = 5_000_000; // 0.005 SUI

const ONE_MILLION: u64 = 1_000_000;

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
    pub wallet_ctx: WalletContext,
    /// Atoma package object ID
    atoma_package_id: ObjectID,
    /// Atoma DB object ID
    atoma_db_id: ObjectID,
    /// USDC package object ID
    pub usdc_package_id: ObjectID,
    /// Initial version of the Atoma DB
    atoma_db_initial_version: Option<SequenceNumber>,
    /// Initial version of the random state object
    randomness_state_object_initial_version: Option<SequenceNumber>,
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
            atoma_package_id: sui_config.atoma_package_id(),
            atoma_db_id: sui_config.atoma_db(),
            usdc_package_id: sui_config.usdc_package_id(),
            atoma_db_initial_version: None,
            randomness_state_object_initial_version: None,
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

    /// Get coins of coin_type with sum greater or equal to value
    ///
    /// # Arguments
    ///
    /// * `address` - The address for which to get the coins
    /// * `coin_type` - The coin type to filter by, None represents SUI coins
    /// * `value` - The value to sum the coins to
    ///
    /// # Returns
    ///
    /// Returns the coins with sum greater or equal to value
    ///
    /// # Errors
    ///
    /// Returns an error if the coins are not found or if the sum of the coins is less than the value
    async fn get_coins_of_value(
        &mut self,
        coin_type: Option<String>,
        value: u64,
    ) -> Result<Vec<Coin>> {
        let client = self.wallet_ctx.get_client().await?;
        let address = self.wallet_ctx.active_address()?;
        let mut cursor = None;
        let mut total_balance = 0;
        let mut selected_coins = Vec::new();
        loop {
            let Page {
                data: coins,
                next_cursor,
                has_next_page,
            } = client
                .coin_read_api()
                .get_coins(address, coin_type.clone(), cursor, None)
                .await?;
            for coin in coins.iter() {
                total_balance += coin.balance;
                selected_coins.push(coin.clone());
                if total_balance >= value {
                    return Ok(selected_coins);
                }
            }
            if !has_next_page {
                break;
            }
            cursor = next_cursor;
        }
        Err(anyhow::anyhow!("Not enough coins to cover the value"))
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
        let atoma_db_initial_version = self.get_or_load_atoma_db_initial_version().await?;
        let randomness_state_object_initial_version = self
            .get_or_load_randomness_state_object_initial_version()
            .await?;
        let total_usdc_value_required =
            num_compute_units * price_per_one_million_compute_units / ONE_MILLION;
        let usdc_coins = self
            .get_coins_of_value(
                Some(format!("{}::usdc::USDC", self.usdc_package_id)),
                total_usdc_value_required,
            )
            .await?;

        let mut ptb = ProgrammableTransactionBuilder::new();
        let mut usdc_coins: Vec<Argument> = usdc_coins
            .into_iter()
            .map(|coin| ptb.input(CallArg::from(coin.object_ref())))
            .collect::<Result<_>>()?;

        // Merge coins first, if needed. The coins are merged into the first coin in the list.
        let primary_coin = usdc_coins.remove(0);
        if !usdc_coins.is_empty() {
            // If we have more than one coin, merge them into the first coin.
            ptb.command(Command::MergeCoins(primary_coin, usdc_coins));
        }

        let atoma_db_arg = ptb.obj(ObjectArg::SharedObject {
            id: self.atoma_db_id,
            initial_shared_version: atoma_db_initial_version,
            mutable: true,
        })?;
        let task_small_id_arg = ptb.pure(task_small_id)?;
        let num_compute_units_arg = ptb.pure(num_compute_units)?;
        let price_per_one_million_compute_units_arg =
            ptb.pure(price_per_one_million_compute_units)?;
        let randomness_object = ptb.obj(ObjectArg::SharedObject {
            id: SUI_RANDOMNESS_STATE_OBJECT_ID,
            initial_shared_version: randomness_state_object_initial_version,
            mutable: false,
        })?;

        ptb.programmable_move_call(
            self.atoma_package_id,
            Identifier::from_str("db")?,
            Identifier::from_str("acquire_new_stack_entry")?,
            vec![],
            vec![
                atoma_db_arg,
                primary_coin,
                task_small_id_arg,
                num_compute_units_arg,
                price_per_one_million_compute_units_arg,
                randomness_object,
            ],
        );

        let pt = ptb.finish();

        let gas_price = client.read_api().get_reference_gas_price().await?;

        let gas_coins = self.get_coins_of_value(None, GAS_BUDGET).await?;

        let tx = TransactionData::new_programmable(
            address,
            gas_coins
                .into_iter()
                .map(|coin| coin.object_ref())
                .collect(),
            pt,
            GAS_BUDGET,
            gas_price,
        );

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

    /// Get the initial version of a sui object.
    ///
    /// # Arguments
    ///
    /// * `object_id` - The object ID for which to get the initial version.
    ///
    /// # Returns
    ///
    /// Returns the initial version of the object.
    ///
    /// # Errors
    ///
    /// Returns an error if the object is not shared.
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// let mut client = AtomaProxy::new(config).await?;
    /// let initial_version = client.get_initial_version(object_id).await?;
    /// ```
    #[instrument(level = "info", skip(self))]
    async fn get_initial_version(&self, object_id: ObjectID) -> Result<SequenceNumber> {
        let client = self.wallet_ctx.get_client().await?;
        let object = client
            .read_api()
            .get_object_with_options(
                object_id,
                SuiObjectDataOptions {
                    show_owner: true,
                    ..Default::default()
                },
            )
            .await?;
        if let Some(Owner::Shared {
            initial_shared_version,
        }) = object.owner()
        {
            Ok(initial_shared_version)
        } else {
            anyhow::bail!("Object is not shared")
        }
    }

    /// Get or load the initial version of the Atoma DB
    ///
    /// This method checks if the initial version of the Atoma DB is already loaded and returns it if so.
    /// Otherwise, it loads the initial version of the Atoma DB by calling `get_initial_version` with the Atoma DB object ID.
    ///
    /// # Returns
    ///
    /// Returns the initial version of the Atoma DB.
    ///
    /// # Errors
    ///
    /// Returns an error if it fails to get the initial version of the Atoma DB.
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// let mut client = AtomaProxy::new(config).await?;
    /// let initial_version = client.get_or_load_atoma_db_initial_version().await?;
    /// ```
    #[instrument(level = "info", skip_all)]
    pub async fn get_or_load_atoma_db_initial_version(&mut self) -> Result<SequenceNumber> {
        if let Some(atoma_db_initial_version) = self.atoma_db_initial_version {
            return Ok(atoma_db_initial_version);
        }
        let initial_version = self.get_initial_version(self.atoma_db_id).await?;
        self.atoma_db_initial_version = Some(initial_version);
        Ok(initial_version)
    }

    /// Get or load the initial version of the randomness state object
    ///
    /// This method checks if the initial version of the randomness state object is already loaded and returns it if so.
    /// Otherwise, it loads the initial version of the randomness state object by calling `get_initial_version` with the randomness state object ID.
    ///
    /// # Returns
    ///
    /// Returns the initial version of the randomness state object.
    ///
    /// # Errors
    ///
    /// Returns an error if it fails to get the initial version of the randomness state object.
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// let mut client = AtomaProxy::new(config).await?;
    /// let initial_version = client.get_or_load_randomness_state_object_initial_version().await?;
    /// ```
    #[instrument(level = "info", skip_all)]
    pub async fn get_or_load_randomness_state_object_initial_version(
        &mut self,
    ) -> Result<SequenceNumber> {
        if let Some(randomness_state_object_initial_version) =
            self.randomness_state_object_initial_version
        {
            return Ok(randomness_state_object_initial_version);
        }
        let initial_version = self
            .get_initial_version(SUI_RANDOMNESS_STATE_OBJECT_ID)
            .await?;
        self.randomness_state_object_initial_version = Some(initial_version);
        Ok(initial_version)
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

    /// Get the balance changes for a given transaction digest
    ///
    /// # Arguments
    ///
    /// * `transaction_digest` - The transaction digest for which to get the balance changes
    ///
    /// # Returns
    ///
    /// Returns the balance changes
    ///
    /// # Errors
    ///
    /// Returns an error if the transaction digest is invalid or if the transaction is not found
    /// or if the balance changes are not found.
    #[instrument(level = "info", skip(self))]
    pub async fn get_balance_changes(
        &self,
        transaction_digest: &str,
    ) -> Result<Option<Vec<BalanceChange>>> {
        let transaction_digest = TransactionDigest::from_str(transaction_digest).unwrap();
        let client = self.wallet_ctx.get_client().await?;
        let transaction = client
            .read_api()
            .get_transaction_with_options(
                transaction_digest,
                SuiTransactionBlockResponseOptions {
                    show_balance_changes: true,
                    ..Default::default()
                },
            )
            .await?;
        Ok(transaction.balance_changes)
    }
}
