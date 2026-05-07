use futuresdr::num_complex::Complex32;
use std::collections::BTreeMap;
use crate::{IqFrame, Node};

/// Global sample rate in Hz (must match the rate used by all transmitters)
const GLOBAL_SAMPLE_RATE: f64 = 1_000_000.0;
const MAX_FRAME_LEN: usize = 1024;

struct Frame {
    frame: IqFrame,
    node: Node,
}

pub struct ChannelProcessor {
    nodes: Vec<Node>,
    pending_frames: BTreeMap<u64, Vec<Frame>>,
}

impl ChannelProcessor {
    pub fn new(nodes: Vec<Node>) -> Self {
        Self {
            nodes,
            pending_frames: BTreeMap::new(),
        }
    }

    async fn distance(a: &crate::Pos2d, b: &crate::Pos2d) -> f32 {
        let dx = a.x - b.x;
        let dy = a.y - b.y;
        (dx * dx + dy * dy).sqrt()
    }

    async fn run(&mut self) {
        loop {
            for node in self.nodes.iter() {
                match node.tx_out.try_recv() {
                    Ok(frame) => {
                        self.pending_frames
                            .entry(frame.start_index)
                            .or_default()
                            .push(Frame { frame, node: node.clone() });
                    },
                    Err(_) => {continue;}
                }
            }
            let Some((&start_idx, frames)) = self.pending_frames.iter().next() else {
                tokio::task::yield_now().await;
                continue;
            };

            for receiver_node in self.nodes.iter() {
                let mut txbuf = vec![Complex32::new(0.0, 0.0); 1024];
                let rx_freq = f64::from(receiver_node.channel);

                for frame in frames {
                    let xmit_node = &frame.node;

                    if xmit_node.local_port == receiver_node.local_port {
                        continue;
                    }
                    let delta_f = (frame.frame.fc - rx_freq).abs();
                    let bw_rx = frame.frame.bw as f64;
                    if delta_f > bw_rx / 2.0 {
                        // signal is outside receiver bandwidth
                        continue;
                    }

                    // let d = ChannelProcessor::distance(&receiver_node.position, &xmit_node.position).await;
                    
                    // let attenuation : Complex32 = (Complex32::new(1.0, 1.0) * (d + 1.0).powi(-3)) / 1.41421356237;

                    for i in 0..1024 {
                        // let global_sample_index = start_idx * 1024 + i as u64;
                        // let t = global_sample_index as f64 / GLOBAL_SAMPLE_RATE;
                        // let phase = 2.0 * std::f64::consts::PI * (frame.frame.fc - rx_freq) * t;
                        // let shift = Complex32::new(phase.cos() as f32, phase.sin() as f32);
                        txbuf[i] += frame.frame.samples[i] ;//* attenuation; //* shift;
                    }
                }

                let xmit_frame = IqFrame {
                    start_index: start_idx,
                    samples : {
                        let mut samples = [Complex32::default(); 1024];
                        samples.copy_from_slice(&txbuf[..1024]);
                        samples
                    },
                    fc: 0.0,
                    bw: 0.0,
                };
                if let Err(e) = receiver_node.rx_in.send(xmit_frame) {
                    println!("send error for recvnode.local_port: {}, error: {}", receiver_node.local_port , e);
                }
            }
            
            self.pending_frames.remove(&start_idx);
        }
    }

    pub fn spawn_task(mut self) -> tokio::task::JoinHandle<i32>
    where
        Self: Send + 'static,
    {
        tokio::spawn(async move {
            self.run().await;
            0
        })
    }
}
