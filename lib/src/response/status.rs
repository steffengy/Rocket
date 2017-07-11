//! Contains types that set the status code and correspoding headers of a
//! response.
//!
//! These types are designed to make it easier to respond correctly with a given
//! status code. Each type takes in the minimum number of parameters required to
//! construct a proper response with that status code. Some types take in
//! responders; when they do, the responder finalizes the response by writing
//! out additional headers and, importantly, the body of the response.

use std::hash::{Hash, Hasher};
use std::collections::hash_map::DefaultHasher;

use futures::future::{Either, Future, BoxFuture, IntoFuture};
use futures::future::FutureResult;

use request::Request;
use response::{RequestFuture, Responder, Response};
use http::hyper::header;
use http::Status;

/// Sets the status of the response to 201 (Created).
///
/// The `String` field is set as the value of the `Location` header in the
/// response. The optional `Responder` field is used to finalize the response.
///
/// # Example
///
/// ```rust
/// use rocket::response::status;
///
/// let url = "http://myservice.com/resource.json".to_string();
/// let content = "{ 'resource': 'Hello, world!' }";
/// # #[allow(unused_variables)]
/// let response = status::Created(url, Some(content));
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct Created<R>(pub String, pub Option<R>);

/// Sets the status code of the response to 201 Created. Sets the `Location`
/// header to the `String` parameter in the constructor.
///
/// The optional responder finalizes the response if it exists. The wrapped
/// responder should write the body of the response so that it contains
/// information about the created resource. If no responder is provided, the
/// response body will be empty.
impl<F: RequestFuture<Self>, R: Responder<Result<(Request, R), (Request, Status)>> + Send + Sync + 'static> Responder<F> for Created<R> 
    where F::Future: Send + 'static, R::Future: Send
{
    type Future = BoxFuture<(Request, Response), (Request, Status)>;

    default fn respond_to(f: F) -> Self::Future {
        f.into_future().and_then(|(req, created)| {
            let mut build = Response::build();
            let (loc, responder) = (created.0, created.1);

            let future = if let Some(responder) = responder {
                Either::A(R::respond_to(Ok((req, responder))).map(|(req, resp)| {
                    build.merge(resp);
                    (build, req)
                }))
            } else {
                Either::B(Ok((build, req)).into_future())
            };

            future.and_then(|(mut build, req)| {
                build.status(Status::Created).header(header::Location::new(loc)).ok()
                    .map(|resp| (req, resp))
            })
        })
        .boxed()
    }
}

/// In addition to setting the status code, `Location` header, and finalizing
/// the response with the `Responder`, the `ETag` header is set conditionally if
/// a `Responder` is provided that implements `Hash`. The `ETag` header is set
/// to a hash value of the responder.
impl<F: RequestFuture<Self>, R: Responder<Result<(Request, R), (Request, Status)>> + Hash + Send + Sync + 'static> Responder<F> for Created<R> 
    where F::Future: Send + 'static, R::Future: Send
{
    fn respond_to(f: F) -> Self::Future {
        f.into_future().and_then(|(req, created)| {
            let mut hasher = DefaultHasher::default();
            let mut build = Response::build();
            let (loc, responder) = (created.0, created.1);

            let future = if let Some(responder) = responder {
                responder.hash(&mut hasher);
                let hash = hasher.finish().to_string();
                Either::A(R::respond_to(Ok((req, responder))).map(|(req, resp)| {
                    build.merge(resp);
                    build.header(header::ETag(header::EntityTag::strong(hash)));
                    (build, req)
                }))
            } else {
                Either::B(Ok((build, req)).into_future())
            };

            future.and_then(|(mut build, req)| {
                build.status(Status::Created).header(header::Location::new(loc)).ok()
                    .map(|resp| (req, resp))
            })
        }).boxed()
    }
}

/// Sets the status of the response to 202 (Accepted).
///
/// If a responder is supplied, the remainder of the response is delegated to
/// it. If there is no responder, the body of the response will be empty.
///
/// # Examples
///
/// A 202 Accepted response without a body:
///
/// ```rust
/// use rocket::response::status;
///
/// # #[allow(unused_variables)]
/// let response = status::Accepted::<()>(None);
/// ```
///
/// A 202 Accepted response _with_ a body:
///
/// ```rust
/// use rocket::response::status;
///
/// # #[allow(unused_variables)]
/// let response = status::Accepted(Some("processing"));
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct Accepted<R>(pub Option<R>);

