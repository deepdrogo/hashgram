//! The `/hashgram/rpc/1` request-response codec.
//!
//! Length-prefixed protobuf with the prefix checked against
//! [`hashgram_proto::limits::MAX_RPC_FRAME`] before the body is read. A peer
//! that declares an oversized frame is refused four bytes in, having cost
//! this node nothing but the prefix read.

use std::io;

use futures::prelude::*;
use hashgram_proto::frame::read_prefix;
use hashgram_proto::limits::MAX_RPC_FRAME;
use hashgram_proto::pb;
use libp2p::request_response;
use libp2p::StreamProtocol;
use prost::Message;

/// The protocol id.
pub const RPC_PROTOCOL: StreamProtocol = StreamProtocol::new("/hashgram/rpc/1");

/// The codec.
#[derive(Debug, Clone, Default)]
pub(crate) struct RpcCodec;

async fn read_message<T, M>(io: &mut T) -> io::Result<M>
where
    T: AsyncRead + Unpin + Send,
    M: Message + Default,
{
    let mut prefix = [0u8; 4];
    io.read_exact(&mut prefix).await?;
    let len = read_prefix(prefix, MAX_RPC_FRAME)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    let mut body = vec![0u8; len];
    io.read_exact(&mut body).await?;
    M::decode(body.as_slice()).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

async fn write_message<T, M>(io: &mut T, msg: &M) -> io::Result<()>
where
    T: AsyncWrite + Unpin + Send,
    M: Message,
{
    let len = msg.encoded_len();
    if len > MAX_RPC_FRAME {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("outbound frame of {len} bytes exceeds {MAX_RPC_FRAME}"),
        ));
    }
    let mut buf = Vec::with_capacity(4 + len);
    buf.extend_from_slice(&(len as u32).to_be_bytes());
    msg.encode(&mut buf)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
    io.write_all(&buf).await?;
    io.close().await
}

#[async_trait::async_trait]
impl request_response::Codec for RpcCodec {
    type Protocol = StreamProtocol;
    type Request = pb::Request;
    type Response = pb::Response;

    async fn read_request<T>(&mut self, _: &StreamProtocol, io: &mut T) -> io::Result<pb::Request>
    where
        T: AsyncRead + Unpin + Send,
    {
        read_message(io).await
    }

    async fn read_response<T>(&mut self, _: &StreamProtocol, io: &mut T) -> io::Result<pb::Response>
    where
        T: AsyncRead + Unpin + Send,
    {
        read_message(io).await
    }

    async fn write_request<T>(
        &mut self,
        _: &StreamProtocol,
        io: &mut T,
        req: pb::Request,
    ) -> io::Result<()>
    where
        T: AsyncWrite + Unpin + Send,
    {
        write_message(io, &req).await
    }

    async fn write_response<T>(
        &mut self,
        _: &StreamProtocol,
        io: &mut T,
        res: pb::Response,
    ) -> io::Result<()>
    where
        T: AsyncWrite + Unpin + Send,
    {
        write_message(io, &res).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use libp2p::request_response::Codec as _;

    #[tokio::test]
    async fn round_trips_a_request() {
        let req = pb::Request {
            body: Some(pb::request::Body::PeerExchange(pb::PeerExchange {
                limit: 5,
            })),
        };
        let mut buf = Vec::new();
        RpcCodec
            .write_request(&RPC_PROTOCOL, &mut buf, req.clone())
            .await
            .unwrap();
        let mut cursor = futures::io::Cursor::new(buf);
        let back = RpcCodec
            .read_request(&RPC_PROTOCOL, &mut cursor)
            .await
            .unwrap();
        assert_eq!(back, req);
    }

    #[tokio::test]
    async fn an_oversized_prefix_is_refused_without_reading_the_body() {
        let mut buf = (MAX_RPC_FRAME as u32 + 1).to_be_bytes().to_vec();
        buf.extend_from_slice(&[0u8; 16]);
        let mut cursor = futures::io::Cursor::new(buf);
        let err = RpcCodec
            .read_request(&RPC_PROTOCOL, &mut cursor)
            .await
            .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        // Only the prefix was consumed.
        assert_eq!(cursor.position(), 4);
    }

    #[tokio::test]
    async fn garbage_is_an_error_not_a_panic() {
        for len in 0..40usize {
            let mut cursor = futures::io::Cursor::new(vec![0x9fu8; len]);
            let _ = RpcCodec.read_response(&RPC_PROTOCOL, &mut cursor).await;
        }
    }
}
