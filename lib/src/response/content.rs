//! Contains types that set the Content-Type of a response.
//!
//! # Usage
//!
//! Each type wraps a given responder. The `Responder` implementation of each
//! type replaces the Content-Type of the wrapped responder and delegates the
//! remainder of the response to the wrapped responder. This allows for setting
//! the Content-Type of a type that doesn't set it itself or for overriding one
//! that does.
//!
//! # Example
//!
//! The following snippet creates an `HTML` content response for a string.
//! Normally, raw strings set their response Content-Type to `text/plain`. By
//! using the `HTML` content response, the Content-Type will be set to
//! `text/html` instead.
//!
//! ```rust
//! use rocket::response::content;
//!
//! # #[allow(unused_variables)]
//! let response = content::HTML("<h1>Hello, world!</h1>");
//! ```

use futures::{Future, BoxFuture, IntoFuture};
use futures::future::FutureResult;

use request::Request;
use response::{RequestFuture, Response, Responder};
use http::{Status, ContentType};

/// Sets the Content-Type of a `Responder` to a chosen value.
///
/// Delagates the remainder of the response to the wrapped responder.
///
/// # Example
///
/// Set the Content-Type of a string to PDF.
///
/// ```rust
/// use rocket::response::content::Content;
/// use rocket::http::ContentType;
///
/// # #[allow(unused_variables)]
/// let response = Content(ContentType::PDF, "Hi.");
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct Content<R>(pub ContentType, pub R);

/// Overrides the Content-Type of the response to the wrapped `ContentType` then
/// delegates the remainder of the response to the wrapped responder.
impl<F: RequestFuture<Self>, R: Responder<Result<(Request, R), (Request, Status)>> + Sync + Send + 'static> Responder<F> for Content<R> 
    where F::Future: Send + 'static, R::Future: Send
{
    type Future = BoxFuture<(Request, Response), (Request, Status)>;

    #[inline(always)]
    fn respond_to(f: F) -> Self::Future {
        f.into_future().and_then(|(req, content)| {
            let (ty, responder) = (content.0, content.1);
            R::respond_to(Ok((req, responder))).and_then(|(req, resp)| {
                Response::build()
                    .merge(resp)
                    .header(ty)
                    .ok()
                    .map(|resp| (req, resp))
            })
        }).boxed()
    }
}

macro_rules! ctrs {
    ($($name:ident: $name_str:expr, $ct_str:expr),+) => {
        $(
            #[doc="Override the `Content-Type` of the response to <b>"]
            #[doc=$name_str]
            #[doc="</b>, or <i>"]
            #[doc=$ct_str]
            #[doc="</i>."]
            ///
            /// Delagates the remainder of the response to the wrapped responder.
            #[derive(Debug, Clone, PartialEq)]
            pub struct $name<R>(pub R);

            /// Sets the Content-Type of the response then delegates the
            /// remainder of the response to the wrapped responder.
            impl<F: RequestFuture<Self>, R: Responder<Result<(Request, R), (Request, Status)>> + Sync + Send + 'static> Responder<F> for $name<R> 
                where F::Future: Send + 'static, R::Future: Send
            {
                type Future = BoxFuture<(Request, Response), (Request, Status)>;

                fn respond_to(f: F) -> Self::Future {
                    f.into_future()
                     .and_then(|(req, self_)| Content::respond_to(Ok((req, Content(ContentType::$name, self_.0)))))
                     .boxed()
                }
            }
        )+
    }
}

ctrs! {
    JSON: "JSON", "application/json",
    XML: "XML", "text/xml",
    MsgPack: "MessagePack", "application/msgpack",
    HTML: "HTML", "text/html",
    Plain: "plain text", "text/plain",
    CSS: "CSS", "text/css",
    JavaScript: "JavaScript", "application/javascript"
}

