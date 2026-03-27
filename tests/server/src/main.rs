use tokio_stream::wrappers::ReceiverStream;
use tonic::{transport::Server, Request, Response, Status};

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

    async fn echo(&self, request: Request<Payload>) -> Result<Response<Payload>, Status> {
        Ok(Response::new(request.into_inner()))
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
        Err(Status::internal("test error"))
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
