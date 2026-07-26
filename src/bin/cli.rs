//! Native peer for the browser gossip demo.
//!
//! Built from exactly the same `chat` module the wasm build uses, so a terminal
//! participant and a browser tab share one gossip swarm.
//!
//!   cargo run --features cli -- join <ROOM>
//!   cargo run --features cli -- join <ROOM> --bootstrap <ENDPOINT_ID> --nick alice
//!
//! Type a line and press enter to broadcast it.

use anyhow::Result;
use clap::Parser;
use iroh::EndpointId;
use iroh_wasm_demo::chat::{ChatEvent, ChatNode, topic_for_room};
use n0_future::StreamExt;
use tokio::io::{AsyncBufReadExt, BufReader};

#[derive(Parser)]
#[command(about = "native peer for the iroh gossip browser demo")]
enum Cli {
    /// Join a chat room and relay stdin into it.
    Join {
        /// Room name. Hashed to a topic id, so all participants must use the
        /// same spelling.
        room: String,

        /// Endpoint id of a peer already on the topic. Repeatable. Omit if you
        /// are the first participant.
        #[arg(long, short)]
        bootstrap: Vec<String>,

        /// Display name attached to your messages.
        #[arg(long, short, default_value = "cli")]
        nick: String,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let Cli::Join {
        room,
        bootstrap,
        nick,
    } = Cli::parse();

    let node = ChatNode::spawn().await?;
    println!("our endpoint id: {}", node.endpoint().id());
    print!("waiting to come online … ");
    node.wait_online().await;
    println!("ok");

    let info = node.info();
    println!("relays: {:?}", info.relays);

    let topic = topic_for_room(&room);
    println!("room:   {room:?}");
    println!("topic:  {topic}");

    let peers: Vec<EndpointId> = bootstrap
        .iter()
        .map(|s| {
            s.trim()
                .parse()
                .map_err(|err| anyhow::anyhow!("invalid endpoint id {s:?}: {err}"))
        })
        .collect::<Result<_>>()?;

    if peers.is_empty() {
        println!("\nno bootstrap peers — you are the first here. Others can join with:");
        println!(
            "  cargo run --features cli -- join {room:?} --bootstrap {}",
            node.endpoint().id()
        );
    }

    let chat = node.join(topic, peers, nick.clone()).await?;
    println!("\njoined. type a message and press enter. ctrl-c to quit.\n");

    // Print incoming events.
    let mut events = chat.events();
    tokio::spawn(async move {
        while let Some(event) = events.next().await {
            match event {
                ChatEvent::NeighborUp { endpoint_id } => {
                    println!("* {endpoint_id} joined the swarm")
                }
                ChatEvent::NeighborDown { endpoint_id } => {
                    println!("* {endpoint_id} left the swarm")
                }
                ChatEvent::Message {
                    nickname, text, ..
                } => println!("<{nickname}> {text}"),
                ChatEvent::Lagged => println!("* lagged — some messages were dropped"),
                ChatEvent::Error { error } => println!("* error: {error}"),
            }
        }
    });

    // Relay stdin into the topic.
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        chat.broadcast(line.clone()).await?;
        // Gossip does not echo our own messages back, so print locally.
        println!("<{nick}> {line}");
    }

    node.shutdown().await?;
    Ok(())
}
