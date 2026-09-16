//! SECS-II message encoding/decoding (SEMI E5).
//!
//! Item format bytes follow the published table:
//! `L=0x00 B=0x20 T=0x24 A=0x40 I8=0x60 I1=0x64 I2=0x68 I4=0x6C F8=0x80 F4=0x84
//!  U8=0xA0 U1=0xA4 U2=0xA8 U4=0xAC`. Each item header is the format byte followed by
//! 1–3 big-endian length bytes (element count), then the payload.

use crate::error::FabError;
use std::fmt;

/// A SECS-II data item.
#[derive(Debug, Clone, PartialEq)]
pub enum Item {
    /// List of items.
    L(Vec<Item>),
    /// Binary data.
    B(Vec<u8>),
    /// Boolean.
    Boolean(Vec<bool>),
    /// ASCII string.
    A(String),
    /// 8-byte signed integers.
    I8(Vec<i64>),
    /// 1-byte signed integers.
    I1(Vec<i8>),
    /// 2-byte signed integers.
    I2(Vec<i16>),
    /// 4-byte signed integers.
    I4(Vec<i32>),
    /// 8-byte floats.
    F8(Vec<f64>),
    /// 4-byte floats.
    F4(Vec<f32>),
    /// 8-byte unsigned integers.
    U8(Vec<u64>),
    /// 1-byte unsigned integers.
    U1(Vec<u8>),
    /// 2-byte unsigned integers.
    U2(Vec<u16>),
    /// 4-byte unsigned integers.
    U4(Vec<u32>),
}

impl Item {
    /// Item format byte per SEMI E5.
    pub fn format_byte(&self) -> u8 {
        match self {
            Item::L(_) => 0x00,
            Item::B(_) => 0x20,
            Item::Boolean(_) => 0x24,
            Item::A(_) => 0x40,
            Item::I8(_) => 0x60,
            Item::I1(_) => 0x64,
            Item::I2(_) => 0x68,
            Item::I4(_) => 0x6C,
            Item::F8(_) => 0x80,
            Item::F4(_) => 0x84,
            Item::U8(_) => 0xA0,
            Item::U1(_) => 0xA4,
            Item::U2(_) => 0xA8,
            Item::U4(_) => 0xAC,
        }
    }

    /// Element size in bytes for fixed-size items (1 for A/B/Boolean/U1/I1).
    pub fn element_size(&self) -> usize {
        match self {
            Item::L(_) | Item::A(_) | Item::B(_) | Item::Boolean(_) => 1,
            Item::I8(_) | Item::U8(_) | Item::F8(_) => 8,
            Item::I1(_) | Item::U1(_) => 1,
            Item::F4(_) | Item::I2(_) | Item::U2(_) => 2,
            Item::I4(_) | Item::U4(_) => 4,
        }
    }

    /// Convenience: first byte of a `B` or `U1` item — the common shape of SECS ACK fields.
    pub fn ack_code(&self) -> Option<u8> {
        match self {
            Item::B(v) | Item::U1(v) => v.first().copied(),
            _ => None,
        }
    }

    /// Element count.
    pub fn element_count(&self) -> usize {
        match self {
            Item::L(v) => v.len(),
            Item::B(v) => v.len(),
            Item::Boolean(v) => v.len(),
            Item::A(s) => s.len(),
            Item::I8(v) => v.len(),
            Item::I1(v) => v.len(),
            Item::I2(v) => v.len(),
            Item::I4(v) => v.len(),
            Item::F8(v) => v.len(),
            Item::F4(v) => v.len(),
            Item::U8(v) => v.len(),
            Item::U1(v) => v.len(),
            Item::U2(v) => v.len(),
            Item::U4(v) => v.len(),
        }
    }

    /// Convenience: the ASCII string if this is an `A` item.
    pub fn as_ascii(&self) -> Option<&str> {
        match self {
            Item::A(s) => Some(s),
            _ => None,
        }
    }

    /// Convenience: the first U4 value if this is a non-empty `U4` item.
    pub fn as_u4(&self) -> Option<u32> {
        match self {
            Item::U4(v) => v.first().copied(),
            _ => None,
        }
    }

    /// Convenience: the first U1 value if this is a non-empty `U1` item.
    pub fn as_u1(&self) -> Option<u8> {
        match self {
            Item::U1(v) => v.first().copied(),
            _ => None,
        }
    }

    /// Convenience: the first U2 value if this is a non-empty `U2` item.
    pub fn as_u2(&self) -> Option<u16> {
        match self {
            Item::U2(v) => v.first().copied(),
            _ => None,
        }
    }

