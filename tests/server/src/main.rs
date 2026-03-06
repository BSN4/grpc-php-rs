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
