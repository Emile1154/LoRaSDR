use futuresdr::prelude::*;
use crossbeam_channel::{Sender, Receiver, bounded};
use std::collections::VecDeque;
use tokio::fs::File;
use tokio::io::AsyncWriteExt;
use std::sync::{Arc, Mutex};
use std::thread;

#[repr(C)]
#[derive(Debug, Clone)]
pub struct IqFrame {
    pub epoch: u64,
    pub samples: [Complex32; 1024],
}

#[derive(Block)]
pub struct ChannelPublisher<I = DefaultCpuReader<Complex32>>
where
    I: CpuBufferReader<Item = Complex32>,
{
    #[input]
    input: I,

    sender: Option<Sender<IqFrame>>,
    n: usize,
    epoch: u64,
    tmpbuf: Vec<Complex32>,
    // // Synchronization fields
    // ready_sender: Option<Sender<usize>>,
    // trigger_receiver: Option<Receiver<bool>>,
    // transceiver_id: usize,
    // sync_enabled: bool,
}

impl<I> ChannelPublisher<I>
where
    I: CpuBufferReader<Item = Complex32>,
{
    pub fn new(sender: Sender<IqFrame>) -> Self {
        return Self {
            input: I::default(),
            sender: Some(sender),
            n: 0,
            epoch: 0,
            tmpbuf: vec![Complex32::default(); 1024],
            // ready_sender: None,
            // trigger_receiver: None,
            // transceiver_id: 0,
            // sync_enabled: false,
        }
    }

    // pub fn with_sync(mut self, ready_sender: Sender<usize>, trigger_receiver: Receiver<bool>, transceiver_id: usize) -> Self {
    //     self.ready_sender = Some(ready_sender);
    //     self.trigger_receiver = Some(trigger_receiver);
    //     self.transceiver_id = transceiver_id;
    //     self.sync_enabled = true;
    //     self
    // }
}

impl<I> Kernel for ChannelPublisher<I>
where
    I: CpuBufferReader<Item = Complex32>,
{
    async fn work(
        &mut self,
        io: &mut WorkIo,
        _mio: &mut MessageOutputs,
        _b: &mut BlockMeta,
    ) -> Result<()> {
        
        // Synchronization: send ready signal and wait for trigger
        // if self.sync_enabled {
        //     if let Some(ref ready_sender) = self.ready_sender {
        //         // Send ready signal
        //         let _ = ready_sender.send(self.transceiver_id);
        //     }

        //     // Wait for trigger signal
        //     if let Some(ref trigger_receiver) = self.trigger_receiver {
        //         let _ = trigger_receiver.recv();
        //     }
        // }
        
        // io.call_again = true;
        
        let input_slice = self.input.slice();
        let available = input_slice.len();
        
        let mut processed = 0;
        let mut space_left = 1024 - self.n;
        
        // Process available samples
        if available > 0 {
            let to_process = std::cmp::min(available, space_left);
            
            for i in 0..to_process {
                self.tmpbuf[self.n + i] = input_slice[i];
            }
            processed = to_process;
            self.n += processed;
            space_left -= to_process;
        }
        
        // Pad with null values if we don't have enough samples to fill the frame
        if self.n < 1024 && !input_slice.is_empty() {
            // Continue processing if we still have space and samples available
        } else if self.n < 1024 && input_slice.is_empty() {
            // Pad remaining space with null values (0 + 0j)
            for i in self.n..1024 {
                self.tmpbuf[i] = Complex32::default(); // This represents 0 + 0j
            }
            self.n = 1024;
        }
        
        if self.n >= 1024 {
            let frame = IqFrame {
                epoch: self.epoch,
                samples: {
                    let mut samples = [Complex32::default(); 1024];
                    samples.copy_from_slice(&self.tmpbuf[..1024]);
                    samples
                },
            };
        
            if let Some(ref sender) = self.sender {
                // println!("Sending frame");
                if sender.send(frame).is_err() {
                    // Handle send error gracefully
                    println!("Failed to send frame");
                }
            }
            
            self.n = 0;
            self.epoch += 1;
            // Clear the temp buffer for the next frame
            for i in 0..self.tmpbuf.len() {
                self.tmpbuf[i] = Complex32::default();
            }
        }
        
        // Consume the input data that was processed
        self.input.consume(processed);
        
        Ok(())
    }
}

#[derive(Block)]
pub struct ChannelSubscriber<O = DefaultCpuWriter<Complex32>>
where
    O: CpuBufferWriter<Item = Complex32>,
{
    #[output]
    output: O,

    receiver: Option<Receiver<IqFrame>>,
    current: Option<IqFrame>,
    pos: usize,
}

impl <O> ChannelSubscriber<O>
where
    O: CpuBufferWriter<Item = Complex32>,
{
    pub fn new(receiver: Receiver<IqFrame>) -> Self {
        // let log_path = "/home/user/src/FutureSDR/examples/lora/tests/samples2.txt";
        // let log_file = tokio::fs::File::create(log_path);

        return Self {
            output: O::default(),
            receiver: Some(receiver),
            current: None,
            pos: 0
        }
    }
    
}

impl<O> Kernel for ChannelSubscriber<O>
where
    O: CpuBufferWriter<Item = Complex32>,
{
    async fn work(
        &mut self,
        io: &mut WorkIo,
        _mio: &mut MessageOutputs,
        _b: &mut BlockMeta,
    ) -> Result<()> {
        // io.call_again = true;

        let produced;

        {
            let out = self.output.slice();
            let mut written = 0;

            while written < out.len() {
                if self.current.is_none() {
                    if let Some(ref rx) = self.receiver {
                        if let Ok(frame) = rx.recv() {
                            self.current = Some(frame);
                            self.pos = 0;
                        }
                    }
                }

                if let Some(ref frame) = self.current {
                    
                    let avail = 1024 - self.pos;
                    let n = std::cmp::min(avail, out.len() - written);

                    out[written..written + n]
                        .copy_from_slice(&frame.samples[self.pos..self.pos + n]);

                    self.pos += n;
                    written += n;

                    if self.pos >= 1024 {
                        self.current = None;
                    }
                } else {
                    for s in &mut out[written..] {
                        *s = Complex32::default();
                    }
                    written = out.len();
                }
            }

            produced = out.len();
        } // ← out DROPPED HERE

        self.output.produce(produced);
        Ok(())
    }


}