    /// Convenience: list elements.
    pub fn as_list(&self) -> Option<&[Item]> {
        match self {
            Item::L(v) => Some(v),
            _ => None,
        }
    }

    fn write_data(&self, out: &mut Vec<u8>) {
        match self {
            Item::L(items) => items.iter().for_each(|i| i.encode_into(out)),
            Item::B(v) | Item::U1(v) => out.extend_from_slice(v),
            Item::Boolean(v) => v.iter().for_each(|b| out.push(u8::from(*b))),
            Item::A(s) => out.extend_from_slice(s.as_bytes()),
            Item::I8(v) => v.iter().for_each(|x| out.extend_from_slice(&x.to_be_bytes())),
            Item::I1(v) => v.iter().for_each(|x| out.push(*x as u8)),
            Item::I2(v) => v.iter().for_each(|x| out.extend_from_slice(&x.to_be_bytes())),
            Item::I4(v) => v.iter().for_each(|x| out.extend_from_slice(&x.to_be_bytes())),
            Item::F8(v) => v.iter().for_each(|x| out.extend_from_slice(&x.to_be_bytes())),
            Item::F4(v) => v.iter().for_each(|x| out.extend_from_slice(&x.to_be_bytes())),
            Item::U8(v) => v.iter().for_each(|x| out.extend_from_slice(&x.to_be_bytes())),
            Item::U2(v) => v.iter().for_each(|x| out.extend_from_slice(&x.to_be_bytes())),
            Item::U4(v) => v.iter().for_each(|x| out.extend_from_slice(&x.to_be_bytes())),
        }
    }

    /// Encodes the item (header + payload) into `out`.
    pub fn encode_into(&self, out: &mut Vec<u8>) {
        out.push(self.format_byte());
        let count = self.element_count();
        if count <= 0x7F {
            out.push(count as u8);
        } else if count <= 0xFF {
            out.push(0x81);
            out.push(count as u8);
        } else {
            out.push(0x82);
            out.extend_from_slice(&(count as u16).to_be_bytes());
        }
        self.write_data(out);
    }

    /// Encodes the item to a fresh byte vector.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        self.encode_into(&mut out);
        out
    }

    /// Decodes one item from `data`.
    pub fn decode(data: &[u8]) -> Result<Item, FabError> {
        let mut pos = 0usize;
        let item = decode_item(data, &mut pos)?;
        Ok(item)
    }
}

impl fmt::Display for Item {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Item::L(items) => {
                write!(f, "<L [{}]", items.len())?;
                for i in items {
                    write!(f, " {i}")?;
                }
                write!(f, ">")
            }
            Item::B(v) => write!(f, "<B {:02X?}>", v),
            Item::Boolean(v) => write!(f, "<BOOLEAN {v:?}>"),
            Item::A(s) => write!(f, "<A \"{s}\">"),
            Item::I8(v) => write!(f, "<I8 {v:?}>"),
            Item::I1(v) => write!(f, "<I1 {v:?}>"),
            Item::I2(v) => write!(f, "<I2 {v:?}>"),
            Item::I4(v) => write!(f, "<I4 {v:?}>"),
            Item::F8(v) => write!(f, "<F8 {v:?}>"),
            Item::F4(v) => write!(f, "<F4 {v:?}>"),
            Item::U8(v) => write!(f, "<U8 {v:?}>"),
            Item::U1(v) => write!(f, "<U1 {v:?}>"),
            Item::U2(v) => write!(f, "<U2 {v:?}>"),
            Item::U4(v) => write!(f, "<U4 {v:?}>"),
        }
    }
}

fn read_length(data: &[u8], pos: &mut usize) -> Result<usize, FabError> {
    if *pos >= data.len() {
        return Err(FabError::SecsDecode("truncated item header".into()));
    }
    let first = data[*pos];
    *pos += 1;
    if first & 0x80 == 0 {
        return Ok(first as usize);
    }
    let extra = (first & 0x7F) as usize;
    if extra == 0 || extra > 3 {
        return Err(FabError::SecsDecode(format!("bad length-of-length {extra}")));
    }
    if *pos + extra > data.len() {
        return Err(FabError::SecsDecode("truncated length bytes".into()));
    }
    let mut len = 0usize;
    for _ in 0..extra {
        len = (len << 8) | data[*pos] as usize;
        *pos += 1;
    }
    Ok(len)
}

