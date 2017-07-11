use futures::{Future, BoxFuture, IntoFuture};
use futures::future::FutureResult;

use request::Request;
use response::{RequestFuture, Response, Responder};
use http::Status;

/// A failing response; simply forwards to the catcher for the given
/// `Status`.
#[derive(Debug)]
pub struct Failure(pub Status);

impl<F: RequestFuture<Self>> Responder<F> for Failure where F::Future: Send + 'static {
    type Future = BoxFuture<(Request, Response), (Request, Status)>;

    fn respond_to(f: F) -> Self::Future {
        f.into_future().and_then(|(req, failure)| Err((req, failure.0))).boxed()
    }
}
