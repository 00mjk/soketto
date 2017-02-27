//! Codec for dedoding/encoding websocket base frames.
use byteorder::{BigEndian, ReadBytesExt, WriteBytesExt};
use frame::base::{Frame, OpCode};
use std::io::{self, Cursor};
use tokio_core::io::{Codec, EasyBuf};
use util::{self, utf8};

/// If the payload length byte is 126, the following two bytes represent the actual payload
/// length.
const TWO_EXT: u8 = 126;
/// If the payload length byte is 127, the following eight bytes represent the actual payload
/// length.
const EIGHT_EXT: u8 = 127;

#[derive(Debug, Clone)]
/// Indicates the state of the decoding process for this frame.
pub enum DecodeState {
    /// None of the frame has been decoded.
    NONE,
    /// The header has been decoded.
    HEADER,
    /// The length has been decoded.
    LENGTH,
    /// The mask has been decoded.
    MASK,
    /// The decoding is complete.
    FULL,
}

impl Default for DecodeState {
    fn default() -> DecodeState {
        DecodeState::NONE
    }
}

#[derive(Clone, Debug, Default)]
/// Codec for dedoding/encoding websocket base frames.
pub struct FrameCodec {
    /// Is this a client frame?
    client: bool,
    /// The `fin` flag.
    fin: bool,
    /// The `rsv1` flag.
    rsv1: bool,
    /// The `rsv2` flag.
    rsv2: bool,
    /// The `rsv3` flag.
    rsv3: bool,
    /// The `opcode`
    opcode: OpCode,
    /// The `masked` flag
    masked: bool,
    /// The length code.
    length_code: u8,
    /// The `payload_length`
    payload_length: u64,
    /// The optional `mask`
    mask_key: u32,
    /// The optional `extension_data`
    extension_data: Option<Vec<u8>>,
    /// The optional `application_data`
    application_data: Option<Vec<u8>>,
    /// Decode state
    state: DecodeState,
    /// Minimum length required to parse the next part of the frame.
    min_len: u64,
    /// Bits reserved by extensions.
    reserved_bits: u8,
}

impl FrameCodec {
    /// Set the `client` flag.
    pub fn set_client(&mut self, client: bool) -> &mut FrameCodec {
        self.client = client;
        self
    }

    /// Set the bits reserved by extensions (0-8 are valid values).
    pub fn set_reserved_bits(&mut self, reserved_bits: u8) -> &mut FrameCodec {
        self.reserved_bits = reserved_bits;
        self
    }

    /// Apply the unmasking to the application data.
    fn apply_mask(&mut self, buf: &mut [u8], mask: u32) -> Result<(), io::Error> {
        let mut mask_buf = Vec::with_capacity(4);
        mask_buf.write_u32::<BigEndian>(mask)?;
        let iter = buf.iter_mut().zip(mask_buf.iter().cycle());
        for (byte, &key) in iter {
            *byte ^= key
        }
        Ok(())
    }
}

impl Codec for FrameCodec {
    type In = Frame;
    type Out = Frame;

