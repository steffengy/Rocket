use request::Request;
use response::{Response, Responder};
use http::Status;

/// A failing response; simply forwards to the catcher for the given
/// `Status`.
#[derive(Debug)]
pub struct Failure(pub Status);

impl Responder for Failure {
    fn respond_to(self, _: &Request) -> Result<Response, Status> {
        Err(self.0)
    }
}
