use core::result::Result;

use base64::prelude::*;
use ctr::cipher::KeyIvInit;
use ctr::cipher::StreamCipher;
use futuresdr::tracing::info;
use meshtastic::Message;

use crate::utils::Bandwidth;
use crate::utils::Channel;
use crate::utils::CodeRate;
use crate::utils::SpreadingFactor;

type Aes128 = ctr::Ctr64BE<aes::Aes128>;
type Aes256 = ctr::Ctr64BE<aes::Aes256>;

const DEFAULT_KEY: [u8; 16] = [
    0xd4, 0xf1, 0xbb, 0x3a, 0x20, 0x29, 0x07, 0x59, 0xf0, 0xbc, 0xff, 0xab, 0xcf, 0x4e, 0x69, 0x01,
];

#[derive(Debug, Clone, clap::ValueEnum, Copy, Default)]
#[clap(rename_all = "SCREAMING_SNAKE_CASE")]
#[allow(non_camel_case_types)]
pub enum MeshtasticConfig {
    ShortTurbo,
    ShortFast,
    ShortSlow,
    MediumFast,
    MediumSlow,
    #[default]
    LongFast,
    LongTurbo,
    LongModerate, // as service channel
    //LongSlow, deprecated presets
    // VeryLongSlowEu 
}
#[derive(Debug)]
pub enum Error {
    NoSlotsForBandwidth(&'static str),
    InvalidSlot(u8),
}

impl MeshtasticConfig {
    pub const ALL: [MeshtasticConfig; 8] = [
        MeshtasticConfig::ShortTurbo,
        MeshtasticConfig::ShortFast,
        MeshtasticConfig::ShortSlow,
        MeshtasticConfig::MediumFast,
        MeshtasticConfig::MediumSlow,
        MeshtasticConfig::LongFast,
        MeshtasticConfig::LongTurbo,
        MeshtasticConfig::LongModerate,
    ];
    pub fn get_avail_slots_cnt(region: &str, bw: Bandwidth) -> u8 {
        match region {
            "EU433" => match bw {
                Bandwidth::BW62 => 0,
                Bandwidth::BW125 => 8, // slots 0..=7
                Bandwidth::BW250 => 4, // slots 0..=3
                Bandwidth::BW500 => 2, // slots 0..=1
            },
            "EU868" => match bw {
                Bandwidth::BW62 => 0,
                Bandwidth::BW125 => 2, // slots 0..=1
                Bandwidth::BW250 => 1, // slot 0
                Bandwidth::BW500 => 0,
            },
            "RU" => match bw {
                Bandwidth::BW62 => 0,
                Bandwidth::BW125 => 4, // slots 0..=3
                Bandwidth::BW250 => 2, // slots 0..=1
                Bandwidth::BW500 => 1, // slot 0
            },
            _ => 0,
        }
    }


    pub fn get_frequency_by_slot(region: &str, bw: Bandwidth, slot:u8) -> Result<Channel, Error>{
        match region {
            "EU433" => match bw{
                Bandwidth::BW62  => Err(Error::NoSlotsForBandwidth("BW62 has no slots in EU433")),
                Bandwidth::BW125 => match slot {
                    0..=7 => Ok(Channel::Custom(433_062_500 + slot as u32*125_000)),
                    s => Err(Error::InvalidSlot((s)))
                },
                Bandwidth::BW250 => match slot {
                    0..=3 => Ok(Channel::Custom(433_125_000 + slot as u32*250_000)),
                    s => Err(Error::InvalidSlot((s)))
                },
                Bandwidth::BW500 => match slot {
                    0 | 1 => Ok(Channel::Custom(433_250_000 + slot as u32*500_000)),
                    s => Err(Error::InvalidSlot((s)))
                },
            },
            "EU868" => match bw {
                Bandwidth::BW62  => Err(Error::NoSlotsForBandwidth("BW62 has no slots in EU868")),
                Bandwidth::BW125 => match slot {
                    0 | 1 => Ok(Channel::Custom(869_462_500 + slot as u32*125_000)),
                    s => Err(Error::InvalidSlot((s)))
                },
                Bandwidth::BW250 => match slot {
                    0 => Ok(Channel::Custom(869_525_000 + slot as u32*250_000)),
                    s => Err(Error::InvalidSlot((s)))
                },
                Bandwidth::BW500 => Err(Error::NoSlotsForBandwidth("BW500 has no slots in EU868")),
            },
            "RU" => match bw {
                Bandwidth::BW62  => Err(Error::NoSlotsForBandwidth("BW62 has no slots in RU")),
                Bandwidth::BW125 => match slot {
                    0..=3 => Ok(Channel::Custom(868_762_500 + slot as u32*125_000)),
                    s => Err(Error::InvalidSlot((s)))
                }
                Bandwidth::BW250 => match slot {
                    0 | 1 => Ok(Channel::Custom(868_825_000 + slot as u32*250_000)),
                    s => Err(Error::InvalidSlot((s)))
                }
                Bandwidth::BW500 => match slot {
                    0  => Ok(Channel::Custom(868_950_000 + slot as u32*500_000)),
                    s => Err(Error::InvalidSlot((s)))
                }
            },
            _ => Err(Error::NoSlotsForBandwidth("Unknown region, need implement in get_frequency_by_slot fn")),
        }
    }

