//! Miscellaneous helpers for the EN.

use std::time::Duration;

use futures::FutureExt;
use tokio::sync::watch;
use zksync_eth_client::EthInterface;
use zksync_types::{L2ChainId, SLChainId};
use zksync_web3_decl::{
    client::{ClientMap, DynClient, L1, L2},
    error::ClientRpcContext,
    namespaces::{EthNamespaceClient, ZksNamespaceClient},
};

/// Task that validates chain IDs using main node and Ethereum clients.
#[derive(Debug)]
pub struct ValidateChainIdsTask {
    sl_chain_id: SLChainId,
    l2_chain_id: L2ChainId,
    eth_client: Box<DynClient<L1>>,
    main_node_client: Box<DynClient<L2>>,
    client_map: ClientMap,
}

// TODO: add validation for the introduced chain_id -> rpc + diamond proxy map
impl ValidateChainIdsTask {
    const BACKOFF_INTERVAL: Duration = Duration::from_secs(5);

    pub fn new(
        sl_chain_id: SLChainId,
        l2_chain_id: L2ChainId,
        eth_client: Box<DynClient<L1>>,
        main_node_client: Box<DynClient<L2>>,
        client_map: ClientMap,
    ) -> Self {
        Self {
            sl_chain_id,
            l2_chain_id,
            eth_client: eth_client.for_component("chain_ids_validation"),
            main_node_client: main_node_client.for_component("chain_ids_validation"),
            client_map,
        }
    }

    async fn check_eth_client(
        eth_client: Box<DynClient<L1>>,
        expected: SLChainId,
    ) -> anyhow::Result<()> {
        loop {
            match eth_client.fetch_chain_id().await {
                Ok(chain_id) => {
                    anyhow::ensure!(
                        expected == chain_id,
                        "Configured L1 chain ID doesn't match the one from Ethereum node. \
                        Make sure your configuration is correct and you are corrected to the right Ethereum node. \
                        Eth node chain ID: {chain_id}. Local config value: {expected}"
                    );
                    tracing::info!(
                        "Checked that L1 chain ID {chain_id} is returned by Ethereum client"
                    );
                    return Ok(());
                }
                Err(err) => {
                    tracing::warn!("Error getting L1 chain ID from Ethereum client: {err}");
                    tokio::time::sleep(Self::BACKOFF_INTERVAL).await;
                }
            }
        }
    }

    async fn check_l1_chain_using_main_node(
        main_node_client: Box<DynClient<L2>>,
        expected: SLChainId,
    ) -> anyhow::Result<()> {
        loop {
            match main_node_client
                .l1_chain_id()
                .rpc_context("l1_chain_id")
                .await
            {
                Ok(chain_id) => {
                    let chain_id = SLChainId(chain_id.as_u64());
                    anyhow::ensure!(
                        expected == chain_id,
                        "Configured L1 chain ID doesn't match the one from main node. \
                        Make sure your configuration is correct and you are corrected to the right main node. \
                        Main node L1 chain ID: {chain_id}. Local config value: {expected}"
                    );
                    tracing::info!(
                        "Checked that L1 chain ID {chain_id} is returned by main node client"
                    );
                    return Ok(());
                }
                Err(err) if err.is_retriable() => {
                    tracing::warn!(
                        "Retriable error getting L1 chain ID from main node client, will retry in {:?}: {err}",
                        Self::BACKOFF_INTERVAL
                    );
                    tokio::time::sleep(Self::BACKOFF_INTERVAL).await;
                }
                Err(err) => {
                    tracing::error!("Error getting L1 chain ID from main node client: {err}");
                    return Err(err.into());
                }
            }
        }
    }

    async fn check_l2_chain_using_main_node(
        main_node_client: Box<DynClient<L2>>,
        expected: L2ChainId,
    ) -> anyhow::Result<()> {
        loop {
            match main_node_client.chain_id().rpc_context("chain_id").await {
                Ok(chain_id) => {
                    let chain_id = L2ChainId::try_from(chain_id.as_u64()).map_err(|err| {
                        anyhow::anyhow!("invalid chain ID supplied by main node: {err}")
                    })?;
                    anyhow::ensure!(
                        expected == chain_id,
                        "Configured L2 chain ID doesn't match the one from main node. \
                        Make sure your configuration is correct and you are corrected to the right main node. \
                        Main node L2 chain ID: {chain_id:?}. Local config value: {expected:?}"
                    );
                    tracing::info!(
                        "Checked that L2 chain ID {chain_id:?} is returned by main node client"
                    );
                    return Ok(());
                }
                Err(err) if err.is_retriable() => {
                    tracing::warn!(
                        "Transient error getting L2 chain ID from main node client, will retry in {:?}: {err}",
                        Self::BACKOFF_INTERVAL
                    );
                    tokio::time::sleep(Self::BACKOFF_INTERVAL).await;
                }
                Err(err) => {
                    tracing::error!("Error getting L2 chain ID from main node client: {err}");
                    return Err(err.into());
                }
            }
        }
    }

