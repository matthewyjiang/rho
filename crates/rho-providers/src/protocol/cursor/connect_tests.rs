use pretty_assertions::assert_eq;

use super::connect::{
    decode_connect_unary_body, encode_connect_frame, ConnectFrame, ConnectFrameParser,
    CONNECT_END_STREAM_FLAG,
};

// Covers: Connect frames split across reads must reassemble without dropping payload
// Owner: cursor protocol
#[test]
fn parser_reassembles_frames_split_across_pushes() {
    let payload = b"hello-connect";
    let encoded = encode_connect_frame(payload, 0);
    let mut parser = ConnectFrameParser::default();

    assert!(parser.push(&encoded[..3]).unwrap().is_empty());
    let frames = parser.push(&encoded[3..]).unwrap();

    assert_eq!(
        frames,
        vec![ConnectFrame {
            flags: 0,
            payload: payload.to_vec(),
        }]
    );
}

// Covers: compressed Connect frames must fail instead of being treated as protobuf
// Owner: cursor protocol
#[test]
fn parser_rejects_compressed_frames() {
    let encoded = encode_connect_frame(b"nope", 0b0000_0001);
    let error = ConnectFrameParser::default().push(&encoded).unwrap_err();
    assert_eq!(error, "compressed Connect frames are not supported");
}

// Covers: unary GetUsableModels bodies may wrap proto in a Connect envelope
// Owner: cursor protocol
#[test]
fn unary_body_unwraps_data_frame_and_skips_end_stream() {
    let proto = b"\x0a\x03abc";
    let mut body = encode_connect_frame(proto, 0);
    body.extend_from_slice(&encode_connect_frame(
        br#"{"error":null}"#,
        CONNECT_END_STREAM_FLAG,
    ));

    assert_eq!(decode_connect_unary_body(&body), Some(proto.as_slice()));
    assert_eq!(decode_connect_unary_body(proto), None);
}
