//! Tests for the `debug` Web3 namespace.

use zksync_multivm::interface::{Call, TransactionExecutionResult};
use zksync_types::{
    api::{CallTracerConfig, CallTracerOption, CallTracerResult, SupportedTracers, TracerConfig},
    BOOTLOADER_ADDRESS,
};
use zksync_web3_decl::{
    client::{DynClient, L2},
    namespaces::DebugNamespaceClient,
};

use super::*;

fn execute_l2_transaction_with_traces(index_in_block: u8) -> TransactionExecutionResult {
    let first_call_trace = Call {
        from: Address::repeat_byte(index_in_block),
        to: Address::repeat_byte(index_in_block + 1),
        gas: 100,
        gas_used: 42,
        ..Call::default()
    };
    let second_call_trace = Call {
        from: Address::repeat_byte(0xff - index_in_block),
        to: Address::repeat_byte(0xab - index_in_block),
        value: 123.into(),
        gas: 58,
        gas_used: 10,
        input: b"input".to_vec(),
        output: b"output".to_vec(),
        ..Call::default()
    };
    TransactionExecutionResult {
        call_traces: vec![first_call_trace, second_call_trace],
        ..execute_l2_transaction(create_l2_transaction(1, 2))
    }
}

#[derive(Debug)]
struct TraceBlockTest(L2BlockNumber);

#[async_trait]
impl HttpTest for TraceBlockTest {
    async fn test(
        &self,
        client: &DynClient<L2>,
        pool: &ConnectionPool<Core>,
    ) -> anyhow::Result<()> {
        let tx_results = [0, 1, 2].map(execute_l2_transaction_with_traces);
        let mut storage = pool.connection().await?;
        let new_l2_block = store_l2_block(&mut storage, self.0, &tx_results).await?;
        drop(storage);

        let block_ids = [
            api::BlockId::Number((*self.0).into()),
            api::BlockId::Number(api::BlockNumber::Latest),
            api::BlockId::Hash(new_l2_block.hash),
        ];

        for block_id in block_ids {
            let block_traces = match block_id {
                api::BlockId::Number(number) => client.trace_block_by_number(number, None).await?,
                api::BlockId::Hash(hash) => client.trace_block_by_hash(hash, None).await?,
            };

            assert_eq!(block_traces.len(), tx_results.len()); // equals to the number of transactions in the block
            for (trace, tx_result) in block_traces.iter().zip(&tx_results) {
                let CallTracerOption::CallTrace(result) = trace else {
                    unreachable!()
                };
                assert_eq!(result.from, Address::zero());
                assert_eq!(result.to, BOOTLOADER_ADDRESS);
                assert_eq!(result.gas, tx_result.transaction.gas_limit());
                let expected_calls: Vec<_> = tx_result
                    .call_traces
                    .iter()
                    .map(|call| DebugNamespace::map_default_call(call.clone(), false))
                    .collect();
                assert_eq!(result.calls, expected_calls);
            }
        }

        let missing_block_number = api::BlockNumber::from(*self.0 + 100);
        let error = client
            .trace_block_by_number(missing_block_number, None)
            .await
            .unwrap_err();
        if let ClientError::Call(error) = error {
            assert_eq!(error.code(), ErrorCode::InvalidParams.code());
            assert!(
                error.message().contains("Block") && error.message().contains("doesn't exist"),
                "{error:?}"
            );
            assert!(error.data().is_none(), "{error:?}");
        } else {
            panic!("Unexpected error: {error:?}");
        }

        Ok(())
    }
}

#[tokio::test]
async fn tracing_block() {
    test_http_server(TraceBlockTest(L2BlockNumber(1))).await;
}

#[derive(Debug)]
struct TraceBlockFlatTest(L2BlockNumber);

