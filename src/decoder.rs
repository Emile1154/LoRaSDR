use futuresdr::prelude::*;
use std::collections::HashMap;

use crate::Frame;
use crate::utils::*;
use crate::kiss_driver::*;
#[derive(Block)]
#[message_inputs(r#in)]
#[message_outputs(out, out_annotated, kiss, crc_check)]
#[null_kernel]
pub struct Decoder;

impl Decoder {
    pub fn new() -> Self {
        Self
    }

    fn crc16(data: &[u8]) -> u16 {
        let mut crc: u16 = 0x0000;
        for byte in data.iter() {
            let mut new_byte = *byte;
            for _ in 0..8 {
                if ((crc & 0x8000) >> 8) as u8 ^ (new_byte & 0x80) != 0 {
                    crc = (crc << 1) ^ 0x1021;
                } else {
                    crc <<= 1;
                }
                new_byte <<= 1;
            }
        }
        crc
    }

    async fn decode(frame: &Frame, mio: &mut MessageOutputs) -> Option<Vec<u8>> {
        let mut dewhitened: Vec<u8> = vec![];
        let start = if frame.implicit_header { 0 } else { 5 };
        let end = if frame.has_crc {
            frame.nibbles.len() - 4
        } else {
            frame.nibbles.len()
        };

        let slice = &frame.nibbles[start..end];

        for (i, c) in slice.chunks_exact(2).enumerate() {
            let low_nib = c[0] ^ (WHITENING_SEQ[i] & 0x0F);
            let high_nib = c[1] ^ ((WHITENING_SEQ[i] & 0xF0) >> 4);
            dewhitened.push((high_nib << 4) | low_nib);
        }

        info!("..:: Decoder");

        let crc_passed = if frame.has_crc {
            let l = frame.nibbles.len();
            let low_nib = frame.nibbles[l - 4];
            let high_nib = frame.nibbles[l - 3];
            dewhitened.push((high_nib << 4) | low_nib);
            let low_nib = frame.nibbles[l - 2];
            let high_nib = frame.nibbles[l - 1];
            dewhitened.push((high_nib << 4) | low_nib);

            let l = dewhitened.len();
            if l < 4 {
                info!("crc check failed: payload length too small to compute crc");
                false
            } else {
                let mut crc = Self::crc16(&dewhitened[0..l - 4]);
                // XOR the obtained CRC with the last 2 data bytes
                crc = crc ^ dewhitened[l - 3] as u16 ^ ((dewhitened[l - 4] as u16) << 8);
                let crc_valid: bool =
                    ((dewhitened[l - 2] as u16) + ((dewhitened[l - 1] as u16) << 8)) as i32
                        == crc as i32;
                mio.post("crc_check", Pmt::Bool(crc_valid)).await.unwrap();
                if !crc_valid {
                    info!("crc check failed");
                    false
                } else {
                    info!("crc check passed");
                    true
                }
            }
        } else {
            true
        };

        let cmd_data = create_cmd(kiss::CMD_DATA, dewhitened.as_slice());
        mio.post("kiss", Pmt::Blob(cmd_data.clone())).await.unwrap();

        let mut crc_payload_ok = RADIOLIB_SX126X_IRQ_CRC_ERR;

        if crc_passed {
            crc_payload_ok = 0;
            let cmd_ready = create_cmd(kiss::CMD_READY, &[crc_payload_ok]);
            mio.post("kiss", Pmt::Blob(cmd_ready.clone())).await.unwrap();
            info!("DECODER received frame [bin]: {:02x?}", &dewhitened);
            Some(dewhitened)
        } else {
            let cmd_ready = create_cmd(kiss::CMD_READY, &[crc_payload_ok]);
            mio.post("kiss", Pmt::Blob(cmd_ready.clone())).await.unwrap();
            info!("DECODER FAILED frame [bin]: {:02x?}", &dewhitened);
            None
        }
        
    }

    async fn r#in(
        &mut self,
        io: &mut WorkIo,
        mio: &mut MessageOutputs,
        _meta: &mut BlockMeta,
        pmt: Pmt,
    ) -> Result<Pmt> {
        let ret = match pmt {
            Pmt::Any(a) => {
                if let Some(frame) = a.downcast_ref::<Frame>() {
                    if let Some(dewhitened) = Self::decode(frame, mio).await {
                        let mut annotated_payload: HashMap<String, Pmt> =
                            HashMap::<String, Pmt>::from([(
                                String::from("payload"),
                                Pmt::Blob(dewhitened.clone()),
                            )]);
                        annotated_payload.extend(frame.annotations.clone());
                        annotated_payload
                            .insert(String::from("code_rate"), Pmt::Usize(frame.code_rate));
                        annotated_payload.insert(String::from("has_crc"), Pmt::Bool(frame.has_crc));
                        annotated_payload.insert(
                            String::from("implicit_header"),
                            Pmt::Bool(frame.implicit_header),
                        );
                        mio.post("out", Pmt::Blob(dewhitened)).await?;
                        mio.post("out_annotated", Pmt::MapStrPmt(annotated_payload))
                            .await?;
                    }
                    Pmt::Ok
                } else {
                    Pmt::InvalidValue
                }
            }
            Pmt::Finished => {
                io.finished = true;
                Pmt::Ok
            }
            _ => Pmt::InvalidValue,
        };
        Ok(ret)
    }
}

impl Default for Decoder {
    fn default() -> Self {
        Self::new()
    }
}
