use std::pin::Pin;
use std::time::Duration;

use clap::{command, Parser, Subcommand};
use futures::{stream::FuturesUnordered, Future};
use ingress::IngressControllerContext;
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
        /// Default time to wait between requeuing resources.
        #[arg(env, long, default_value_t = 30)]
        requeue_time_secs: u64,

        /// If enabled, controller will create Records for all
        /// ingresses based on its hosts and loadBalancer settings.
        #[arg(env, long, default_value_t = false)]
        ingress_record_creation: bool,
    },
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    tracing_subscriber::fmt::init();
    let args = Args::parse();

    match args.command {
        Command::Reconcile {
            requeue_time_secs,
            ingress_record_creation,
        } => {
            let client = Client::try_default().await.unwrap();

            let futures: FuturesUnordered<Pin<Box<dyn Future<Output = ()>>>> =
                FuturesUnordered::new();

            futures.push(Box::pin(async {
                zone::controller(ZoneControllerContext {
                    client: client.clone(),
                    requeue_time: Duration::from_secs(requeue_time_secs),
                })
                .await;
            }));

            futures.push(Box::pin(async {
                record::controller(RecordControllerContext {
                    client: client.clone(),
                    requeue_time: Duration::from_secs(requeue_time_secs),
                })
                .await;
            }));

            if ingress_record_creation {
                futures.push(Box::pin(async {
                    ingress::controller(IngressControllerContext {
                        client: client.clone(),
                        requeue_time: Duration::from_secs(requeue_time_secs),
                    })
                    .await;
                }));
            }

            futures::future::select_all(futures.into_iter()).await;
        }
    }
}
