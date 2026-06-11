use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::{env, fs};

use anyhow::{Context, Result};
use serde::Deserialize;
use tokio::net::UdpSocket;

use futuresdr::prelude::*;

use lora::meshtastic::MeshtasticConfig;
use lora::{ChannelProcessor, Node, Pos2d};

/// UDP port on which the channel listens for live node-position updates from
/// the GUI (sent when a node is dragged on the topology map).
const CONTROL_PORT: u16 = 17000;
/// GUI pixels per channel distance unit (must match node creation below).
const POS_SCALE: f32 = 100.0;

#[derive(Deserialize)]
struct PosUpdate {
    local_port: u16,
    x: f32,
    y: f32,
}

/// Background task: receive `{ "local_port", "x", "y" }` datagrams and update
/// the shared live positions used by the channel for path-loss.
async fn position_control_task(
    positions: Arc<Mutex<Vec<Pos2d>>>,
    port_to_idx: HashMap<u16, usize>,
    control_port: u16,
) {
    let sock = match UdpSocket::bind(("127.0.0.1", control_port)).await {
        Ok(s) => s,
        Err(e) => {
            eprintln!("position control: cannot bind {control_port}: {e}");
            return;
        }
    };
    println!("CONTROL port={control_port}");

    let mut buf = vec![0u8; 1024];
    loop {
        match sock.recv_from(&mut buf).await {
            Ok((n, _peer)) => {
                if let Ok(upd) = serde_json::from_slice::<PosUpdate>(&buf[..n]) {
                    if let Some(&idx) = port_to_idx.get(&upd.local_port) {
                        let mut p = positions.lock().unwrap();
                        if idx < p.len() {
                            p[idx] = Pos2d { x: upd.x / POS_SCALE, y: upd.y / POS_SCALE };
                        }
                    }
                }
            }
            Err(e) => {
                eprintln!("position control recv error: {e}");
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
        }
    }
}

#[derive(Deserialize)]
struct NodeConfig {
    local_port: u16,
    remote_port: u16,
    x: f32,
    y: f32,
    region: String,
    modem_preset: String,
    #[serde(default = "default_noise_std")]
    noise_std: f32,
}

fn default_noise_std() -> f32 { 2e-6 }

fn map_preset(s: &str) -> MeshtasticConfig {
    match s {
        "SHORT_TURBO" => MeshtasticConfig::ShortTurbo,
        "SHORT_FAST" => MeshtasticConfig::ShortFast,
        "SHORT_SLOW" => MeshtasticConfig::ShortSlow,
        "MEDIUM_FAST" => MeshtasticConfig::MediumFast,
        "MEDIUM_SLOW" => MeshtasticConfig::MediumSlow,
        "LONG_TURBO" => MeshtasticConfig::LongTurbo,
        "LONG_MODERATE" => MeshtasticConfig::LongModerate,
        _ => MeshtasticConfig::LongFast,
    }
}


fn map_region(region: &str) -> &'static str {
    match region {
        "EU_433" => "EU433",
        "EU_868" => "EU868",
        "RU" => "RU",
        _ => "RU",
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: channel_process <nodes.json>");
        std::process::exit(1);
    }

    let json = fs::read_to_string(&args[1])
        .with_context(|| format!("Cannot read {}", args[1]))?;

    let configs: Vec<NodeConfig> =
        serde_json::from_str(&json).context("Failed to parse nodes JSON")?;

    if configs.is_empty() {
        eprintln!("No nodes in JSON, exiting");
        return Ok(());
    }

    const SYNC_WORD: u8 = 0x2b;
    const SCALE: f32 = 100.0;
    let mut nodes: Vec<Node> = Vec::with_capacity(configs.len());

    // Kept clear of node web_port (9000+), tcp_port (4403) and local/remote
    // UDP ports (55554+) so the spectrum WebSocket never clashes with a
    // host-networked meshtasticd container (which would kill the flowgraph).
    const SPECTRUM_PORT_BASE: u16 = 18000;

    for (i, cfg) in configs.iter().enumerate() {
        let preset = map_preset(&cfg.modem_preset);
        let region = map_region(&cfg.region);
        let spectrum_port = SPECTRUM_PORT_BASE + i as u16;

        let node = Node::new(
            preset,
            region,
            0,
            SYNC_WORD,
            cfg.noise_std,
            false,
            cfg.remote_port,
            cfg.local_port,
            Pos2d { x: cfg.x/SCALE, y: cfg.y/SCALE },
            spectrum_port,
        )
        .with_context(|| {
            format!(
                "Failed to create node local={} remote={}",
                cfg.local_port, cfg.remote_port
            )
        })?;
        println!("noise std {}", cfg.noise_std);
        // Machine-readable line the GUI parses to map a node to its spectrum feed.
        println!("SPECTRUM node={} local_port={} port={}", i, cfg.local_port, spectrum_port);
        nodes.push(node);
    }
    let mut rt = Runtime::new();
    for node in &mut nodes{
        node.start(&mut rt, true).map_err(|e| anyhow::anyhow!("{}", e))?;
    }
    
    // Map local_port -> node index so the GUI can address position updates by
    // port regardless of node ordering.
    let port_to_idx: HashMap<u16, usize> = configs
        .iter()
        .enumerate()
        .map(|(i, c)| (c.local_port, i))
        .collect();

    println!("Channel simulation running with {} nodes", nodes.len());
    let processor = ChannelProcessor::new(nodes);
    let positions = processor.positions_handle();
    tokio::spawn(position_control_task(positions, port_to_idx, CONTROL_PORT));
    let _handle = processor.spawn_task();

    std::future::pending::<()>().await;
    Ok(())
}
