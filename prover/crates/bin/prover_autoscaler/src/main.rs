use std::time::Duration;

use anyhow::Context as _;
use structopt::StructOpt;
use tokio::{
    sync::{oneshot, watch},
    task::JoinHandle,
};
use zksync_prover_autoscaler::{
    agent,
    global::{self, queuer},
    k8s::{Scaler, Watcher},
    task_wiring::TaskRunner,
};
use zksync_utils::wait_for_tasks::ManagedTasks;
use zksync_vlog::prometheus::PrometheusExporterConfig;

/// Represents the sequential number of the Prover Autoscaler Jobs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum ProverJob {
    Scaler = 0,
    Agent = 1,
}

impl std::str::FromStr for ProverJob {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "scaler" => Ok(ProverJob::Scaler),
            "agent" => Ok(ProverJob::Agent),
            other => Err(format!("{} is not a valid Prover Scaler Job", other)),
        }
    }
}

#[derive(Debug, StructOpt)]
#[structopt(name = "Prover Autoscaler", about = "Run Prover Autoscaler components")]
struct Opt {
    /// Prover Autoscaler can run Agent or Scaler job.
    ///
    /// Specify `agent` or `scaler`
    #[structopt(short, long, default_value = "agent")]
    job: ProverJob,
    /// Name of the cluster Agent is watching.
    #[structopt(long)]
    cluster_name: Option<String>,
    /// Path to the configuration file.
    #[structopt(long)]
    config_path: Option<std::path::PathBuf>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    let opt = Opt::from_args();
    // let general_config = load_general_config(opt.config_path).context("general config")?;

    let exporter_config = PrometheusExporterConfig::pull(8080);

    let (stop_signal_sender, stop_signal_receiver) = oneshot::channel();
    let mut stop_signal_sender = Some(stop_signal_sender);
    ctrlc::set_handler(move || {
        if let Some(sender) = stop_signal_sender.take() {
            sender.send(()).ok();
        }
    })
    .context("Error setting Ctrl+C handler")?;

    let (stop_sender, stop_receiver) = watch::channel(false);

    let _ = rustls::crypto::ring::default_provider().install_default();
    let client = kube::Client::try_default().await?;

    tracing::info!("Starting ProverAutoscaler");

    // TODO get from config
    let graceful_shutdown_timeout = Duration::from_millis(20);

    let mut tasks = vec![tokio::spawn(exporter_config.run(stop_receiver.clone()))];

    match opt.job {
        ProverJob::Agent => {
            // TODO: maybe get cluster name from curl -H "Metadata-Flavor: Google"
            // http://metadata.google.internal/computeMetadata/v1/instance/attributes/cluster-name
            let watcher = Watcher::new(
                client.clone(),
                opt.cluster_name
                    .context("cluster_name is required for Agent")?,
                vec!["prover-blue".to_string(), "prover-red".to_string()],
            );
            let scaler = Scaler { client };
            tasks.push(tokio::spawn(watcher.clone().run()));
            tasks.push(tokio::spawn(agent::run_server(
                8081,
                watcher,
                scaler,
                stop_receiver.clone(),
            )))
        }
        ProverJob::Scaler => {
            let watcher = global::watcher::Watcher::new(vec!["http://localhost:8081".to_string()]);
            let queuer = global::queuer::Queuer {};
            let scaler = global::scaler::Scaler::new(watcher.clone(), queuer);
            tasks.extend(get_tasks(watcher, scaler, stop_receiver)?);
        }
    }

    let mut tasks = ManagedTasks::new(tasks);

    tokio::select! {
        _ = tasks.wait_single() => {},
        _ = stop_signal_receiver => {
            tracing::info!("Stop signal received, shutting down");
        }
    }
    stop_sender.send(true).ok();
    tasks.complete(graceful_shutdown_timeout).await;

    Ok(())
}

fn get_tasks(
    watcher: global::watcher::Watcher,
    scaler: global::scaler::Scaler,
    stop_receiver: watch::Receiver<bool>,
) -> anyhow::Result<Vec<JoinHandle<anyhow::Result<()>>>> {
    let mut task_runner = TaskRunner::new();

    task_runner.add("Watcher", Duration::from_secs(10), watcher);
    task_runner.add("Scaler", Duration::from_secs(10), scaler);

    Ok(task_runner.spawn(stop_receiver))
}
