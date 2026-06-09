use core::time;
use std::sync::Arc;

use crossbeam_channel::{Receiver, Sender, unbounded};
use futuredsp::firdes;
use futuresdr::{async_io::Timer, blocks::{BlobToUdp, WebsocketSinkBuilder, WebsocketSinkMode, XlatingFir}, macros::connect, prelude::{Complex32, CpuBufferWriter, DefaultCpuReader, DefaultCpuWriter}, runtime::{BlockId, BlockRef, Flowgraph, FlowgraphHandle, Pmt, WrappedKernel, scheduler::SmolScheduler}, tracing::Instrument};
use futuresdr::runtime::Runtime;
use anyhow::Result;
use tokio::{net::UdpSocket, task::JoinHandle};

use crate::{AddAWGN, ChannelPublisher, ChannelSubscriber, Decoder, Deinterleaver, FftDemod, FrameSync, GrayMapping, HammingDecoder, HeaderDecoder, HeaderMode, IqFrame, SPECTRUM_IQ_BLOCK, Transmitter, default_values::ldro, frame_sync, meshtastic::MeshtasticConfig, utils::{
    Bandwidth, Channel, CodeRate, SpreadingFactor
}};

impl Clone for Node{
    fn clone(&self) -> Self {
        Node {
            preset: self.preset,
            channel: self.channel,
            bw: self.bw,           
            sf: self.sf,
            cr: self.cr,
            ldr: self.ldr,
            avail_slots: self.avail_slots,
            sync_word: self.sync_word,
            sigma: self.sigma,
            transmitters: Vec::new(), // drop heavy items
            selected_tx: self.selected_tx,
            fg: None,                // skip cloning flowgraph
            remote_port: self.remote_port,
            local_port: self.local_port,
            position: self.position.clone(), 
            tx_out: self.tx_out.clone(),
            rx_in: self.rx_in.clone(),
        }
    }
}


#[derive(Clone)]
pub struct Pos2d { pub x: f32, pub y: f32 }

pub struct Node {
    preset : MeshtasticConfig,

    pub channel : Channel,
    pub bw : Bandwidth,
    sf : SpreadingFactor,
    cr : CodeRate,
    ldr : bool,
    pub avail_slots : u8,

    sync_word : u8, 
    sigma : f32,
    
    //DSP interface
    transmitters: Vec<BlockRef<Transmitter>>,
    selected_tx: u8,
    pub fg : Option<Flowgraph>,

    //aka MAC interface
    remote_port: u16, // remote port
    pub local_port : u16, // node rcv port
    
    pub position: Pos2d,
    pub tx_out: Receiver<IqFrame>,   // channel processor reads node TX from here
    pub rx_in: Sender<IqFrame>,      // channel processor writes node RX to here
}

