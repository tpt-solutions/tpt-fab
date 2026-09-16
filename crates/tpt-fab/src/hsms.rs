//! HSMS message transport (SEMI E37): framing, control messages, timers, and session state.
//!
//! HSMS frames are `[4-byte big-endian length][10-byte header][body]`. The 10-byte header
//! layout is shared with SECS-II data messages ([`crate::secs::SecsHeader`]): session ID,
//! `W|stream`, function, PType (0 = SECS-II), SType, system bytes.
//!
//! Session types (SType): 0 = data, 1 = select request, 2 = select response,
//! 4 = separate request, 5 = linktest request, 6 = linktest response, 7 = reject.
//!
//! State machine per E37: `NOT_CONNECTED → CONNECTED(NOT_SELECTED) → SELECTED`.
//! Timer semantics implemented here: T3 (reply), T5 (connect separation), T6 (control
//! transaction), T7 (NOT_SELECTED), T8 (inter-byte).

use crate::error::FabError;
use crate::secs::SecsHeader;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

/// HSMS session state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HsmsState {
    /// No TCP connection.
    NotConnected,
    /// TCP connected but not yet selected.
    ConnectedNotSelected,
    /// Selected; data messages flow.
    Selected,
}

/// HSMS role of the local entity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HsmsRole {
    /// Initiates TCP connections and the select transaction.
    Active,
    /// Listens and accepts.
    Passive,
}

/// HSMS timer configuration, seconds (defaults follow common E37 practice).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HsmsTimers {
    /// Reply timeout for W-bit data messages.
    pub t3: Duration,
    /// Connect-separation delay between successive active connection attempts.
    pub t5: Duration,
    /// Control-transaction timeout (select/linktest round trip).
    pub t6: Duration,
    /// Time a passive side allows between TCP connect and select.
    pub t7: Duration,
    /// Inter-byte timeout within a single frame.
    pub t8: Duration,
    /// Linktest interval (0 = disabled).
    pub linktest: Duration,
}

impl Default for HsmsTimers {
    fn default() -> Self {
        Self {
            t3: Duration::from_secs(45),
            t5: Duration::from_secs(10),
            t6: Duration::from_secs(5),
            t7: Duration::from_secs(10),
            t8: Duration::from_secs(6),
            linktest: Duration::from_secs(30),
        }
    }
}

/// One HSMS frame: header plus optional body.
#[derive(Debug, Clone, PartialEq)]
pub struct HsmsFrame {
    /// 10-byte HSMS header fields.
    pub header: SecsHeader,
    /// Message body (SECS-II item bytes for data messages, empty for control messages).
    pub body: Vec<u8>,
}

/// Select response status codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectStatus {
    /// Selection accepted.
    Accepted,
    /// Responder not ready (e.g. already selected).
    NotReady,
    /// Unknown status code.
    Unknown(u16),
}

impl SelectStatus {
    fn from_raw(raw: u16) -> Self {
        match raw {
            0 => SelectStatus::Accepted,
            1 => SelectStatus::NotReady,
            other => SelectStatus::Unknown(other),
        }
    }
}

impl HsmsFrame {
    /// Builds a data frame carrying a SECS-II message.
    pub fn data(
        device_id: u16,
        stream: u8,
        function: u8,
        w_bit: bool,
        system_bytes: u32,
        body: Vec<u8>,
    ) -> Self {
        HsmsFrame {
            header: SecsHeader { device_id, stream, function, w_bit, s_type: 0, system_bytes },
            body,
        }
    }

    /// Select request.
    pub fn select_req(system_bytes: u32) -> Self {
        HsmsFrame {
            header: SecsHeader {
                device_id: 0xFFFF,
                stream: 0,
                function: 0,
                w_bit: false,
                s_type: 1,
                system_bytes,
            },
            body: Vec::new(),
        }
    }

    /// Select response carrying the select status in bytes 6-7.
    pub fn select_rsp(system_bytes: u32, status: SelectStatus) -> Self {
        let raw = match status {
            SelectStatus::Accepted => 0u16,
            SelectStatus::NotReady => 1,
            SelectStatus::Unknown(v) => v,
        };
        let mut body = Vec::new();
        body.extend_from_slice(&raw.to_be_bytes());
        HsmsFrame {
            header: SecsHeader {
                device_id: 0xFFFF,
                stream: 0,
                function: 0,
                w_bit: false,
                s_type: 2,
                system_bytes,
            },
            body,
        }
    }

    /// Separate request (orderly goodbye).
    pub fn separate_req(system_bytes: u32) -> Self {
        HsmsFrame {
            header: SecsHeader {
                device_id: 0xFFFF,
                stream: 0,
                function: 0,
                w_bit: false,
                s_type: 4,
                system_bytes,
            },
            body: Vec::new(),
        }
    }

    /// Linktest request.
    pub fn linktest_req(system_bytes: u32) -> Self {
        HsmsFrame {
            header: SecsHeader {
                device_id: 0xFFFF,
                stream: 0,
                function: 0,
                w_bit: false,
                s_type: 5,
                system_bytes,
            },
            body: Vec::new(),
        }
    }

    /// Linktest response.
    pub fn linktest_rsp(system_bytes: u32) -> Self {
        HsmsFrame {
            header: SecsHeader {
                device_id: 0xFFFF,
                stream: 0,
                function: 0,
                w_bit: false,
                s_type: 6,
                system_bytes,
            },
            body: Vec::new(),
        }
    }

