use futuresdr::prelude::*;
use tokio::task::JoinHandle;
use std::collections::{BTreeMap};
use crossbeam_channel::{Receiver, Sender};
use crate::IqFrame;

struct Frame{
    frame: IqFrame,
    sender_id: usize
}

pub struct ChannelProcessor {
    
    tx_nodes: Vec<Receiver<IqFrame>>,
    rx_nodes: Vec<Sender<IqFrame>>,
    
    d_matrix: Vec<Vec<f32>>,
    buffer: BTreeMap<u64, Vec<Frame>>,
}

impl ChannelProcessor {

    pub fn new(
        tx_nodes_sub: Vec<Receiver<IqFrame>>,
        rx_nodes_pub: Vec<Sender<IqFrame>>,     
        d_matrix: Vec<Vec<f32>>,
    ) -> Self
    {
        Self {      
            tx_nodes: tx_nodes_sub,
            rx_nodes: rx_nodes_pub,
           
            d_matrix: d_matrix,
            buffer: BTreeMap::new(),
        }
    }
    async fn process(&mut self) -> Result<()> {
        loop {
          
            for (id, rx) in self.tx_nodes.iter().enumerate() {
                let frame = rx.recv().expect("TX disconnected");
                self.buffer
                    .entry(frame.epoch)
                    .or_default()
                    .push(Frame {
                        frame,
                        sender_id: id,
                    });
            }

            let (&epoch, frames) = self.buffer.iter().next().unwrap();

            
            if frames.len() < self.tx_nodes.len() {
                continue;
            }

            
            for rx_id in 0..self.rx_nodes.len() {
                let mut txbuf = vec![Complex32::new(0.0, 0.0); 1024];

                for frame in frames {
                
                    let d_nm = self.d_matrix[frame.sender_id][rx_id];
                    
                    let c_nm : Complex32 = (Complex32::new(1.0, 1.0) * (d_nm+1.0).powi(-3)) / 1.41421356237;  
                  
                    for i in 0..1024 {
                        txbuf[i] += frame.frame.samples[i] * c_nm ;
                    }
                }

                self.rx_nodes[rx_id]
                    .send(IqFrame { epoch, samples: txbuf.try_into().unwrap() })
                    .unwrap();
            }

           
            self.buffer.remove(&epoch);
        }
    }

    
    pub fn spawn_task(mut self) -> JoinHandle<i32>
    where
        Self: Send + 'static,
    {
        tokio::spawn(async move {
            match self.process().await {
                Ok(_) => 0,                  
                Err(e) => {
                    eprintln!("process task error: {:#}", e);
                    1                   
                }
            }
        })
    }
}