/// Sets the status code of the response to 202 Accepted. If the responder is
/// `Some`, it is used to finalize the response.
impl<F: RequestFuture<Self>, R: Responder<Result<(Request, R), (Request, Status)>> + Sync + Send + 'static> Responder<F> for Accepted<R> 
    where F::Future: Send + 'static, R::Future: Send,
{
    type Future = BoxFuture<(Request, Response), (Request, Status)>;

    fn respond_to(f: F) -> Self::Future {
        f.into_future().and_then(|(req, accepted)| {
            let mut build = Response::build();

            let future = if let Some(responder) = accepted.0 {
                Either::A(R::respond_to(Ok((req, responder))).map(|(req, resp)| {
                    build.merge(resp);
                    (build, req)
                }))
            } else {
                Either::B(Ok((build, req)).into_future())
            };

            future.and_then(|(mut build, req)| {
                build.status(Status::Accepted).ok().map(|resp| (req, resp))
            })
        }).boxed()
    }
}

/// Sets the status of the response to 204 (No Content).
///
/// # Example
///
/// ```rust
/// use rocket::response::status;
///
/// # #[allow(unused_variables)]
/// let response = status::NoContent;
/// ```
// TODO: This would benefit from Header support.
#[derive(Debug, Clone, PartialEq)]
pub struct NoContent;

/// Sets the status code of the response to 204 No Content. The body of the
/// response will be empty.
impl<F: RequestFuture<Self>> Responder<F> for NoContent where F::Future: Send + 'static {
    type Future = BoxFuture<(Request, Response), (Request, Status)>;

    fn respond_to(f: F) -> Self::Future {
        f.into_future().and_then(|(req, no_content)| {
            Response::build().status(Status::NoContent).ok()
            .map(|resp| (req, resp))
        }).boxed()
    }
}


/// Sets the status of the response to 205 (Reset Content).
///
/// # Example
///
/// ```rust
/// use rocket::response::status;
///
/// # #[allow(unused_variables)]
/// let response = status::Reset;
/// ```
#[derive(Debug, Copy, Clone, PartialEq)]
pub struct Reset;

/// Sets the status code of the response to 205 Reset Content. The body of the
/// response will be empty.
impl<F: RequestFuture<Self>> Responder<F> for Reset where F::Future: Send + 'static {
    type Future = BoxFuture<(Request, Response), (Request, Status)>;

    fn respond_to(f: F) -> Self::Future {
        f.into_future().and_then(|(req, reset)| {
            Response::build().status(Status::ResetContent).ok()
            .map(|resp| (req, resp))
        }).boxed()
    }
}

/// Sets the status of the response to 404 (Not Found).
///
/// The remainder of the response is delegated to the wrapped `Responder`.
///
/// # Example
///
/// ```rust
/// use rocket::response::status;
///
/// # #[allow(unused_variables)]
/// let response = status::NotFound("Sorry, I couldn't find it!");
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct NotFound<R>(pub R);

/// Sets the status code of the response to 404 Not Found.
impl<F: RequestFuture<Self>, R: Responder<Result<(Request, R), (Request, Status)>> + 'static> Responder<F> for NotFound<R> 
    where F::Future: Send + 'static, R::Future: Send
{
    type Future = BoxFuture<(Request, Response), (Request, Status)>;

    fn respond_to(f: F) -> Self::Future {
        f.into_future().and_then(|(req, not_found)| {
            R::respond_to(Ok((req, not_found.0))).and_then(|(req, response)| {
                Response::build_from(response)
                    .status(Status::NotFound)
                    .ok()
                    .map(|resp| (req, resp))
            })
        }).boxed()
    }
}

/// Creates a response with the given status code and underyling responder.
///
/// # Example
///
/// ```rust
/// use rocket::response::status;
/// use rocket::http::Status;
///
/// # #[allow(unused_variables)]
/// let response = status::Custom(Status::ImATeapot, "Hi!");
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct Custom<R>(pub Status, pub R);

/// Sets the status code of the response and then delegates the remainder of the
/// response to the wrapped responder.
impl<F: RequestFuture<Self>, R: Responder<Result<(Request, R), (Request, Status)>> + Sync + Send + 'static> Responder<F> for Custom<R> 
    where F::Future: Send + 'static, R::Future: Send
{
    type Future = BoxFuture<(Request, Response), (Request, Status)>;

    fn respond_to(f: F) -> Self::Future {
        f.into_future().and_then(|(req, custom)| {
            let (status, responder) = (custom.0, custom.1);
            R::respond_to(Ok((req, responder))).and_then(move |(req, response)| {
                Response::build_from(response)
                    .status(status)
                    .ok()
                    .map(|resp| (req, resp))
            })
        })
        .boxed()
    }
}

// The following are unimplemented.
// 206 Partial Content (variant), 203 Non-Authoritative Information (headers).