    fn decode(&mut self, buf: &mut EasyBuf) -> Result<Option<Self::In>, io::Error> {
        let buf_len = buf.len();
        if buf_len == 0 {
            return Ok(None);
        }

        self.min_len = 0;
        loop {
            match self.state {
                DecodeState::NONE => {
                    // println!("buf at state NONE\n{}", util::as_hex(buf.as_slice()));
                    self.min_len += 2;
                    // Split of the 2 'header' bytes.
                    #[cfg_attr(feature = "cargo-clippy", allow(cast_possible_truncation))]
                    let size = self.min_len as usize;
                    if buf_len < size {
                        return Ok(None);
                    }
                    let header_bytes = buf.drain_to(2);
                    let header = header_bytes.as_slice();
                    let first = header[0];
                    let second = header[1];

                    // Extract the details
                    self.fin = first & 0x80 != 0;
                    self.rsv1 = first & 0x40 != 0;
                    if self.rsv1 && (self.reserved_bits & 0x4 == 0) {
                        return Err(util::other("invalid rsv1 bit set"));
                    }

                    self.rsv2 = first & 0x20 != 0;
                    if self.rsv2 && (self.reserved_bits & 0x2 == 0) {
                        return Err(util::other("invalid rsv2 bit set"));
                    }

                    self.rsv3 = first & 0x10 != 0;
                    if self.rsv3 && (self.reserved_bits & 0x1 == 0) {
                        return Err(util::other("invalid rsv3 bit set"));
                    }

                    self.opcode = OpCode::from((first & 0x0F) as u8);
                    if self.opcode.is_invalid() {
                        return Err(util::other("invalid opcode set"));
                    }
                    if self.opcode.is_control() && !self.fin {
                        return Err(util::other("control frames must not be fragmented"));
                    }

                    self.masked = second & 0x80 != 0;
                    if !self.masked && self.client {
                        return Err(util::other("all client frames must have a mask"));
                    }

                    self.length_code = (second & 0x7F) as u8;
                    self.state = DecodeState::HEADER;
                }
                DecodeState::HEADER => {
                    // println!("buf at state HEADER\n{}", util::as_hex(buf.as_slice()));
                    if self.length_code == TWO_EXT {
                        self.min_len += 2;
                        #[cfg_attr(feature = "cargo-clippy", allow(cast_possible_truncation))]
                        let size = self.min_len as usize;
                        if buf_len < size {
                            self.min_len -= 2;
                            return Ok(None);
                        }
                        let mut rdr = Cursor::new(buf.drain_to(2));
                        if let Ok(len) = rdr.read_u16::<BigEndian>() {
                            self.payload_length = len as u64;
                            self.state = DecodeState::LENGTH;
                        } else {
                            return Err(util::other("invalid length bytes"));
                        }
                    } else if self.length_code == EIGHT_EXT {
                        self.min_len += 8;
                        #[cfg_attr(feature = "cargo-clippy", allow(cast_possible_truncation))]
                        let size = self.min_len as usize;
                        if buf_len < size {
                            self.min_len -= 8;
                            return Ok(None);
                        }
                        let mut rdr = Cursor::new(buf.drain_to(8));
                        if let Ok(len) = rdr.read_u64::<BigEndian>() {
                            self.payload_length = len as u64;
                            self.state = DecodeState::LENGTH;
                        } else {
                            return Err(util::other("invalid length bytes"));
                        }
                    } else {
                        self.payload_length = self.length_code as u64;
                        self.state = DecodeState::LENGTH;
                    }
                    if self.payload_length > 125 && self.opcode.is_control() {
                        return Err(util::other("invalid control frame"));
                    }
                }
                DecodeState::LENGTH => {
                    if self.masked {
                        self.min_len += 4;
                        #[cfg_attr(feature = "cargo-clippy", allow(cast_possible_truncation))]
                        let size = self.min_len as usize;
                        if buf_len < size {
                            self.min_len -= 4;
                            return Ok(None);
                        }
                        let mut rdr = Cursor::new(buf.drain_to(4));
                        if let Ok(mask) = rdr.read_u32::<BigEndian>() {
                            self.mask_key = mask;
                            self.state = DecodeState::MASK;
                        } else {
                            return Err(util::other("invalid mask value"));
                        }
                    } else {
                        self.mask_key = 0;
                        self.state = DecodeState::MASK;
                    }
                }
                DecodeState::MASK => {
                    self.min_len += self.payload_length;
                    #[cfg_attr(feature = "cargo-clippy", allow(cast_possible_truncation))]
                    let size = self.min_len as usize;
                    let mask = self.mask_key;
                    if buf_len < size {
                        self.min_len -= self.payload_length;
                        if self.opcode == OpCode::Text {
                            let mut test_buf = buf.as_slice().to_vec();
                            if self.masked {
                                self.apply_mask(&mut test_buf, mask)?;
                                match utf8::validate(&test_buf) {
                                    Ok(Some(_)) => {}
                                    Ok(None) => return Ok(None),
                                    Err(_e) => {
                                        return Err(util::other("error during UTF-8 \
                                        validation"))
                                    }
                                }
                            } else {
                                return Err(util::other("cannot unmask data"));
                            }
                        }
                        return Ok(None);
                    }

                    if self.payload_length > 0 {
                        #[cfg_attr(feature = "cargo-clippy", allow(cast_possible_truncation))]
                        let mut app_data_bytes = buf.drain_to(self.payload_length as usize);
                        let mut adb = app_data_bytes.get_mut();
                        if self.masked {
                            self.apply_mask(&mut adb, mask)?;
                            self.application_data = Some(adb.to_vec());
                            self.state = DecodeState::FULL;
                        } else {
                            return Err(util::other("cannot unmask data"));
                        }
                    } else {
                        self.state = DecodeState::FULL;
                    }
                }
                DecodeState::FULL => break,
            }
        }

        Ok(Some(self.clone().into()))
    }

