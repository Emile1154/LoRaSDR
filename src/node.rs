use core::time;
use std::sync::Arc;

use crossbeam_channel::{Receiver, Sender};
use futuresdr::{async_io::Timer, blocks::BlobToUdp, macros::connect, prelude::{Complex32, DefaultCpuReader, DefaultCpuWriter}, runtime::{BlockId, BlockRef, Flowgraph, FlowgraphHandle, Pmt, WrappedKernel, scheduler::SmolScheduler}, tracing::Instrument};
use futuresdr::runtime::Runtime;
use anyhow::Result;
use tokio::{net::UdpSocket, task::JoinHandle};

use crate::{AddAWGN, ChannelPublisher, ChannelSubscriber, Decoder, Deinterleaver, FftDemod, FrameSync, GrayMapping, HammingDecoder, HeaderDecoder, HeaderMode, IqFrame, Transmitter, utils::{
    Bandwidth, Channel, CodeRate, SpreadingFactor
}};


pub struct Node {
    
    //lora phy settings
    channel : Channel,
    bw : Bandwidth,
    sf : SpreadingFactor,
    sync_word : u8, 
    oversampling : usize,
    sigma : f32,
    
    //DSP interface
    transmitter: BlockRef<Transmitter>,
    pub fg : Option<Flowgraph>,

    //aka MAC interface
    remote_port: u16, // remote port
    local_port : u16, // node rcv port
}

impl Node {
    pub fn new(
        channel : Channel,
        bw : Bandwidth,
        sf : SpreadingFactor,
        ldro : bool,
        sync_word : u8,
        oversampling : usize,
        sigma : f32,
        implicit_header : bool,

        receiver: Receiver<IqFrame>,
        sender: Sender<IqFrame>,

        local_port: u16,
        remote_port: u16,

        // rt : Runtime<'_, SmolScheduler>,
    ) -> Result<Self> {
        //TODO: coderate setup
        //rx graph
        let subscriber =
        ChannelSubscriber::<DefaultCpuWriter<Complex32>>
        ::new(receiver);
        
        let awgn = AddAWGN
        ::<DefaultCpuReader<Complex32>, DefaultCpuWriter<Complex32>>
        ::new(sigma, 42);
        // let throttle = futuresdr::blocks::Throttle::<Complex32>::new(samplerate as f64);

        let frame_sync: FrameSync = FrameSync::new(
            channel,
            bw,
            sf,
            implicit_header,
            vec![vec![sync_word.into()]],
            oversampling,
            None,
            Some("header_crc_ok"),
            false,
            None,
        );

        let fft_demod: FftDemod = FftDemod::new(sf, ldro);
        let gray_mapping: GrayMapping = GrayMapping::new();
        let deinterleaver: Deinterleaver = Deinterleaver::new(
            ldro,
            sf
        );
        let hamming_dec: HammingDecoder = HammingDecoder::new();
        let header_decoder: HeaderDecoder = HeaderDecoder::new(
            HeaderMode::Explicit,
            ldro,
        );
        let decoder: Decoder = Decoder::new();
        let dest = format!("127.0.0.1:{}", remote_port);
        let udp_data: BlobToUdp = BlobToUdp::new(dest);

        //tx graph
        let transmitter: Transmitter = Transmitter::new(
            CodeRate::CR_4_5,
            true,
            sf,
            ldro,
            implicit_header,
            oversampling,
            vec![sync_word as usize],
            8,
            0,
        );
        let publisher = ChannelPublisher
        ::<DefaultCpuReader<Complex32>>
        ::new(sender);

        //flowgraph connection
        let mut fg = Flowgraph::new();
        
        connect!(fg,
            // rx graph 
            subscriber > awgn > frame_sync > fft_demod > gray_mapping > deinterleaver > hamming_dec > header_decoder;
            header_decoder.frame_info | frame_info.frame_sync;
            header_decoder | decoder;
            decoder.rftap | udp_data;
            // tx graph
            transmitter > publisher;
        );    

        
        println!("flowgraph started");
        
        Ok(Self {
            channel,
            bw,
            sf,
            sync_word,
            oversampling,
            sigma,
            fg: Some(fg),
            transmitter: transmitter,
            remote_port,
            local_port,
        })
    }

    async fn server_task_body(mut handle: FlowgraphHandle, tx_id : BlockId, local_port: u16) {
        let src = format!("127.0.0.1:{}", local_port);
        let socket= match UdpSocket::bind(src).await {
            Ok(s) => s,
            Err(e) => {
                eprintln!("cannot bind socket on port: {}: {}", local_port, e);
                return;
            }
        };
       
        let mut buf = vec![0u8; 1500];
        let resp = [0xC0, 0x0F, 0x00, 0xC0];
        println!("thread running, listen port: {}", local_port);
        loop {
            match socket.recv_from(&mut buf).await {
                Ok((n, _peer)) => {
                    let payload = buf[..n].to_vec();
                    for i in 0..n {
                        print!("{} ", payload[i]);
                    }
                    println!(" payload len {}", n);
                    if let Err(e) = handle.call(tx_id, "msg", Pmt::Blob(resp.to_vec())).await {
                        eprintln!("flowgraph call error: {}", e);
                    }
                    
                    // Check if the block has finished
                    println!("peer ip: {}:{}",  _peer.ip(), _peer.port());
                    if let Err(e) = socket.send_to(&resp, _peer).await {
                        eprintln!("error sending response: {}", e);
                    }
                    println!("TRANSMISSION FINISHED");
                }
                Err(e) => {
                    eprintln!("socket recv error: {}", e);
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                }   
            }            
        }
    }

    pub fn server_task_create(&self, handle: FlowgraphHandle) {
        let tx_id = self.transmitter.clone().into();
        let local_port = self.local_port;

        tokio::spawn(
            Self::server_task_body(handle, tx_id, local_port)
        );
    }

    pub fn start(
        &mut self,
        rt: &mut Runtime<'_, SmolScheduler>,
        enabled: bool,
    ) -> Result<(), Box<dyn std::error::Error>> {

        let fg = self.fg.take().expect("Flowgraph already started");

        let (_fg, handle) = rt.start_sync(fg)?;

        if enabled {
            self.server_task_create(handle);
        }

        Ok(())
    }
    

    pub fn get_sample_rate(self) -> u32 {
        if matches!(self.bw, Bandwidth::BW62) {
            return 62500*self.oversampling as u32;
        }
        if matches!(self.bw, Bandwidth::BW125) {
            return 125000*self.oversampling as u32;
        }
        if matches!(self.bw, Bandwidth::BW250) {
            return 250000*self.oversampling as u32;
        }
        if matches!(self.bw, Bandwidth::BW500) {
            return 500000*self.oversampling as u32;
        }
        return 0;
    }

}