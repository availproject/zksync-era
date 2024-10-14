use anyhow::anyhow;
use async_trait::async_trait;
use blake2::digest::crypto_common::rand_core::block;
use bytes::Bytes;
use jsonrpsee::ws_client::WsClientBuilder;
use serde::{Deserialize, Serialize};
use std::{fmt::Debug, sync::Arc};
use subxt_signer::ExposeSecret;
use zksync_config::configs::da_client::avail::{
    AvailClientConfig, AvailConfig, AvailDefaultConfig, AvailGasRelayConfig, AvailSecrets,
};
use zksync_da_client::{
    types::{DAError, DispatchResponse, InclusionData},
    DataAvailabilityClient,
};
use zksync_types::{
    api_key::APIKey,
    ethabi::{self, Token},
    web3::contract::Tokenize,
    H256, U256,
};

use crate::avail::sdk::{GasRelayClient, RawAvailClient};

#[derive(Debug, Clone)]
enum AvailClientMode {
    Default(RawAvailClient),
    GasRelay(GasRelayClient),
}

/// An implementation of the `DataAvailabilityClient` trait that interacts with the Avail network.
#[derive(Debug, Clone)]
pub struct AvailClient {
    config: AvailConfig,
    sdk_client: Arc<AvailClientMode>,
    api_client: Arc<reqwest::Client>, // bridge API reqwest client
}

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct BridgeAPIResponse {
    blob_root: Option<H256>,
    bridge_root: Option<H256>,
    data_root_index: Option<U256>,
    data_root_proof: Option<Vec<H256>>,
    leaf: Option<H256>,
    leaf_index: Option<U256>,
    leaf_proof: Option<Vec<H256>>,
    range_hash: Option<H256>,
    error: Option<String>,
}

#[derive(Deserialize, Serialize, Debug)]
#[serde(rename_all = "camelCase")]
struct MerkleProofInput {
    // proof of inclusion for the data root
    data_root_proof: Vec<H256>,
    // proof of inclusion of leaf within blob/bridge root
    leaf_proof: Vec<H256>,
    // abi.encodePacked(startBlock, endBlock) of header range commitment on vectorx
    range_hash: H256,
    // index of the data root in the commitment tree
    data_root_index: U256,
    // blob root to check proof against, or reconstruct the data root
    blob_root: H256,
    // bridge root to check proof against, or reconstruct the data root
    bridge_root: H256,
    // leaf being proven
    leaf: H256,
    // index of the leaf in the blob/bridge root tree
    leaf_index: U256,
}

impl Tokenize for MerkleProofInput {
    fn into_tokens(self) -> Vec<Token> {
        vec![Token::Tuple(vec![
            Token::Array(
                self.data_root_proof
                    .iter()
                    .map(|x| Token::FixedBytes(x.as_bytes().to_vec()))
                    .collect(),
            ),
            Token::Array(
                self.leaf_proof
                    .iter()
                    .map(|x| Token::FixedBytes(x.as_bytes().to_vec()))
                    .collect(),
            ),
            Token::FixedBytes(self.range_hash.as_bytes().to_vec()),
            Token::Uint(self.data_root_index),
            Token::FixedBytes(self.blob_root.as_bytes().to_vec()),
            Token::FixedBytes(self.bridge_root.as_bytes().to_vec()),
            Token::FixedBytes(self.leaf.as_bytes().to_vec()),
            Token::Uint(self.leaf_index),
        ])]
    }
}

impl AvailClient {
    pub async fn new(config: AvailConfig, secrets: AvailSecrets) -> anyhow::Result<Self> {
        let api_client = Arc::new(reqwest::Client::new());
        if config.gas_relay_mode {
            let gas_relay_api_key = secrets
                .gas_relay_api_key
                .ok_or_else(|| anyhow::anyhow!("Gas relay API key is missing"))?;
            let gas_relay_config: AvailGasRelayConfig = match config.config.clone() {
                AvailClientConfig::GasRelay(conf) => conf,
                _ => unreachable!(), // validated in protobuf config
            };
            let gas_relay_client = GasRelayClient::new(
                &gas_relay_config.gas_relay_api_url,
                gas_relay_api_key.0.expose_secret(),
                Arc::clone(&api_client),
            )
            .await?;
            return Ok(Self {
                config,
                sdk_client: Arc::new(AvailClientMode::GasRelay(gas_relay_client)),
                api_client,
            });
        }

        let default_config = match &config.config {
            AvailClientConfig::Default(conf) => conf.clone(),
            _ => unreachable!(), // validated in protobug config
        };
        let seed_phrase = secrets
            .seed_phrase
            .ok_or_else(|| anyhow::anyhow!("Seed phrase is missing"))?;
        // these unwraps are safe because we validate in protobuf config
        let sdk_client =
            RawAvailClient::new(default_config.app_id, seed_phrase.0.expose_secret()).await?;

        Ok(Self {
            config,
            sdk_client: Arc::new(AvailClientMode::Default(sdk_client)),
            api_client,
        })
    }
}