    fn encode(&mut self, msg: Self::Out, buf: &mut Vec<u8>) -> io::Result<()> {
        let mut first_byte = 0_u8;

        if msg.fin() {
            first_byte |= 0x80;
        }

        if msg.rsv1() {
            first_byte |= 0x40;
        }

        if msg.rsv2() {
            first_byte |= 0x20;
        }

        if msg.rsv3() {
            first_byte |= 0x10;
        }

        let opcode: u8 = msg.opcode().into();
        first_byte |= opcode;
        buf.push(first_byte);

        let mut second_byte = 0_u8;

        if msg.masked() {
            second_byte |= 0x80;
        }

        let len = msg.payload_length();
        if len < TWO_EXT as u64 {
            #[cfg_attr(feature = "cargo-clippy", allow(cast_possible_truncation))]
            let cast_len = len as u8;
            second_byte |= cast_len;
            buf.push(second_byte);
        } else if len < 65536 {
            second_byte |= TWO_EXT;
            let mut len_buf = Vec::with_capacity(2);
            #[cfg_attr(feature = "cargo-clippy", allow(cast_possible_truncation))]
            let cast_len = len as u16;
            len_buf.write_u16::<BigEndian>(cast_len)?;
            buf.push(second_byte);
            buf.extend(len_buf);
        } else {
            second_byte |= EIGHT_EXT;
            let mut len_buf = Vec::with_capacity(8);
            len_buf.write_u64::<BigEndian>(len)?;
            buf.push(second_byte);
            buf.extend(len_buf);
        }

        if msg.masked() {
            let mut mask_buf = Vec::with_capacity(4);
            mask_buf.write_u32::<BigEndian>(msg.mask())?;
            buf.extend(mask_buf);
        }

        if let Some(app_data) = msg.application_data() {
            buf.extend(app_data);
        }

        Ok(())
    }
}

impl From<FrameCodec> for Frame {
    fn from(frame_codec: FrameCodec) -> Frame {
        let mut frame: Frame = Default::default();
        frame.set_fin(frame_codec.fin);
        frame.set_rsv1(frame_codec.rsv1);
        frame.set_rsv2(frame_codec.rsv2);
        frame.set_rsv3(frame_codec.rsv3);
        frame.set_masked(frame_codec.masked);
        frame.set_opcode(frame_codec.opcode);
        frame.set_mask(frame_codec.mask_key);
        frame.set_payload_length(frame_codec.payload_length);
        frame.set_application_data(frame_codec.application_data);
        frame.set_extension_data(frame_codec.extension_data);
        frame
    }
}

#[cfg(test)]
mod test {
    use super::FrameCodec;
    use frame::base::{Frame, OpCode};
    use std::io;
    use tokio_core::io::{Codec, EasyBuf};
    use util;

    // Bad Frames, should err
    #[cfg_attr(rustfmt, rustfmt_skip)]
    // Mask bit must be one. 2nd byte must be 0x80 or greater.
    const NO_MASK: [u8; 2]           = [0x89, 0x00];
    #[cfg_attr(rustfmt, rustfmt_skip)]
    // Payload on control frame must be 125 bytes or less. 2nd byte must be 0xFD or less.
    const CTRL_PAYLOAD_LEN : [u8; 9] = [0x89, 0xFE, 0x10, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00];

    // Truncated Frames, should return Ok(None)
    #[cfg_attr(rustfmt, rustfmt_skip)]
    // One byte of the 2 byte header is ok.
    const PARTIAL_HEADER: [u8; 1]    = [0x89];
    #[cfg_attr(rustfmt, rustfmt_skip)]
    // Between 0 and 2 bytes of a 2 byte length block is ok.
    const PARTIAL_LENGTH_1: [u8; 3]  = [0x89, 0xFE, 0x01];
    #[cfg_attr(rustfmt, rustfmt_skip)]
    // Between 0 and 8 bytes of an 8 byte length block is ok.
    const PARTIAL_LENGTH_2: [u8; 6]  = [0x89, 0xFF, 0x01, 0x02, 0x03, 0x04];
    #[cfg_attr(rustfmt, rustfmt_skip)]
    // Between 0 and 4 bytes of the 4 byte mask is ok.
    const PARTIAL_MASK: [u8; 6]      = [0x82, 0xFE, 0x01, 0x02, 0x00, 0x00];
    #[cfg_attr(rustfmt, rustfmt_skip)]
    // Between 0 and X bytes of the X byte payload is ok.
    const PARTIAL_PAYLOAD: [u8; 8]    = [0x82, 0x85, 0x01, 0x02, 0x03, 0x04, 0x00, 0x00];

    // Good Frames, should return Ok(Some(x))
    #[cfg_attr(rustfmt, rustfmt_skip)]
    const PING_NO_DATA: [u8; 6]     = [0x89, 0x80, 0x00, 0x00, 0x00, 0x01];