    pub fn to_config(&self, region: &str, slot:u8) -> (Bandwidth, SpreadingFactor, CodeRate, Channel, bool, u8) {
        match self {
            Self::ShortTurbo => (
                Bandwidth::BW500,
                SpreadingFactor::SF7,
                CodeRate::CR_4_5,
                Self::get_frequency_by_slot(region,Bandwidth::BW500,slot).unwrap(),
                false,
                Self::get_avail_slots_cnt(region, Bandwidth::BW500),
            ),
            Self::ShortFast => (
                Bandwidth::BW250,
                SpreadingFactor::SF7,
                CodeRate::CR_4_5,
                Self::get_frequency_by_slot(region,Bandwidth::BW250,slot).unwrap(),
                false,
                Self::get_avail_slots_cnt(region, Bandwidth::BW250),
            ),
            Self::ShortSlow => (
                Bandwidth::BW250,
                SpreadingFactor::SF8,
                CodeRate::CR_4_5,
                Self::get_frequency_by_slot(region,Bandwidth::BW250,slot).unwrap(),
                false,
                Self::get_avail_slots_cnt(region, Bandwidth::BW250),
            ),
            Self::MediumFast => (
                Bandwidth::BW250,
                SpreadingFactor::SF9,
                CodeRate::CR_4_5,
                Self::get_frequency_by_slot(region,Bandwidth::BW250,slot).unwrap(),
                false,
                Self::get_avail_slots_cnt(region, Bandwidth::BW250),
            ),
            Self::MediumSlow => (
                Bandwidth::BW250,
                SpreadingFactor::SF10,
                CodeRate::CR_4_5,
                Self::get_frequency_by_slot(region,Bandwidth::BW250,slot).unwrap(),
                false,
                Self::get_avail_slots_cnt(region, Bandwidth::BW250),
            ),
            Self::LongFast => (
                Bandwidth::BW250,
                SpreadingFactor::SF11,
                CodeRate::CR_4_5,
                Self::get_frequency_by_slot(region,Bandwidth::BW250,slot).unwrap(),
                false,
                Self::get_avail_slots_cnt(region, Bandwidth::BW250),
            ),
            Self::LongTurbo => (
                Bandwidth::BW500,
                SpreadingFactor::SF11,
                CodeRate::CR_4_5,
                Self::get_frequency_by_slot(region,Bandwidth::BW500,slot).unwrap(),
                false,
                Self::get_avail_slots_cnt(region, Bandwidth::BW500),
            ),
            Self::LongModerate => (
                Bandwidth::BW125,
                SpreadingFactor::SF11,
                CodeRate::CR_4_8,
                Self::get_frequency_by_slot(region,Bandwidth::BW125,slot).unwrap(),
                true,
                Self::get_avail_slots_cnt(region, Bandwidth::BW125),
            ),

        }
    }
}

#[derive(Debug)]
pub struct MeshPacket {
    _dest: u32,
    sender: u32,
    packet_id: u32,
    _flags: u8,
    channel_hash: u8,
    _reserved: u16,
    data: Vec<u8>,
}

impl MeshPacket {
    pub fn new(bytes: &[u8]) -> Self {
        Self {
            _dest: u32::from_le_bytes(bytes[0..4].try_into().unwrap()),
            sender: u32::from_le_bytes(bytes[4..8].try_into().unwrap()),
            packet_id: u32::from_le_bytes(bytes[8..12].try_into().unwrap()),
            _flags: bytes[12],
            channel_hash: bytes[13],
            _reserved: u16::from_le_bytes(bytes[14..16].try_into().unwrap()),
            data: bytes[16..].to_vec(),
        }
    }
}

#[derive(Debug)]
enum Key {
    Aes128([u8; 16]),
    Aes256([u8; 32]),
}

impl Key {
    fn as_slice(&self) -> &[u8] {
        match self {
            Self::Aes128(x) => x,
            Self::Aes256(x) => x,
        }
    }
}

#[derive(Debug)]
pub struct MeshtasticChannel {
    key: Key,
    hash: u8,
    name: String,
}

impl MeshtasticChannel {
    pub fn new(name: &str, key: &str) -> Self {
        let key = BASE64_STANDARD.decode(key).unwrap();
        let key = if key == [0x01] {
            Key::Aes128(DEFAULT_KEY)
        } else if key.len() == 16 {
            Key::Aes128(key.clone().try_into().unwrap())
        } else if key.len() == 32 {
            Key::Aes256(key.clone().try_into().unwrap())
        } else {
            panic!("wrong key (base64-encoded 1/16/32 bytes expected)");
        };

        let (hash, name) = if name.is_empty() || name == "\n" {
            let hash = Self::hash("\n", key.as_slice());
            (hash, "<unset>".to_string())
        } else {
            let hash = Self::hash(name, key.as_slice());
            (hash, name.to_string())
        };

        Self { key, hash, name }
    }

