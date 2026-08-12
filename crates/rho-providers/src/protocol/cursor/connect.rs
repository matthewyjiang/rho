//! Connect protocol framing for Cursor's protobuf HTTP/2 streams.

pub(crate) const CONNECT_END_STREAM_FLAG: u8 = 0b0000_0010;
const CONNECT_COMPRESSED_FLAG: u8 = 0b0000_0001;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ConnectFrame {
    pub flags: u8,
    pub payload: Vec<u8>,
}

impl ConnectFrame {
    pub(crate) fn is_end_stream(&self) -> bool {
        self.flags & CONNECT_END_STREAM_FLAG != 0
    }
}

pub(crate) fn encode_connect_frame(payload: &[u8], flags: u8) -> Vec<u8> {
    let mut frame = Vec::with_capacity(5 + payload.len());
    frame.push(flags);
    frame.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    frame.extend_from_slice(payload);
    frame
}

pub(crate) fn encode_client_message(message: &impl prost::Message) -> Vec<u8> {
    encode_connect_frame(&message.encode_to_vec(), 0)
}

/// Incremental parser for Connect data frames.
#[derive(Clone, Debug, Default)]
pub(crate) struct ConnectFrameParser {
    pending: Vec<u8>,
}

impl ConnectFrameParser {
    pub(crate) fn push(&mut self, chunk: &[u8]) -> Result<Vec<ConnectFrame>, &'static str> {
        self.pending.extend_from_slice(chunk);
        let mut frames = Vec::new();
        loop {
            if self.pending.len() < 5 {
                break;
            }
            let flags = self.pending[0];
            let len =
                u32::from_be_bytes(self.pending[1..5].try_into().expect("slice of 4 is a u32"))
                    as usize;
            let frame_end = 5 + len;
            if self.pending.len() < frame_end {
                break;
            }
            if flags & CONNECT_COMPRESSED_FLAG != 0 {
                return Err("compressed Connect frames are not supported");
            }
            let payload = self.pending[5..frame_end].to_vec();
            self.pending.drain(..frame_end);
            frames.push(ConnectFrame { flags, payload });
        }
        Ok(frames)
    }
}

/// Strips a Connect unary envelope when the body is framed rather than raw proto.
pub(crate) fn decode_connect_unary_body(payload: &[u8]) -> Option<&[u8]> {
    if payload.len() < 5 {
        return None;
    }
    let mut offset = 0;
    while offset + 5 <= payload.len() {
        let flags = payload[offset];
        let len = u32::from_be_bytes(payload[offset + 1..offset + 5].try_into().ok()?) as usize;
        let frame_end = offset + 5 + len;
        if frame_end > payload.len() {
            return None;
        }
        if flags & CONNECT_COMPRESSED_FLAG != 0 {
            return None;
        }
        if flags & CONNECT_END_STREAM_FLAG == 0 {
            return Some(&payload[offset + 5..frame_end]);
        }
        offset = frame_end;
    }
    None
}
