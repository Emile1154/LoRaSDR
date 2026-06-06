use std::{env, fs};

use anyhow::{Context, Result};
use serde::Deserialize;

use futuresdr::prelude::*;

use lora::meshtastic::MeshtasticConfig;
use lora::{ChannelProcessor, Node, Pos2d};

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

    for cfg in &configs {
        let preset = map_preset(&cfg.modem_preset);
        let region = map_region(&cfg.region);

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
        )
        .with_context(|| {
            format!(
                "Failed to create node local={} remote={}",
                cfg.local_port, cfg.remote_port
            )
        })?;
        println!("noise std {}", cfg.noise_std);
        nodes.push(node);
    }
    let mut rt = Runtime::new();
    for node in &mut nodes{
        node.start(&mut rt, true).map_err(|e| anyhow::anyhow!("{}", e))?;
    }
    
    println!("Channel simulation running with {} nodes", nodes.len());
    let _handle = ChannelProcessor::new(nodes).spawn_task();
  
    std::future::pending::<()>().await;
    Ok(())
}