fn decode_item(data: &[u8], pos: &mut usize) -> Result<Item, FabError> {
    if *pos >= data.len() {
        return Err(FabError::SecsDecode("no item data".into()));
    }
    let fmt = data[*pos];
    *pos += 1;
    let count = read_length(data, pos)?;
    let take = |n: usize, pos: &mut usize| -> Result<&[u8], FabError> {
        if *pos + n > data.len() {
            return Err(FabError::SecsDecode("item payload truncated".into()));
        }
        let s = &data[*pos..*pos + n];
        *pos += n;
        Ok(s)
    };
    let item = match fmt {
        0x00 => {
            let mut items = Vec::with_capacity(count);
            for _ in 0..count {
                items.push(decode_item(data, pos)?);
            }
            Item::L(items)
        }
        0x20 => Item::B(take(count, pos)?.to_vec()),
        0x24 => Item::Boolean(take(count, pos)?.iter().map(|b| *b != 0).collect()),
        0x40 => {
            let raw = take(count, pos)?;
            Item::A(String::from_utf8_lossy(raw).into_owned())
        }
        0x60 | 0x64 | 0x68 | 0x6C | 0x80 | 0x84 | 0xA0 | 0xA4 | 0xA8 | 0xAC => {
            let esz = match fmt {
                0x60 | 0xA0 | 0x80 => 8,
                0x64 | 0xA4 => 1,
                0x68 | 0xA8 => 2,
                0x6C | 0xAC | 0x84 => 4,
                _ => unreachable!(),
            };
            let raw = take(count * esz, pos)?;
            match fmt {
                0x60 => Item::I8(
                    raw.chunks(8).map(|c| i64::from_be_bytes(c.try_into().unwrap())).collect(),
                ),
                0x64 => Item::I1(raw.iter().map(|b| *b as i8).collect()),
                0x68 => Item::I2(
                    raw.chunks(2).map(|c| i16::from_be_bytes(c.try_into().unwrap())).collect(),
                ),
                0x6C => Item::I4(
                    raw.chunks(4).map(|c| i32::from_be_bytes(c.try_into().unwrap())).collect(),
                ),
                0x80 => Item::F8(
                    raw.chunks(8).map(|c| f64::from_be_bytes(c.try_into().unwrap())).collect(),
                ),
                0x84 => Item::F4(
                    raw.chunks(4).map(|c| f32::from_be_bytes(c.try_into().unwrap())).collect(),
                ),
                0xA0 => Item::U8(
                    raw.chunks(8).map(|c| u64::from_be_bytes(c.try_into().unwrap())).collect(),
                ),
                0xA4 => Item::U1(raw.to_vec()),
                0xA8 => Item::U2(
                    raw.chunks(2).map(|c| u16::from_be_bytes(c.try_into().unwrap())).collect(),
                ),
                0xAC => Item::U4(
                    raw.chunks(4).map(|c| u32::from_be_bytes(c.try_into().unwrap())).collect(),
                ),
                _ => unreachable!(),
            }
        }
        _ => return Err(FabError::SecsDecode(format!("unknown format byte 0x{fmt:02X}"))),
    };
    Ok(item)
}

/// HSMS session-ID independent SECS-II message header (SEMI E37 header layout).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SecsHeader {
    /// Session/device ID (header bytes 0-1).
    pub device_id: u16,
    /// Stream number (0 for control messages).
    pub stream: u8,
    /// Function number (0 for control messages).
    pub function: u8,
    /// W-bit (reply expected).
    pub w_bit: bool,
    /// HSMS session type: 0 = data, 1 = select req, 2 = select rsp, 4 = separate,
    /// 5 = linktest req, 6 = linktest rsp, 7 = reject.
    pub s_type: u8,
    /// Transaction ID (system bytes).
    pub system_bytes: u32,
}

impl SecsHeader {
    /// Encodes the 10-byte header.
    pub fn encode(&self) -> [u8; 10] {
        let mut h = [0u8; 10];
        h[0..2].copy_from_slice(&self.device_id.to_be_bytes());
        h[2] = if self.w_bit { 0x80 } else { 0x00 } | (self.stream & 0x7F);
        h[3] = self.function;
        h[4] = 0; // PType: SECS-II
        h[5] = self.s_type;
        h[6..10].copy_from_slice(&self.system_bytes.to_be_bytes());
        h
    }

    /// Decodes a 10-byte header.
    pub fn decode(data: &[u8]) -> Result<SecsHeader, FabError> {
        if data.len() < 10 {
            return Err(FabError::SecsDecode(format!("header too short: {} bytes", data.len())));
        }
        Ok(SecsHeader {
            device_id: u16::from_be_bytes([data[0], data[1]]),
            stream: data[2] & 0x7F,
            function: data[3],
            w_bit: data[2] & 0x80 != 0,
            s_type: data[5],
            system_bytes: u32::from_be_bytes([data[6], data[7], data[8], data[9]]),
        })
    }
}