    /// Reject message for an unsupported/illegal PType or SType.
    pub fn reject(system_bytes: u32, rejected_s_type: u8) -> Self {
        // Body: rejected PType (SECS-II) then the rejected SType.
        let body = vec![0u8, rejected_s_type];
        HsmsFrame {
            header: SecsHeader {
                device_id: 0xFFFF,
                stream: 0,
                function: 0,
                w_bit: false,
                s_type: 7,
                system_bytes,
            },
            body,
        }
    }

    /// Whether this frame is a data (SECS-II) frame.
    pub fn is_data(&self) -> bool {
        self.header.s_type == 0
    }

    /// Decodes the select status carried in a select response (SType 2) body.
    pub fn select_status(&self) -> Option<SelectStatus> {
        if self.header.s_type != 2 || self.body.len() < 2 {
            return None;
        }
        Some(SelectStatus::from_raw(u16::from_be_bytes([self.body[0], self.body[1]])))
    }

    /// Encodes the frame: 4-byte length prefix + header + body.
    pub fn encode(&self) -> Vec<u8> {
        let len = (10 + self.body.len()) as u32;
        let mut out = Vec::with_capacity(4 + 10 + self.body.len());
        out.extend_from_slice(&len.to_be_bytes());
        out.extend_from_slice(&self.header.encode());
        out.extend_from_slice(&self.body);
        out
    }

    /// Decodes one frame from a byte buffer holding exactly one frame (incl. length prefix).
    pub fn decode(data: &[u8]) -> Result<HsmsFrame, FabError> {
        if data.len() < 4 {
            return Err(FabError::Hsms("frame shorter than length prefix".into()));
        }
        let len = u32::from_be_bytes([data[0], data[1], data[2], data[3]]) as usize;
        if len < 10 {
            return Err(FabError::Hsms(format!("declared length {len} shorter than header")));
        }
        if data.len() < 4 + len {
            return Err(FabError::Hsms("frame body truncated".into()));
        }
        let header = SecsHeader::decode(&data[4..14])?;
        Ok(HsmsFrame { header, body: data[14..4 + len].to_vec() })
    }
}

/// A length-delimited HSMS connection over any stream (TCP in practice).
pub struct HsmsChannel {
    stream: TcpStream,
    t8: Duration,
}

impl HsmsChannel {
    /// Wraps an established TCP stream.
    pub fn new(stream: TcpStream, t8: Duration) -> Self {
        HsmsChannel { stream, t8 }
    }

    /// Sends one frame.
    pub fn send(&mut self, frame: &HsmsFrame) -> Result<(), FabError> {
        let bytes = frame.encode();
        self.stream.write_all(&bytes)?;
        self.stream.flush()?;
        Ok(())
    }

    /// Receives one frame, waiting up to `idle_timeout` for the first byte and up to T8
    /// between subsequent bytes of the same frame.
    pub fn recv(&mut self, idle_timeout: Duration) -> Result<HsmsFrame, FabError> {
        self.stream.set_read_timeout(Some(idle_timeout))?;
        let mut len_buf = [0u8; 4];
        read_exact(self.stream.try_clone()?, &mut len_buf)?;
        // We are mid-frame now: apply T8.
        self.stream.set_read_timeout(Some(self.t8))?;
        let len = u32::from_be_bytes(len_buf) as usize;
        if !(10..=16 * 1024 * 1024).contains(&len) {
            return Err(FabError::Hsms(format!("unreasonable frame length {len}")));
        }
        let mut rest = vec![0u8; len];
        read_exact(self.stream.try_clone()?, &mut rest)?;
        let mut full = Vec::with_capacity(4 + len);
        full.extend_from_slice(&len_buf);
        full.extend_from_slice(&rest);
        HsmsFrame::decode(&full)
    }
}

fn read_exact(mut stream: TcpStream, buf: &mut [u8]) -> Result<(), FabError> {
    let mut filled = 0usize;
    while filled < buf.len() {
        match stream.read(&mut buf[filled..]) {
            Ok(0) => return Err(FabError::Disconnected),
            Ok(n) => filled += n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut =>
            {
                return Err(FabError::Timeout("read timed out".into()))
            }
            Err(e) => return Err(FabError::Io(e)),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_roundtrip() {
        let f = HsmsFrame::data(7, 1, 13, true, 99, vec![0x01, 0x41, 0x01, b'x']);
        let enc = f.encode();
        assert_eq!(HsmsFrame::decode(&enc).unwrap(), f);
    }

    #[test]
    fn control_frames_roundtrip() {
        let select = HsmsFrame::select_req(5);
        assert_eq!(select.header.s_type, 1);
        let enc = select.encode();
        assert_eq!(HsmsFrame::decode(&enc).unwrap(), select);

        let rsp = HsmsFrame::select_rsp(5, SelectStatus::Accepted);
        let dec = HsmsFrame::decode(&rsp.encode()).unwrap();
        assert_eq!(dec.header.s_type, 2);
        assert_eq!(dec.body, vec![0, 0]);

        let sep = HsmsFrame::separate_req(6);
        assert_eq!(HsmsFrame::decode(&sep.encode()).unwrap(), sep);
    }
}
