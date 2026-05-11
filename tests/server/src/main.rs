use tokio_stream::wrappers::ReceiverStream;
use tokio_stream::StreamExt;
use tonic::{transport::Server, Request, Response, Status, Streaming};

pub mod pb {
    tonic::include_proto!("grpc.testing");
}

use pb::test_service_server::{TestService, TestServiceServer};
use pb::{Empty, Payload};

#[derive(Default)]
pub struct TestServiceImpl;

#[tonic::async_trait]
impl TestService for TestServiceImpl {
    type StreamEchoStream = ReceiverStream<Result<Payload, Status>>;
    type BidiEchoStream = ReceiverStream<Result<Payload, Status>>;

    async fn echo(&self, request: Request<Payload>) -> Result<Response<Payload>, Status> {
        let mut response = Response::new(request.into_inner());
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
        // Attach a binary trailer via Status metadata. This is the path real
        // services use for rich-status / google.rpc.Status propagation in
        // `grpc-status-details-bin`. Surfaces to PHP through the trailing
        // metadata in OP_RECV_STATUS_ON_CLIENT.
        let mut md = tonic::metadata::MetadataMap::new();
        md.insert_bin(
            "x-test-binary-bin",
            tonic::metadata::MetadataValue::from_bytes(b"hello\x00\xfftrailer"),
        );
        Err(Status::with_metadata(
            tonic::Code::Internal,
            "test error",
            md,
        ))
    }

    async fn stream_echo(
        &self,
        request: Request<Payload>,
    ) -> Result<Response<Self::StreamEchoStream>, Status> {
        let payload = request.into_inner();
        let (tx, rx) = tokio::sync::mpsc::channel(4);

        tokio::spawn(async move {
            // Send the payload back 3 times
            for _ in 0..3 {
                if tx.send(Ok(payload.clone())).await.is_err() {
                    break;
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

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let addr = "0.0.0.0:50051".parse()?;
    eprintln!("TestServer listening on {addr}");

    Server::builder()
        .add_service(TestServiceServer::new(TestServiceImpl::default()))
        .serve(addr)
        .await?;

    Ok(())
}
