//! Native peer for the browser demo.
//!
//! Built from exactly the same `echo` module the wasm build uses, which is the
//! interesting part: a node in a terminal and a node in a browser tab speak the
//! same protocol and can connect to each other.
//!
//!   cargo run --features cli -- accept
//!   cargo run --features cli -- connect <ENDPOINT_ID> "some payload"

use anyhow::Result;
use clap::{Parser, Subcommand};
use iroh::EndpointId;
use iroh_wasm_demo::echo::{AcceptEvent, ConnectEvent, EchoNode};
use n0_future::StreamExt;

#[derive(Parser)]
#[command(about = "native peer for the iroh wasm browser demo")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Bind an endpoint and echo whatever anyone sends, until ctrl-c.
    Accept,
    /// Dial a peer (e.g. a browser tab) and echo a payload off it.
    Connect {
        /// The peer's endpoint id.
        endpoint_id: String,
        /// Text to send.
        #[arg(default_value = "hi from the cli")]
        payload: String,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let node = EchoNode::spawn().await?;

    println!("our endpoint id: {}", node.endpoint().id());
    print!("waiting to come online … ");
    node.wait_online().await;
    println!("ok");

    let info = node.info();
    println!("relays: {:?}", info.relays);
    println!("alpn:   {}", info.alpn);

    match cli.command {
        Command::Accept => {
            println!(
                "\naccepting echo connections — dial this id from the browser. ctrl-c to quit.\n"
            );
            let mut events = node.accept_events();
            while let Some(event) = events.next().await {
                match event {
                    AcceptEvent::Accepted { endpoint_id } => {
                        println!("[{endpoint_id}] accepted");
                    }
                    AcceptEvent::Echoed {
                        endpoint_id,
                        bytes_sent,
                    } => println!("[{endpoint_id}] echoed {bytes_sent} byte(s)"),
                    AcceptEvent::Closed { endpoint_id, error } => match error {
                        Some(err) => println!("[{endpoint_id}] closed with error: {err}"),
                        None => println!("[{endpoint_id}] closed"),
                    },
                }
            }
        }
        Command::Connect {
            endpoint_id,
            payload,
        } => {
            let peer: EndpointId = endpoint_id
                .trim()
                .parse()
                .map_err(|err| anyhow::anyhow!("invalid endpoint id: {err}"))?;

            println!("\ndialing {peer} …\n");
            let mut events = node.connect(peer, payload);
            let mut failed = false;
            while let Some(event) = events.next().await {
                match event {
                    ConnectEvent::Connected => println!("connected"),
                    ConnectEvent::Sent { bytes_sent } => println!("sent {bytes_sent} byte(s)"),
                    ConnectEvent::Received {
                        bytes_received,
                        text,
                    } => println!("received {bytes_received} byte(s) back: {text:?}"),
                    ConnectEvent::Closed { error } => match error {
                        Some(err) => {
                            failed = true;
                            println!("closed with error: {err}");
                        }
                        None => println!("closed"),
                    },
                }
            }
            node.shutdown().await?;
            if failed {
                anyhow::bail!("echo exchange failed");
            }
        }
    }

    Ok(())
}