    fn hash(name: &str, key: &[u8]) -> u8 {
        let mut xor = 0;
        for x in name.bytes() {
            xor ^= x;
        }
        for x in key.iter() {
            xor ^= x;
        }
        xor
    }

    pub fn decode(&self, packet: &MeshPacket) -> bool {
        info!("MeshPacket: {:?}", packet);
        let mut iv = vec![];
        iv.extend_from_slice(&(packet.packet_id as u64).to_le_bytes());
        iv.extend_from_slice(&(packet.sender as u64).to_le_bytes());
        let iv: [u8; 16] = iv.try_into().unwrap();

        let mut bytes = packet.data.clone();
        match self.key {
            Key::Aes128(key) => {
                let mut cipher = Aes128::new(&key.into(), &iv.into());
                cipher.apply_keystream(&mut bytes);
            }
            Key::Aes256(key) => {
                let mut cipher = Aes256::new(&key.into(), &iv.into());
                cipher.apply_keystream(&mut bytes);
            }
        }
        if let Ok(res) = meshtastic::protobufs::Data::decode(&*bytes) {
            if res.portnum == meshtastic::protobufs::PortNum::TextMessageApp as i32 {
                info!(
                    "Channel {}: Message {:?}",
                    self.name,
                    String::from_utf8_lossy(&res.payload)
                );
                true
            } else {
                info!("Channel {}: Message {:?}", self.name, res);
                true
            }
        } else {
            false
        }
    }

    pub fn encode(&self, data: String) -> Vec<u8> {
        let packet_id = 0u32;
        let dest = 0xffffffffu32;
        let sender = 0x3a48290eu32;

        let data = meshtastic::protobufs::Data {
            portnum: 1,
            payload: data.into_bytes(),
            want_response: false,
            dest: 0,
            source: 0,
            request_id: 0,
            reply_id: 0,
            emoji: 0,
            bitfield: None,
        };

        let mut bytes = data.encode_to_vec();

        let mut iv = vec![];
        iv.extend_from_slice(&(packet_id as u64).to_le_bytes());
        iv.extend_from_slice(&(sender as u64).to_le_bytes());
        let iv: [u8; 16] = iv.try_into().unwrap();

        match self.key {
            Key::Aes128(key) => {
                let mut cipher = Aes128::new(&key.into(), &iv.into());
                cipher.apply_keystream(&mut bytes);
            }
            Key::Aes256(key) => {
                let mut cipher = Aes256::new(&key.into(), &iv.into());
                cipher.apply_keystream(&mut bytes);
            }
        }

        let mut out = vec![];
        out.extend_from_slice(&dest.to_le_bytes());
        out.extend_from_slice(&sender.to_le_bytes());
        out.extend_from_slice(&packet_id.to_le_bytes());
        out.push(0);
        out.push(self.hash);
        out.extend_from_slice(&[0; 2]);
        out.extend_from_slice(&bytes);
        out
    }
}

pub struct MeshtasticChannels {
    channels: Vec<MeshtasticChannel>,
}

impl MeshtasticChannels {
    pub fn new() -> Self {
        Self {
            channels: vec![MeshtasticChannel::new("", "AQ==")],
        }
    }

    pub fn add_channel(&mut self, chan: MeshtasticChannel) {
        self.channels.push(chan);
    }

    pub fn decode(&self, bytes: &[u8]) {
        let packet = MeshPacket::new(bytes);

        for chan in self.channels.iter() {
            if packet.channel_hash == chan.hash && chan.decode(&packet) {
                return;
            }
        }
        self.channels[0].decode(&packet);
    }
}

impl Default for MeshtasticChannels {
    fn default() -> Self {
        Self::new()
    }
}
