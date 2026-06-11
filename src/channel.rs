use futuresdr::num_complex::Complex32;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use crate::{IqFrame, Node, Pos2d};

/// Global sample rate in Hz (must match the rate used by all transmitters)
const GLOBAL_SAMPLE_RATE: f64 = 1_000_000.0;
const MAX_FRAME_LEN: usize = 1024;

struct Frame {
    frame: IqFrame,
    tx_idx: usize,
}
// --- Add this struct definition somewhere (outside the loop) ---
#[derive(Clone)]
struct RxFilter {
    taps: Vec<f32>,          // real coefficients (low‑pass)
    delay: Vec<Complex32>,   // delay line for I/Q
    index: usize,
}

impl RxFilter {
    fn new(cutoff_norm: f32, num_taps: usize) -> Self {
        let mut taps = Vec::with_capacity(num_taps);
        let m = (num_taps - 1) as f32;
        let fc = cutoff_norm;
        for n in 0..num_taps {
            let nf = n as f32;
            let h = if nf == m / 2.0 {
                2.0 * fc
            } else {
                let sinc = (std::f32::consts::TAU * fc * (nf - m / 2.0)).sin()
                         / (std::f32::consts::TAU * fc * (nf - m / 2.0));
                sinc * (2.0 * fc)
            };
            let window = 0.54 - 0.46 * (std::f32::consts::TAU * nf / m).cos();
            taps.push(h * window);
        }
        Self {
            taps,
            delay: vec![Complex32::new(0.0, 0.0); num_taps],
            index: 0,
        }
    }

    fn process(&mut self, input: Complex32) -> Complex32 {
        self.delay[self.index] = input;
        self.index = (self.index + 1) % self.delay.len();

        let mut out = Complex32::new(0.0, 0.0);
        let len = self.taps.len();
        for (k, &tap) in self.taps.iter().enumerate() {
            let idx = (self.index + len - 1 - k) % len;
            out = out + self.delay[idx] * tap;
        }
        out
    }
}
pub struct ChannelProcessor {
    nodes: Vec<Node>,
    pending_frames: BTreeMap<u64, Vec<Frame>>,
    /// Live node positions, shared so an external control task
    positions: Arc<Mutex<Vec<Pos2d>>>,
}

impl ChannelProcessor {
    pub fn new(nodes: Vec<Node>) -> Self {
        let positions = nodes.iter().map(|n| n.position.clone()).collect();
        Self {
            nodes,
            pending_frames: BTreeMap::new(),
            positions: Arc::new(Mutex::new(positions)),
        }
    }

    /// Handle to the live positions, for a control task to update on the fly.
    pub fn positions_handle(&self) -> Arc<Mutex<Vec<Pos2d>>> {
        Arc::clone(&self.positions)
    }

    fn distance(a: &Pos2d, b: &Pos2d) -> f32 {
        let dx = a.x - b.x;
        let dy = a.y - b.y;
        (dx * dx + dy * dy).sqrt()
    }

    async fn run(&mut self) {
        loop {
            for (idx, node) in self.nodes.iter().enumerate() {
                match node.tx_out.try_recv() {
                    Ok(frame) => {
                        self.pending_frames
                            .entry(frame.start_index)
                            .or_default()
                            .push(Frame { frame, tx_idx: idx });
                    },
                    Err(_) => {continue;}
                }
            }
            let Some((&start_idx, frames)) = self.pending_frames.iter().next() else {
                tokio::task::yield_now().await;
                continue;
            };

            // Snapshot the live positions once per frame group (drag updates are
            // rare, so cloning a few Pos2d here is negligible).
            let pos = self.positions.lock().unwrap().clone();

            for (rx_idx, receiver_node) in self.nodes.iter().enumerate() {
                let mut txbuf = vec![Complex32::new(0.0, 0.0); 1024];
                let rx_freq = f64::from(receiver_node.channel);
                let rx_bw = f64::from(receiver_node.bw);
                for frame in frames {
                    let d = ChannelProcessor::distance(&pos[rx_idx], &pos[frame.tx_idx]);
                    if d == 0.0 {
                        continue;
                    }

                    let attenuation : Complex32 = (Complex32::new(1.0, 1.0) * (d + 1.0).powi(-3)) / 1.41421356237;
                 
                    // let fs = GLOBAL_SAMPLE_RATE as f32;
                    // let rx_fc = rx_freq as f32;
                    // let rx_bw = rx_bw as f32;
                    // let tx_fc = f64::from(xmit_node.channel);
                    // let tx_bw = f64::from(xmit_node.bw);

                    // let delta_f = rx_fc - tx_fc;
                    // let phase_step = std::f32::consts::TAU * delta_f / fs;

                    // let step_re = phase_step.cos();
                    // let step_im = phase_step.sin();

                    // let mut phasor_re = 1.0_f32;
                    // let mut phasor_im = 0.0_f32;

                    for i in 0..1024 {
                        // let s = frame.frame.samples[i];

                        // // complex multiply: s * phasor
                        // let re = s.re * phasor_re - s.im * phasor_im;
                        // let im = s.re * phasor_im + s.im * phasor_re;
                        // let shifted = Complex32::new(re, im);

                        // apply attenuation and accumulate
                        txbuf[i] +=  frame.frame.samples[i] * attenuation;

                        // // advance phasor: phasor *= step
                        // let next_re = phasor_re * step_re - phasor_im * step_im;
                        // let next_im = phasor_re * step_im + phasor_im * step_re;
                        // phasor_re = next_re;
                        // phasor_im = next_im;
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