    /// Runs the task once, exiting either when all the checks are performed or when the stop signal is received.
    pub async fn run_once(self, mut stop_receiver: watch::Receiver<bool>) -> anyhow::Result<()> {
        let eth_client_check = Self::check_eth_client(self.eth_client, self.sl_chain_id);
        let main_node_l1_check =
            Self::check_l1_chain_using_main_node(self.main_node_client.clone(), self.sl_chain_id);
        let main_node_l2_check =
            Self::check_l2_chain_using_main_node(self.main_node_client, self.l2_chain_id);
        let client_map_checks = self.client_map.0.keys().map(|chain_id| {
            let (client, _) = self.client_map.get_boxed(*chain_id).unwrap();
            let client = client.for_component("chain_ids_validation");
            Self::check_eth_client(client, *chain_id)
        });
        let joined_client_map_checks = futures::future::try_join_all(client_map_checks).fuse();
        let joined_futures = futures::future::try_join4(
            eth_client_check,
            main_node_l1_check,
            main_node_l2_check,
            joined_client_map_checks,
        )
        .fuse();
        tokio::select! {
            res = joined_futures => res.map(drop),
            _ = stop_receiver.changed() =>  Ok(()),
        }
    }

    // TODO
    /// Runs the task until the stop signal is received.
    pub async fn run(self, mut stop_receiver: watch::Receiver<bool>) -> anyhow::Result<()> {
        // Since check futures are fused, they are safe to poll after getting resolved; they will never resolve again,
        // so we'll just wait for another check or a stop signal.
        let eth_client_check = Self::check_eth_client(self.eth_client, self.sl_chain_id).fuse();
        let main_node_l1_check =
            Self::check_l1_chain_using_main_node(self.main_node_client.clone(), self.sl_chain_id)
                .fuse();
        let main_node_l2_check =
            Self::check_l2_chain_using_main_node(self.main_node_client, self.l2_chain_id).fuse();
        tokio::select! {
            Err(err) = eth_client_check =>  Err(err),
            Err(err) = main_node_l1_check =>  Err(err),
            Err(err) = main_node_l2_check =>  Err(err),
            _ = stop_receiver.changed() =>  Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use zksync_types::U64;
    use zksync_web3_decl::client::{MockClient, L1};

    use super::*;

    #[tokio::test]
    async fn validating_chain_ids_errors() {
        let eth_client = MockClient::builder(L1::default())
            .method("eth_chainId", || Ok(U64::from(9)))
            .build();
        let main_node_client = MockClient::builder(L2::default())
            .method("eth_chainId", || Ok(U64::from(270)))
            .method("zks_L1ChainId", || Ok(U64::from(3)))
            .build();

        let validation_task = ValidateChainIdsTask::new(
            SLChainId(3), // << mismatch with the Ethereum client
            L2ChainId::default(),
            Box::new(eth_client.clone()),
            Box::new(main_node_client.clone()),
            Default::default(),
        );
        let (_stop_sender, stop_receiver) = watch::channel(false);
        let err = validation_task
            .run(stop_receiver.clone())
            .await
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("L1 chain ID") && err.contains("Ethereum node"),
            "{err}"
        );

        let validation_task = ValidateChainIdsTask::new(
            SLChainId(9), // << mismatch with the main node client
            L2ChainId::from(270),
            Box::new(eth_client.clone()),
            Box::new(main_node_client),
            Default::default(),
        );
        let err = validation_task
            .run(stop_receiver.clone())
            .await
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("L1 chain ID") && err.contains("main node"),
            "{err}"
        );

        let main_node_client = MockClient::builder(L2::default())
            .method("eth_chainId", || Ok(U64::from(270)))
            .method("zks_L1ChainId", || Ok(U64::from(9)))
            .build();

        let validation_task = ValidateChainIdsTask::new(
            SLChainId(9),
            L2ChainId::from(271), // << mismatch with the main node client
            Box::new(eth_client),
            Box::new(main_node_client),
            Default::default(),
        );
        let err = validation_task
            .run(stop_receiver)
            .await
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("L2 chain ID") && err.contains("main node"),
            "{err}"
        );
    }

    #[tokio::test]
    async fn validating_chain_ids_success() {
        let eth_client = MockClient::builder(L1::default())
            .method("eth_chainId", || Ok(U64::from(9)))
            .build();
        let main_node_client = MockClient::builder(L2::default())
            .method("eth_chainId", || Ok(U64::from(270)))
            .method("zks_L1ChainId", || Ok(U64::from(9)))
            .build();

        let validation_task = ValidateChainIdsTask::new(
            SLChainId(9),
            L2ChainId::default(),
            Box::new(eth_client),
            Box::new(main_node_client),
            Default::default(),
        );
        let (stop_sender, stop_receiver) = watch::channel(false);
        let task = tokio::spawn(validation_task.run(stop_receiver));

        // Wait a little and ensure that the task hasn't terminated.
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!task.is_finished());

        stop_sender.send_replace(true);
        task.await.unwrap().unwrap();
    }
}
