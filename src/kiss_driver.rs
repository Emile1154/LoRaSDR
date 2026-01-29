
// --> RX commands
// CMD DETECT
// CMD DATA
// CMD SNR
// CMD RSSI
// CMD READY
//---------------
// TX commands
// CMD DATA
// CMD READY

pub const RADIOLIB_SX126X_IRQ_HEADER_ERR: u8   = 0b00100000; // 5: LoRa header CRC error
pub const RADIOLIB_SX126X_IRQ_HEADER_VALID: u8 = 0b00010000; // 4: valid LoRa header received
pub const RADIOLIB_SX126X_IRQ_CRC_ERR: u8      = 0b01000000; // 6: wrong CRC received

pub mod kiss {
    pub const FEND       : u8 = 0xC0; 
    pub const FESC       : u8 = 0xDB; 
    pub const TFEND      : u8 = 0xDC; 
    pub const TFESC      : u8 = 0xDD; 

    pub const CMD_DETECT : u8 = 0x08;
    pub const CMD_DATA   : u8 = 0x00;
    pub const CMD_SNR    : u8 = 0x24;
    pub const CMD_RSSI   : u8 = 0x23;
    pub const CMD_READY  : u8 = 0x0F;
}

pub fn escape(data: &[u8]) -> Vec<u8> {
    let mut result = Vec::with_capacity(data.len() * 2); // worst case

    for &b in data.iter() {
        if b == kiss::FEND {
            result.push(kiss::FESC);
            result.push(kiss::TFEND);
        } else if b == kiss::FESC {
            result.push(kiss::FESC);
            result.push(kiss::TFESC);
        } else {
            result.push(b);
        }
    }
    result
}

pub fn descape(data: &[u8]) -> Vec<u8> {
    let mut result = Vec::with_capacity(data.len());

    let mut i = 0;
    while i < data.len() {
        let b = data[i];
        if b == kiss::FESC {
            i += 1;
            if i >= data.len() {
                break; // Prevent out-of-bounds
            }

            let next_b = data[i];
            if next_b == kiss::TFEND {
                result.push(kiss::FEND);
            } else if next_b == kiss::TFESC {
                result.push(kiss::FESC);
            } else {
                result.push(next_b);
            }
        } else {
            result.push(b);
        }
        i += 1;
    }
    result
}

pub fn create_cmd(cmd: u8, data: &[u8]) -> Vec<u8> {
    let mut result = Vec::with_capacity(data.len() + 2);
    result.push(kiss::FEND);
    result.push(cmd);
    result.extend_from_slice(data);
    result.push(kiss::FEND);
    result
}