use clap::{command, Parser, Subcommand};
use kube::Client;

mod record;
mod zone;

#[derive(Debug, Parser)]
#[command(author, version, about, long_about = None)]
struct Args {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Reconcile,
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    tracing_subscriber::fmt::init();
    let args = Args::parse();

    match args.command {
        Command::Reconcile => {
            let client = Client::try_default().await.unwrap();

            tokio::select! {
                _ = zone::controller(client.clone()) => (),
                _ = record::controller(client) => ()
            }
        }
    }
}
