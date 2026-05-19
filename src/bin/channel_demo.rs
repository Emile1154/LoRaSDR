use futuresdr::prelude::*;

use lora::{ChannelProcessor, Node, Pos2d};
use lora::meshtastic::MeshtasticConfig;

#[tokio::main]
async fn main() -> Result<()> {
    let sync_word: u8 = 0x2b;
    let noise_std = 2e-6;

    let mut node = Node::new(
        MeshtasticConfig::LongFast,
        "RU",
        0,
        sync_word,
        noise_std,
        false,
        55554,
        55555,
        Pos2d { x: 0.0, y: 25.0 }
    ).unwrap();

    let mut node2 = Node::new(
        MeshtasticConfig::LongFast,
        "RU",
        0,
        sync_word,
        noise_std,
        false,
        55556,
        55557,
        Pos2d { x: 0.0, y: 0.0 }
    ).unwrap();

    let mut rt = Runtime::new();

    node.start(&mut rt, true).unwrap();
    node2.start(&mut rt, true).unwrap();

    let _handle = ChannelProcessor::new(vec![node, node2]).spawn_task();

    loop {}
}
