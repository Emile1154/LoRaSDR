use futuresdr::prelude::*;

use crossbeam_channel::{unbounded};
use lora::utils::{Channel, Bandwidth, SpreadingFactor};
use lora::{ChannelProcessor, IqFrame, Node};
use lora::meshtastic::MeshtasticConfig;

#[tokio::main]
async fn main() -> Result<()> {
    let (tx_node_pub, tx_node_sub) = unbounded::<IqFrame>();
    let (rx_node_pub, rx_node_sub) = unbounded::<IqFrame>();

    let (tx_node_pub2, tx_node_sub2) = unbounded::<IqFrame>();
    let (rx_node_pub2, rx_node_sub2) = unbounded::<IqFrame>();
    
    // let channel = Channel::EU868_1;
    // let bandwidth = Bandwidth::BW125;
    // let spreading_factor = SpreadingFactor::SF7;
    let sync_word:u8 = 0x12;
    let oversampling = 8; 
    let noise_std =2e-2;

    let (bandwidth, spreading_factor, _, channel, ldro) = MeshtasticConfig::LongFastEu.to_config();
    
    let n = 2;
    let mut d_matrix=vec![vec![0f32; n]; n];
    for i in 0..n { d_matrix[i][i] = 0.1; }
    d_matrix[0][1] = 25.0;
    d_matrix[1][0] = 25.0; 

    let tx_nodes = vec![tx_node_sub, tx_node_sub2];
    let rx_nodes = vec![rx_node_pub, rx_node_pub2];

    let cm = ChannelProcessor::new(tx_nodes, rx_nodes, d_matrix);
    tokio::spawn(async move{
        let _ = cm.spawn_task().await;
    });
 
   
    let node = Node::new(
        channel,
        bandwidth,
        spreading_factor,
        ldro,
        sync_word,
        oversampling,
        noise_std,
        false,
        rx_node_sub,
        tx_node_pub,
        55554,
        55555,
        
    );
    let node2 = Node::new(
        channel,
        bandwidth,
        spreading_factor,
        ldro,
        sync_word,
        oversampling,
        noise_std,
        false,
        rx_node_sub2,
        tx_node_pub2,
        55556,
        55557,
    );
    loop{}
    println!("Single flowgraph completed successfully!");
    Ok(())
}