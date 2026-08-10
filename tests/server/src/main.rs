use tokio_stream::wrappers::ReceiverStream;
use tokio_stream::StreamExt;
use tonic::{transport::Server, Request, Response, Status, Streaming};

pub mod pb {
    tonic::include_proto!("grpc.testing");
}

use pb::test_service_server::{TestService, TestServiceServer};
use pb::{Empty, Payload};

/// Per-call delay of the SlowEcho method. Kept in sync with the same constant
/// in tests/test_unary_split.php.
const SLOW_ECHO_DELAY: std::time::Duration = std::time::Duration::from_millis(250);

#[derive(Default)]
pub struct TestServiceImpl;

#[tonic::async_trait]
impl TestService for TestServiceImpl {
    type StreamEchoStream = ReceiverStream<Result<Payload, Status>>;
    type BidiEchoStream = ReceiverStream<Result<Payload, Status>>;

    async fn echo(&self, request: Request<Payload>) -> Result<Response<Payload>, Status> {
        let payload = request.into_inner();
        // A body of "sleep:N" delays the response by N ms (used to test
        // concurrent in-flight calls).
        if let Some(ms) = std::str::from_utf8(&payload.body)
            .ok()
            .and_then(|s| s.strip_prefix("sleep:"))
            .and_then(|n| n.parse::<u64>().ok())
        {
            tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
        }
        let mut response = Response::new(payload);
        // Attach a binary trailer so PHP clients can verify their extension
        // surfaces `-bin` metadata. Payload contains a NUL and non-UTF-8 byte
        // to make sure raw bytes survive intact (no string conversion).
        response.metadata_mut().insert_bin(
            "x-test-binary-bin",
            tonic::metadata::MetadataValue::from_bytes(b"hello\x00\xfftrailer"),
        );
        Ok(response)
    }

    async fn empty_response(&self, _request: Request<Payload>) -> Result<Response<Empty>, Status> {
        Ok(Response::new(Empty {}))
    }

    /// Echo after a fixed delay, so a client can tell concurrent calls from
    /// serialised ones: N calls take ~SLOW_ECHO_DELAY when they overlap and
    /// ~N × SLOW_ECHO_DELAY when they do not.
    async fn slow_echo(&self, request: Request<Payload>) -> Result<Response<Payload>, Status> {
        tokio::time::sleep(SLOW_ECHO_DELAY).await;
        Ok(Response::new(request.into_inner()))
    }

    /// Echo the request metadata (and :authority, injected by the transport
    /// layer as x-seen-authority) into the response body, one sorted line per
    /// key: "key:hex(v1),hex(v2)". Lets clients verify what actually reached
    /// the wire — send-metadata parity, multi-values, -bin keys, authority.
    async fn echo_metadata(&self, request: Request<Payload>) -> Result<Response<Payload>, Status> {
        use std::collections::BTreeMap;
        use tonic::metadata::KeyAndValueRef;

        let mut by_key: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for kv in request.metadata().iter() {
            match kv {
                KeyAndValueRef::Ascii(k, v) => {
                    by_key
                        .entry(k.as_str().to_string())
                        .or_default()
                        .push(hex(v.as_encoded_bytes()));
                }
                KeyAndValueRef::Binary(k, v) => {
                    let decoded = v.to_bytes().map_err(|e| {
                        Status::internal(format!("bad binary metadata: {e}"))
                    })?;
                    by_key
                        .entry(k.as_str().to_string())
                        .or_default()
                        .push(hex(&decoded));
                }
            }
        }

        let body: String = by_key
            .iter()
            .map(|(k, vs)| format!("{k}:{}\n", vs.join(",")))
            .collect();

        Ok(Response::new(Payload { body: body.into() }))
    }

    async fn large_response(
        &self,
        _request: Request<Payload>,
    ) -> Result<Response<Payload>, Status> {
        let body = vec![0x42u8; 64 * 1024]; // 64KB
        Ok(Response::new(Payload {
            body: body.into(),
        }))
    }

    async fn error_response(
        &self,
        _request: Request<Payload>,
    ) -> Result<Response<Payload>, Status> {
        // `with_details` serializes this as tonic's reserved
        // `grpc-status-details-bin` header. The client then reconstructs the
        // Status using `Status::from_header_map`, which moves this payload out
        // of `status.metadata()` and into `status.details()`.
        Err(Status::with_details(
            tonic::Code::Internal,
            "test error",
            b"hello\x00\xfftrailer".to_vec().into(),
        ))
    }