#[async_trait]
impl HttpTest for TraceBlockFlatTest {
    async fn test(
        &self,
        client: &DynClient<L2>,
        pool: &ConnectionPool<Core>,
    ) -> anyhow::Result<()> {
        let tx_results = [0, 1, 2].map(execute_l2_transaction_with_traces);
        let mut storage = pool.connection().await?;
        store_l2_block(&mut storage, self.0, &tx_results).await?;
        drop(storage);

        let block_ids = [
            api::BlockId::Number((*self.0).into()),
            api::BlockId::Number(api::BlockNumber::Latest),
        ];

        for block_id in block_ids {
            if let api::BlockId::Number(number) = block_id {
                let block_traces = client
                    .trace_block_by_number(
                        number,
                        Some(TracerConfig {
                            tracer: SupportedTracers::FlatCallTracer,
                            tracer_config: CallTracerConfig {
                                only_top_call: false,
                            },
                        }),
                    )
                    .await?;

                // A transaction with 2 nested calls will convert into 3 Flattened calls.
                // Also in this test, all tx have the same # of nested calls
                assert_eq!(
                    block_traces.len(),
                    tx_results.len() * (tx_results[0].call_traces.len() + 1)
                );

                // First tx has 2 nested calls, thus 2 sub-traces
                // assert_eq!(block_traces[0].subtraces, 2);
                // assert_eq!(block_traces[0].traceaddress, [0]);
                // // Second flat-call (fist nested call) do not have nested calls
                // assert_eq!(block_traces[1].subtraces, 0);
                // assert_eq!(block_traces[1].traceaddress, [0, 0]);

                let top_level_call_indexes = [0, 3, 6];
                let top_level_traces = top_level_call_indexes
                    .iter()
                    .map(|&i| block_traces[i].clone());

                for (top_level_trace, tx_result) in top_level_traces.zip(&tx_results) {
                    let CallTracerOption::FlattCallTrace(top_level_trace) = top_level_trace else {
                        unreachable!()
                    };
                    assert_eq!(top_level_trace.action.from, Address::zero());
                    assert_eq!(top_level_trace.action.to, BOOTLOADER_ADDRESS);
                    assert_eq!(
                        top_level_trace.action.gas,
                        tx_result.transaction.gas_limit()
                    );
                }
                // TODO: test inner calls
            }
        }

        let missing_block_number = api::BlockNumber::from(*self.0 + 100);
        let error = client
            .trace_block_by_number(
                missing_block_number,
                Some(TracerConfig {
                    tracer: SupportedTracers::FlatCallTracer,
                    tracer_config: CallTracerConfig {
                        only_top_call: false,
                    },
                }),
            )
            .await
            .unwrap_err();
        if let ClientError::Call(error) = error {
            assert_eq!(error.code(), ErrorCode::InvalidParams.code());
            assert!(
                error.message().contains("Block") && error.message().contains("doesn't exist"),
                "{error:?}"
            );
            assert!(error.data().is_none(), "{error:?}");
        } else {
            panic!("Unexpected error: {error:?}");
        }

        Ok(())
    }
}

#[tokio::test]
async fn tracing_block_flat() {
    test_http_server(TraceBlockFlatTest(L2BlockNumber(1))).await;
}

#[derive(Debug)]
struct TraceTransactionTest;

#[async_trait]
impl HttpTest for TraceTransactionTest {
    async fn test(
        &self,
        client: &DynClient<L2>,
        pool: &ConnectionPool<Core>,
    ) -> anyhow::Result<()> {
        let tx_results = [execute_l2_transaction_with_traces(0)];
        let mut storage = pool.connection().await?;
        store_l2_block(&mut storage, L2BlockNumber(1), &tx_results).await?;
        drop(storage);

        let expected_calls: Vec<_> = tx_results[0]
            .call_traces
            .iter()
            .map(|call| DebugNamespace::map_default_call(call.clone(), false))
            .collect();

        let CallTracerResult::CallTrace(result) = client
            .trace_transaction(tx_results[0].hash, None)
            .await?
            .context("no transaction traces")?
        else {
            unreachable!()
        };
        assert_eq!(result.from, Address::zero());
        assert_eq!(result.to, BOOTLOADER_ADDRESS);
        assert_eq!(result.gas, tx_results[0].transaction.gas_limit());
        assert_eq!(result.calls, expected_calls);

        Ok(())
    }
}

#[tokio::test]
async fn tracing_transaction() {
    test_http_server(TraceTransactionTest).await;
}

#[derive(Debug)]
struct TraceBlockTestWithSnapshotRecovery;

#[async_trait]
impl HttpTest for TraceBlockTestWithSnapshotRecovery {
    fn storage_initialization(&self) -> StorageInitialization {
        StorageInitialization::empty_recovery()
    }

    async fn test(
        &self,
        client: &DynClient<L2>,
        pool: &ConnectionPool<Core>,
    ) -> anyhow::Result<()> {
        let snapshot_l2_block_number = StorageInitialization::SNAPSHOT_RECOVERY_BLOCK;
        let missing_l2_block_numbers = [
            L2BlockNumber(0),
            snapshot_l2_block_number - 1,
            snapshot_l2_block_number,
        ];

        for number in missing_l2_block_numbers {
            let error = client
                .trace_block_by_number(number.0.into(), None)
                .await
                .unwrap_err();
            assert_pruned_block_error(&error, snapshot_l2_block_number + 1);
        }

        TraceBlockTest(snapshot_l2_block_number + 2)
            .test(client, pool)
            .await?;
        Ok(())
    }
}

#[tokio::test]
async fn tracing_block_after_snapshot_recovery() {
    test_http_server(TraceBlockTestWithSnapshotRecovery).await;
}