    fn decode(buf: &[u8]) -> Result<Option<Frame>, io::Error> {
        let mut eb = EasyBuf::from(buf.to_vec());
        let mut fc: FrameCodec = Default::default();
        fc.set_client(true);
        fc.decode(&mut eb)
    }

    #[test]
    /// Checking that partial header returns Ok(None).
    fn decode_partial_header() {
        if let Ok(None) = decode(&PARTIAL_HEADER) {
            assert!(true);
        } else {
            assert!(false);
        }
    }

    #[test]
    /// Checking that partial 2 byte length returns Ok(None).
    fn decode_partial_len_1() {
        if let Ok(None) = decode(&PARTIAL_LENGTH_1) {
            assert!(true);
        } else {
            assert!(false);
        }
    }

    #[test]
    /// Checking that partial 8 byte length returns Ok(None).
    fn decode_partial_len_2() {
        if let Ok(None) = decode(&PARTIAL_LENGTH_2) {
            assert!(true);
        } else {
            assert!(false);
        }
    }

    #[test]
    /// Checking that partial mask returns Ok(None).
    fn decode_partial_mask() {
        if let Ok(None) = decode(&PARTIAL_MASK) {
            assert!(true);
        } else {
            assert!(false);
        }
    }

    #[test]
    /// Checking that partial payload returns Ok(None).
    fn decode_partial_payload() {
        if let Ok(None) = decode(&PARTIAL_PAYLOAD) {
            assert!(true);
        } else {
            assert!(false);
        }
    }

    #[test]
    /// Checking that partial mask returns Ok(None).
    fn decode_invalid_control_payload_len() {
        if let Err(_e) = decode(&CTRL_PAYLOAD_LEN) {
            assert!(true);
        } else {
            assert!(false);
        }
    }

    #[test]
    /// Checking that rsv1, rsv2, and rsv3 bit set returns error.
    fn decode_reserved() {
        // rsv1, rsv2, and rsv3.
        let reserved = [0x90, 0xa0, 0xc0];

        for res in &reserved {
            let mut buf = Vec::with_capacity(2);
            let mut first_byte = 0_u8;
            first_byte |= *res;
            buf.push(first_byte);
            buf.push(0x00);
            if let Err(_e) = decode(&buf) {
                assert!(true);
                // TODO: Assert error type when implemented.
            } else {
                util::stdo(&format!("rsv should not be set: {}", res));
                assert!(false);
            }
        }
    }

    #[test]
    /// Checking that a control frame, where fin bit is 0, returns an error.
    fn decode_fragmented_control() {
        let second_bytes = [8, 9, 10];

        for sb in &second_bytes {
            let mut buf = Vec::with_capacity(2);
            let mut first_byte = 0_u8;
            first_byte |= *sb;
            buf.push(first_byte);
            buf.push(0x00);
            if let Err(_e) = decode(&buf) {
                assert!(true);
                // TODO: Assert error type when implemented.
            } else {
                util::stdo("control frame {} is marked as fragment");
                assert!(false);
            }
        }
    }

    #[test]
    /// Checking that reserved opcodes return an error.
    fn decode_reserved_opcodes() {
        let reserved = [3, 4, 5, 6, 7, 11, 12, 13, 14, 15];

        for res in &reserved {
            let mut buf = Vec::with_capacity(2);
            let mut first_byte = 0_u8;
            first_byte |= 0x80;
            first_byte |= *res;
            buf.push(first_byte);
            buf.push(0x00);
            if let Err(_e) = decode(&buf) {
                assert!(true);
                // TODO: Assert error type when implemented.
            } else {
                util::stdo(&format!("opcode {} should be reserved", res));
                assert!(false);
            }
        }
    }

    #[test]
    /// Checking that a decode frame (always from client) with the mask bit not set returns an
    /// error.
    fn decode_no_mask() {
        if let Err(_e) = decode(&NO_MASK) {
            assert!(true);
            // TODO: Assert error type when implemented.
        } else {
            util::stdo("decoded frames should always have a mask");
            assert!(false);
        }
    }

    #[test]
    fn decode_ping_no_data() {
        if let Ok(Some(frame)) = decode(&PING_NO_DATA) {
            assert!(frame.fin());
            assert!(!frame.rsv1());
            assert!(!frame.rsv2());
            assert!(!frame.rsv3());
            assert!(frame.opcode() == OpCode::Ping);
            assert!(frame.payload_length() == 0);
            assert!(frame.extension_data().is_none());
            assert!(frame.application_data().is_none());
        } else {
            assert!(false);
        }
    }
}