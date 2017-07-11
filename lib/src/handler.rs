//! The types of request and error handlers and their return values.

use futures::{Future, IntoFuture, BoxFuture};

use data::Data;
use request::Request;
use response::{self, RequestFuture, Response, Responder};
use error::Error;
use http::Status;
use outcome;

/// Type alias for the `Outcome` of a `Handler`.
pub type Outcome = outcome::Outcome<Response, Status, Data>;

impl Outcome {
    /// Return the `Outcome` of response to `req` from `responder`.
    ///
    /// If the responder responds with `Ok`, an outcome of `Success` is returns
    /// with the response. If the outcomes reeturns `Err`, an outcome of
    /// `Failure` is returned with the status code.
    ///
    /// # Example
    ///
    /// ```rust
    /// use rocket::{Request, Data};
    /// use rocket::handler::Outcome;
    ///
    /// fn str_responder(req: &Request, _: Data) -> Outcome<'static> {
    ///     Outcome::from(req, "Hello, world!")
    /// }
    /// ```
    #[inline]
    pub fn from<F, T>(req: Request, f: F) -> impl Future<Item=(Request, Outcome), Error=Request>
        where F: IntoFuture<Item=T, Error=Status>, F::Future: Send, T: Responder<Result<(Request, T), (Request, Status)>>
    {
        f.into_future().then(|ret| match ret {
            Ok(ret) => Ok((req, ret)),
            Err(e) => Err(req),
        }).and_then(|(req, ret)| {
            T::respond_to(Ok((req, ret))).then(|result| {
                match result {
                    Ok((req, response)) => Ok((req, outcome::Outcome::Success(response))),
                    Err((req, status)) => Ok((req, outcome::Outcome::Failure(status)))
                }
            })
        })
    }

    /// Return an `Outcome` of `Failure` with the status code `code`. This is
    /// equivalent to `Outcome::Failure(code)`.
    ///
    /// This method exists to be used during manual routing where
    /// `rocket::handler::Outcome` is imported instead of `rocket::Outcome`.
    ///
    /// # Example
    ///
    /// ```rust
    /// use rocket::{Request, Data};
    /// use rocket::handler::Outcome;
    /// use rocket::http::Status;
    ///
    /// fn bad_req_route(_: &Request, _: Data) -> Outcome<'static> {
    ///     Outcome::failure(Status::BadRequest)
    /// }
    /// ```
    #[inline(always)]
    pub fn failure(code: Status) -> Outcome {
        outcome::Outcome::Failure(code)
    }

    /// Return an `Outcome` of `Forward` with the data `data`. This is
    /// equivalent to `Outcome::Forward(data)`.
    ///
    /// This method exists to be used during manual routing where
    /// `rocket::handler::Outcome` is imported instead of `rocket::Outcome`.
    ///
    /// # Example
    ///
    /// ```rust
    /// use rocket::{Request, Data};
    /// use rocket::handler::Outcome;
    ///
    /// fn always_forward(_: &Request, data: Data) -> Outcome {
    ///     Outcome::forward(data)
    /// }
    /// ```
    #[inline(always)]
    pub fn forward(data: Data) -> Outcome {
        outcome::Outcome::Forward(data)
    }
}

/// The type of a request handler.
pub type Handler = fn(Request, Data) -> BoxFuture<(Request, Outcome), Request>;

/// The type of an error handler.
pub type ErrorHandler = fn(Error, Request) -> BoxFuture<(Request, response::Response), (Request, Status)>;