impl Node {
    pub fn new(
        sel_preset : MeshtasticConfig,
        region:&str,
        slot:u8,

        sync_word : u8,
        
        sigma : f32,
        implicit_header : bool,

        local_port: u16,
        remote_port: u16,
        position: Pos2d,
        spectrum_port: u16,

    ) -> Result<Self> {

        let mut fg = Flowgraph::new();

        let dest = format!("127.0.0.1:{}", remote_port);

        let (node_tx, tx_out) = unbounded::<IqFrame>();
        let (rx_in, node_rx) = unbounded::<IqFrame>();

        let subscriber =
        ChannelSubscriber::<DefaultCpuWriter<Complex32>>
        ::new(node_rx);

        let mut awgn = AddAWGN::<DefaultCpuReader<Complex32>, DefaultCpuWriter<Complex32>>::new(sigma, 42);
        awgn.output().set_min_buffer_size_in_items(65536);

        connect!(fg, subscriber > awgn);

        // Live spectrum tap: fan the post-channel RX signal (after path loss +
        // AWGN) out to a WebSocket as *raw complex IQ*. The GUI does its own
        // short-window STFT, so it can trade time vs frequency resolution live
        // (and actually resolve the LoRa chirp sweeps). FixedDropping keeps the
        // sink from ever back-pressuring the radio when no viewer is attached.
        // Port 0 disables the feed.
        if spectrum_port != 0 {
            let awgn_spec = awgn.clone();
            let ws = WebsocketSinkBuilder::<Complex32>::new(spectrum_port as u32)
                .mode(WebsocketSinkMode::FixedDropping(SPECTRUM_IQ_BLOCK))
                .build();
            connect!(fg, awgn_spec > ws);
        }

        let mut tx_vect = Vec::new();
        for preset in MeshtasticConfig::ALL{
            let (bw, sf, cr, channel, ldr, avail_slot) = preset.to_config(region, slot);
            let interpolation = match bw {
                Bandwidth::BW62 => 16,
                Bandwidth::BW125 => 8,
                Bandwidth::BW250 => 4,
                Bandwidth::BW500 => 2,
                _ => panic!("wrong bandwidth for Meshtastic"),
            };
            for slot_i in 0..avail_slot {
        
                if slot_i != slot {
                    continue;
                }
                
        
                let (bw, sf, cr, channel, ldr, _) = preset.to_config(region, slot_i);

                //rx graph pathes
                let awgn = awgn.clone();
                let udp_data = BlobToUdp::new(dest.clone());
                let frame_sync: FrameSync = FrameSync::new(
                    channel,
                    bw,
                    sf,
                    implicit_header,
                    vec![vec![16, 88]],
                    interpolation,
                    None,
                    Some("header_crc_ok"),
                    false,
                    None,
                );
                let fft_demod: FftDemod = FftDemod::new(sf, ldr);
                let gray_mapping: GrayMapping = GrayMapping::new();
                let deinterleaver: Deinterleaver = Deinterleaver::new(
                    ldr,
                    sf
                );
                let hamming_dec: HammingDecoder = HammingDecoder::new();
                let header_decoder: HeaderDecoder = HeaderDecoder::new(
                    HeaderMode::Explicit,
                    ldr,
                );
                let decoder: Decoder = Decoder::new();

                connect!(fg,
                    // rx graph
                    awgn > frame_sync;
                    frame_sync > fft_demod > gray_mapping > deinterleaver > hamming_dec > header_decoder;

                    frame_sync.kiss           | udp_data;
                    header_decoder.frame_info | frame_info.frame_sync;
                    header_decoder            | decoder;
                    header_decoder.kiss       | udp_data;
                    decoder.crc_check         | payload_crc_result.frame_sync;
                    decoder.kiss              | udp_data;
                );
            }
            let publisher = ChannelPublisher::<DefaultCpuReader<Complex32>>::new(
                node_tx.clone(),
                f64::from(channel),
                f32::from(bw),
            );
            // all tx pathes
           
            let tx: Transmitter = Transmitter::new(
                cr,
                true,
                sf,
                ldr,
                implicit_header,
                interpolation,
                vec![16, 88],
                8,
                10000,
            );
            connect!(fg, tx > publisher);
            tx_vect.push(tx);
        }
        
        println!("flowgraph started");

        let (bw, sf, cr, channel, ldr, avail) = sel_preset.to_config(region, slot);
       
        Ok(Self {
            preset: sel_preset,
            channel,
            bw,
            sf,
            cr,
            ldr,
            avail_slots: avail,
            sync_word,
            sigma,
            selected_tx: sel_preset as u8,
            fg: Some(fg),
            transmitters: tx_vect,
            remote_port,
            local_port,
            position,
            tx_out,
            rx_in,
        })
    }

    async fn set_preset(&mut self, preset :MeshtasticConfig, region:&str, slot:u8){
        (self.bw, self.sf, self.cr, self.channel, self.ldr, self.avail_slots) = preset.to_config(region, slot);
        self.preset = preset;
        self.selected_tx = preset as u8;
    }
   
    async fn server_task_body(mut handle: FlowgraphHandle, txs_ids : Vec<BlockId>, local_port: u16, mut selected_tx : u8) {
        use crate::kiss_driver::kiss;

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

                    // Parse KISS frame: [FEND, cmd, data..., FEND]
                    let is_config_cmd = payload.len() >= 2
                        && payload[0] == kiss::FEND
                        && payload[1] != kiss::CMD_DATA;

                    if is_config_cmd {
                        let cmd = payload[1];
                        eprintln!("KISS config cmd 0x{:02X} received, updating selected_tx if needed", cmd);
                    } else {
                        // CMD_DATA or raw frame — forward to transmitter as actual TX payload
                        if let Err(e) = handle.call(txs_ids[selected_tx as usize], "msg", Pmt::Blob(payload)).await {
                            eprintln!("flowgraph call error: {}", e);
                        }
                    }

                    if let Err(e) = socket.send_to(&resp, _peer).await {
                        eprintln!("error sending response: {}", e);
                    }
                }
                Err(e) => {
                    eprintln!("socket recv error: {}", e);
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                }
            }
        }
    }

    pub fn server_task_create(&self, handle: FlowgraphHandle) {
        let txs_ids: Vec<BlockId> = self.transmitters.iter().map(|tx| tx.into()).collect();
        let local_port = self.local_port;

        tokio::spawn(
            Self::server_task_body(handle, txs_ids, local_port, self.selected_tx)
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
    

    // pub fn get_sample_rate(self) -> u32 {
    //     if matches!(self.bw, Bandwidth::BW62) {
    //         return 62500*self.oversampling as u32;
    //     }
    //     if matches!(self.bw, Bandwidth::BW125) {
    //         return 125000*self.oversampling as u32;
    //     }
    //     if matches!(self.bw, Bandwidth::BW250) {
    //         return 250000*self.oversampling as u32;
    //     }
    //     if matches!(self.bw, Bandwidth::BW500) {
    //         return 500000*self.oversampling as u32;
    //     }
    //     return 0;
    // }

}