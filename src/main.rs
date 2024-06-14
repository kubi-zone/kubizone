use std::time::Duration;

use clap::{command, Parser, Subcommand};
use kube::Client;
use record::RecordControllerContext;
use zone::ZoneControllerContext;

pub use kubizone::*;

#[derive(Debug, Parser)]
#[command(author, version, about, long_about = None)]
struct Args {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Reconcile {
        #[arg(env, long, default_value_t = 30)]
        requeue_time_secs: u64,
    },
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    tracing_subscriber::fmt::init();
    let args = Args::parse();

    match args.command {
        Command::Reconcile { requeue_time_secs } => {
            let client = Client::try_default().await.unwrap();

            tokio::select! {
                _ = zone::controller(ZoneControllerContext {
                    client: client.clone(),
                    requeue_time: Duration::from_secs(requeue_time_secs)}) => (),
                _ = record::controller(RecordControllerContext {
                    client: client.clone(),
                    requeue_time: Duration::from_secs(requeue_time_secs)}) => ()
            }
        }
    }
}
