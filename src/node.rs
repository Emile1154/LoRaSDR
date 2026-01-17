use crossbeam_channel::{Receiver, Sender};
use futuresdr::{async_io::Timer, blocks::BlobToUdp, macros::connect, prelude::{Complex32, DefaultCpuReader, DefaultCpuWriter}, runtime::{BlockId, Flowgraph, FlowgraphHandle, Pmt, scheduler::SmolScheduler}};
use futuresdr::runtime::Runtime;
use anyhow::Result;
use tokio::net::UdpSocket;

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
    transmitter: BlockId,

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

        let rt = Runtime::new();
       
        // server task create  
        let tx = transmitter.into();
        
        let (_fg, handle) = rt.start_sync(fg)?;
        
        // let handle_for_task = handle.clone();
        tokio::spawn(Self::server_task_body(handle, tx, local_port));
        
       
        Ok(Self {
            channel,
            bw,
            sf,
            sync_word,
            oversampling,
            sigma,
            // flowgraph: fg,
            transmitter: tx,
            remote_port,
            local_port,
        })
    }

    async fn server_task_body(mut handle: FlowgraphHandle, tx: BlockId, local_port: u16) {
        let src = format!("127.0.0.1:{}", local_port);
        let socket= match UdpSocket::bind(src).await {
            Ok(s) => s,
            Err(e) => {
                eprintln!("cannot bind socket on port: {}: {}", local_port, e);
                return;
            }
        };
       
        let mut buf = vec![0u8; 1500];
        loop {
            match socket.recv_from(&mut buf).await {
                Ok((n, _peer)) => {
                    let payload = buf[..n].to_vec();
                    if let Err(e) = handle.call(tx, "msg", Pmt::Blob(payload.clone())).await {
                        eprintln!("flowgraph call error: {}", e);
                    }
                }
                Err(e) => {
                    eprintln!("socket recv error: {}", e);
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                }   
            }            
        }
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