    async fn stream_echo(
        &self,
        request: Request<Payload>,
    ) -> Result<Response<Self::StreamEchoStream>, Status> {
        let payload = request.into_inner();
        let (tx, rx) = tokio::sync::mpsc::channel(4);

        // A body of "repeat:N" streams N single-byte messages (used by
        // benchmarks); any other body is echoed back 3 times.
        let repeat = std::str::from_utf8(&payload.body)
            .ok()
            .and_then(|s| s.strip_prefix("repeat:"))
            .and_then(|n| n.parse::<usize>().ok());

        tokio::spawn(async move {
            // End a stream after response headers with a rich status. This
            // exercises the client's `Streaming::message()` error path.
            if payload.body.as_slice() == b"stream-status-error" {
                let _ = tx
                    .send(Err(Status::with_details(
                        tonic::Code::Internal,
                        "stream test error",
                        b"hello\x00\xfftrailer".to_vec().into(),
                    )))
                    .await;
                return;
            }

            match repeat {
                Some(n) => {
                    let msg = Payload { body: b"x".to_vec() };
                    for _ in 0..n {
                        if tx.send(Ok(msg.clone())).await.is_err() {
                            break;
                        }
                    }
                }
                None => {
                    for _ in 0..3 {
                        if tx.send(Ok(payload.clone())).await.is_err() {
                            break;
                        }
                    }
                }
            }
        });

        Ok(Response::new(ReceiverStream::new(rx)))
    }

    async fn collect_payloads(
        &self,
        request: Request<Streaming<Payload>>,
    ) -> Result<Response<Payload>, Status> {
        let mut stream = request.into_inner();
        let mut collected = Vec::new();

        while let Some(payload) = stream.next().await {
            let payload = payload?;
            collected.extend_from_slice(&payload.body);
        }

        Ok(Response::new(Payload {
            body: collected.into(),
        }))
    }

    async fn bidi_echo(
        &self,
        request: Request<Streaming<Payload>>,
    ) -> Result<Response<Self::BidiEchoStream>, Status> {
        let mut stream = request.into_inner();
        let (tx, rx) = tokio::sync::mpsc::channel(4);

        tokio::spawn(async move {
            while let Some(payload) = stream.next().await {
                match payload {
                    Ok(p) => {
                        if tx.send(Ok(p)).await.is_err() {
                            break;
                        }
                    }
                    Err(e) => {
                        let _ = tx.send(Err(e)).await;
                        break;
                    }
                }
            }
        });

        Ok(Response::new(ReceiverStream::new(rx)))
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Copy the request :authority into an x-seen-authority header before tonic
/// strips the URI, so EchoMetadata can surface it to clients.
fn inject_authority<B>(mut req: http::Request<B>) -> http::Request<B> {
    let authority = req
        .uri()
        .authority()
        .map(|a| a.to_string())
        .unwrap_or_default();
    if let Ok(value) = http::HeaderValue::from_str(&authority) {
        req.headers_mut().insert("x-seen-authority", value);
    }
    req
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let plain_addr = "0.0.0.0:50051".parse()?;
    let tls_addr: std::net::SocketAddr = "0.0.0.0:50052".parse()?;
    eprintln!("TestServer listening on {plain_addr} (plaintext) and {tls_addr} (TLS)");

    // Long-lived self-signed test PKI, baked in at compile time
    // (tests/server/tls/, CA at tests/server/tls/ca.crt for clients).
    let identity = tonic::transport::Identity::from_pem(
        include_str!("../tls/server.crt"),
        include_str!("../tls/server.key"),
    );

    let tls = tokio::spawn(
        Server::builder()
            .tls_config(tonic::transport::ServerTlsConfig::new().identity(identity))?
            .layer(tower::util::MapRequestLayer::new(inject_authority))
            .add_service(TestServiceServer::new(TestServiceImpl::default()))
            .serve(tls_addr),
    );

    let plain = tokio::spawn(
        Server::builder()
            .layer(tower::util::MapRequestLayer::new(inject_authority))
            .add_service(TestServiceServer::new(TestServiceImpl::default()))
            .serve(plain_addr),
    );

    let (p, t) = tokio::try_join!(plain, tls)?;
    p?;
    t?;

    Ok(())
}