impl fmt::Display for SecsHeader {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.s_type == 0 {
            write!(f, "S{}F{}{}", self.stream, self.function, if self.w_bit { "W" } else { "" })
        } else {
            write!(f, "SType{}", self.s_type)
        }
    }
}

/// A complete SECS-II message: header plus optional item body.
#[derive(Debug, Clone, PartialEq)]
pub struct SecsMessage {
    /// Message header.
    pub header: SecsHeader,
    /// Optional item body (control messages carry none).
    pub item: Option<Item>,
}

impl SecsMessage {
    /// Creates a data message (S-type 0).
    pub fn data(
        device_id: u16,
        stream: u8,
        function: u8,
        w_bit: bool,
        system_bytes: u32,
        item: Option<Item>,
    ) -> Self {
        SecsMessage {
            header: SecsHeader { device_id, stream, function, w_bit, s_type: 0, system_bytes },
            item,
        }
    }

    /// Name like `S1F13W` for logs.
    pub fn name(&self) -> String {
        self.header.to_string()
    }

    /// Encodes header + body.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&self.header.encode());
        if let Some(item) = &self.item {
            item.encode_into(&mut out);
        }
        out
    }

    /// Decodes a full message (header + body) from `data`.
    pub fn decode(data: &[u8]) -> Result<SecsMessage, FabError> {
        let header = SecsHeader::decode(data)?;
        let item = if data.len() > 10 {
            let mut pos = 10usize;
            Some(decode_item(data, &mut pos)?)
        } else {
            None
        };
        Ok(SecsMessage { header, item })
    }

    /// Assembles a message from a decoded header and raw body bytes (HSMS frame payload).
    pub fn from_parts(header: SecsHeader, body: &[u8]) -> Result<SecsMessage, FabError> {
        let item = if body.is_empty() {
            None
        } else {
            let mut pos = 0usize;
            Some(decode_item(body, &mut pos)?)
        };
        Ok(SecsMessage { header, item })
    }
}

/// CRC-32 (IEEE 802.3), used for recipe body checksums.
pub fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for b in data {
        crc ^= *b as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn item_roundtrip_all_formats() {
        let items = vec![
            Item::L(vec![Item::U1(vec![1]), Item::A("hello".into())]),
            Item::B(vec![0xDE, 0xAD]),
            Item::Boolean(vec![true, false]),
            Item::A("sky130".into()),
            Item::I8(vec![-1, i64::MIN]),
            Item::I1(vec![-3]),
            Item::I2(vec![-400]),
            Item::I4(vec![-100_000]),
            Item::F8(vec![3.25]),
            Item::F4(vec![1.5]),
            Item::U8(vec![u64::MAX]),
            Item::U1(vec![7]),
            Item::U2(vec![1_000]),
            Item::U4(vec![100_000]),
        ];
        for item in items {
            let enc = item.encode();
            let dec = Item::decode(&enc).unwrap();
            assert_eq!(dec, item, "roundtrip failed for {item}");
        }
    }

    #[test]
    fn long_list_uses_multibyte_length() {
        let items: Vec<Item> = (0..200).map(|i| Item::U1(vec![i as u8])).collect();
        let item = Item::L(items);
        let enc = item.encode();
        assert_eq!(enc[1], 0x81); // one length byte with continuation flag (200 > 0x7F)
        assert_eq!(Item::decode(&enc).unwrap(), item);
    }

    #[test]
    fn message_roundtrip() {
        let msg = SecsMessage::data(
            0,
            6,
            11,
            true,
            42,
            Some(Item::L(vec![Item::U4(vec![7]), Item::A("lot-1".into())])),
        );
        let enc = msg.encode();
        let dec = SecsMessage::decode(&enc).unwrap();
        assert_eq!(dec, msg);
        assert_eq!(dec.name(), "S6F11W");
    }

    #[test]
    fn header_encode_decode() {
        let h = SecsHeader {
            device_id: 10,
            stream: 1,
            function: 13,
            w_bit: true,
            s_type: 0,
            system_bytes: 7,
        };
        let enc = h.encode();
        assert_eq!(SecsHeader::decode(&enc).unwrap(), h);
    }

    #[test]
    fn crc32_known_vector() {
        // CRC-32("123456789") = 0xCBF43926 (IEEE 802.3 check value).
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }
}