#[async_trait]
impl DataAvailabilityClient for AvailClient {
    async fn dispatch_blob(
        &self,
        _: u32, // batch_number
        data: Vec<u8>,
    ) -> anyhow::Result<DispatchResponse, DAError> {
        match self.sdk_client.as_ref() {
            AvailClientMode::Default(client) => {
                let default_config = match &self.config.config {
                    AvailClientConfig::Default(conf) => conf,
                    _ => unreachable!(), // validated in protobuf config
                };
                let ws_client = WsClientBuilder::default()
                    .build(default_config.api_node_url.clone().as_str())
                    .await
                    .map_err(to_non_retriable_da_error)?;

                let extrinsic = client
                    .build_extrinsic(&ws_client, data)
                    .await
                    .map_err(to_non_retriable_da_error)?;

                let block_hash = client
                    .submit_extrinsic(&ws_client, extrinsic.as_str())
                    .await
                    .map_err(to_non_retriable_da_error)?;
                let tx_id = client
                    .get_tx_id(&ws_client, block_hash.as_str(), extrinsic.as_str())
                    .await
                    .map_err(to_non_retriable_da_error)?;
                Ok(DispatchResponse::from(format!("{}:{}", block_hash, tx_id)))
            }
            AvailClientMode::GasRelay(client) => {
                let (block_hash, extrinsic_index) = client
                    .post_data(data)
                    .await
                    .map_err(to_retriable_da_error)?;
                Ok(DispatchResponse {
                    blob_id: format!("{:x}:{}", block_hash, extrinsic_index),
                })
            }
        }
    }

    async fn get_inclusion_data(
        &self,
        blob_id: &str,
    ) -> anyhow::Result<Option<InclusionData>, DAError> {
        let (block_hash, tx_idx) = blob_id.split_once(':').ok_or_else(|| DAError {
            error: anyhow!("Invalid blob ID format"),
            is_retriable: false,
        })?;

        let url = format!(
            "{}/eth/proof/{}?index={}",
            self.config.bridge_api_url, block_hash, tx_idx
        );

        let response = self
            .api_client
            .get(&url)
            .send()
            .await
            .map_err(to_retriable_da_error)?;
        let bridge_api_data = response
            .json::<BridgeAPIResponse>()
            .await
            .map_err(to_retriable_da_error)?;
        if bridge_api_data.error.is_some() {
            return Err(to_retriable_da_error(anyhow!(format!(
                "Bridge API returned an error: {}",
                bridge_api_data.error.unwrap()
            ))));
        }

        let attestation_data: MerkleProofInput = MerkleProofInput {
            data_root_proof: bridge_api_data.data_root_proof.unwrap(),
            leaf_proof: bridge_api_data.leaf_proof.unwrap(),
            range_hash: bridge_api_data.range_hash.unwrap(),
            data_root_index: bridge_api_data.data_root_index.unwrap(),
            blob_root: bridge_api_data.blob_root.unwrap(),
            bridge_root: bridge_api_data.bridge_root.unwrap(),
            leaf: bridge_api_data.leaf.unwrap(),
            leaf_index: bridge_api_data.leaf_index.unwrap(),
        };
        Ok(Some(InclusionData {
            data: ethabi::encode(&attestation_data.into_tokens()),
        }))
    }

    fn clone_boxed(&self) -> Box<dyn DataAvailabilityClient> {
        Box::new(self.clone())
    }

    fn blob_size_limit(&self) -> Option<usize> {
        Some(RawAvailClient::MAX_BLOB_SIZE)
    }
}

pub fn to_non_retriable_da_error(error: impl Into<anyhow::Error>) -> DAError {
    DAError {
        error: error.into(),
        is_retriable: false,
    }
}

pub fn to_retriable_da_error(error: impl Into<anyhow::Error>) -> DAError {
    DAError {
        error: error.into(),
        is_retriable: true,
    }
}
