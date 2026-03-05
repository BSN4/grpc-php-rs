use bytes::{Buf, BufMut, Bytes};
use tonic::codec::{Codec, Decoder, Encoder};
use tonic::Status;

#[derive(Debug, Clone, Copy)]
pub struct RawBytesCodec;

#[derive(Debug, Clone)]
pub struct RawBytesEncoder;

#[derive(Debug, Clone)]
pub struct RawBytesDecoder;

impl Encoder for RawBytesEncoder {
    type Item = Bytes;
    type Error = Status;

    fn encode(&mut self, item: Self::Item, dst: &mut tonic::codec::EncodeBuf<'_>) -> Result<(), Self::Error> {
        dst.put(item);
        Ok(())
    }
}

impl Decoder for RawBytesDecoder {
    type Item = Bytes;
    type Error = Status;

    fn decode(&mut self, src: &mut tonic::codec::DecodeBuf<'_>) -> Result<Option<Self::Item>, Self::Error> {
        let remaining = src.remaining();
        if remaining == 0 {
            // Return empty Bytes for zero-length messages (e.g. google.protobuf.Empty).
            // Returning None would signal "no message" and cause tonic to throw
            // "Missing response message."
            return Ok(Some(Bytes::new()));
        }
        Ok(Some(src.copy_to_bytes(remaining)))
    }
}

impl Codec for RawBytesCodec {
    type Encode = Bytes;
    type Decode = Bytes;
    type Encoder = RawBytesEncoder;
    type Decoder = RawBytesDecoder;

    fn encoder(&mut self) -> Self::Encoder {
        RawBytesEncoder
    }

    fn decoder(&mut self) -> Self::Decoder {
        RawBytesDecoder
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tonic::codec::Codec;

    #[test]
    fn codec_produces_encoder_and_decoder() {
        let mut codec = RawBytesCodec;
        let _encoder = codec.encoder();
        let _decoder = codec.decoder();
    }
}
