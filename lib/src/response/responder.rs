use std::fs::File;
use std::io::Cursor;
use std::fmt;

use futures::{self, BoxFuture, Future, IntoFuture};
use futures::future::{Either, FutureResult};

use http::{Status, ContentType};
use response::Response;
use request::Request;

/// Trait implemented by types that generate responses for clients.
///
/// Types that implement this trait can be used as the return type of a handler,
/// as illustrated below:
///
/// ```rust,ignore
/// #[get("/")]
/// fn index() -> T { ... }
/// ```
///
/// In this example, `T` can be any type that implements `Responder`.
///
/// # Return Value
///
/// A `Responder` returns an `Ok(Response)` or an `Err(Status)`:
///
///   * An `Ok` variant means that the `Responder` was successful in generating
///     a `Response`. The `Response` will be written out to the client.
///
///   * An `Err` variant means that the `Responder` could not or did not
///     generate a `Response`. The contained `Status` will be used to find the
///     relevant error catcher which then generates an error response.
///
/// # Provided Implementations
///
/// Rocket implements `Responder` for several standard library types. Their
/// behavior is documented here. Note that the `Result` implementation is
/// overloaded, allowing for two `Responder`s to be used at once, depending on
/// the variant.
///
///   * **&str**
///
///     Sets the `Content-Type` to `text/plain`. The string is used as the body
///     of the response, which is fixed size and not streamed. To stream a raw
///     string, use `Stream::from(Cursor::new(string))`.
///
///   * **String**
///
///     Sets the `Content-Type`t to `text/plain`. The string is used as the body
///     of the response, which is fixed size and not streamed. To stream a
///     string, use `Stream::from(Cursor::new(string))`.
///
///   * **File**
///
///     Responds with a streamed body containing the data in the `File`. No
///     `Content-Type` is set. To automatically have a `Content-Type` set based
///     on the file's extension, use
///     [`NamedFile`](/rocket/response/struct.NamedFile.html).
///
///   * **()**
///
///     Responds with an empty body. No `Content-Type` is set.
///
///   * **Option&lt;T>**
///
///     If the `Option` is `Some`, the wrapped responder is used to respond to
///     the client. Otherwise, an `Err` with status **404 Not Found** is
///     returned and a warning is printed to the console.
///
///   * **Result&lt;T, E>** _where_ **E: Debug**
///
///     If the `Result` is `Ok`, the wrapped responder is used to respond to the
///     client. Otherwise, an `Err` with status **500 Internal Server Error** is
///     returned and the error is printed to the console using the `Debug`
///     implementation.
///
///   * **Result&lt;T, E>** _where_ **E: Debug + Responder**
///
///     If the `Result` is `Ok`, the wrapped `Ok` responder is used to respond
///     to the client. If the `Result` is `Err`, the wrapped `Err` responder is
///     used to respond to the client.
///
/// # Implementation Tips
///
/// This section describes a few best practices to take into account when
/// implementing `Responder`.
///
/// ## Debug
///
/// A type implementing `Responder` should implement the `Debug` trait when
/// possible. This is because the `Responder` implementation for `Result`
/// requires its `Err` type to implement `Debug`. Therefore, a type implementing
/// `Debug` can more easily be composed.
///
/// ## Joining and Merging
///
/// When chaining/wrapping other `Responder`s, use the
/// [merge](/rocket/struct.Response.html#method.merge) or
/// [join](/rocket/struct.Response.html#method.join) methods on the `Response`
/// or `ResponseBuilder` struct. Ensure that you document the merging or joining
/// behavior appropriately.
///
/// ## Inspecting Requests
///
/// A `Responder` has access to the request it is responding to. Even so, you
/// should avoid using the `Request` value as much as possible. This is because
/// using the `Request` object makes your responder _inpure_, and so the use of
/// the type as a `Responder` has less intrinsic meaning associated with it. If
/// the `Responder` were pure, however, it always respond in the same manner,
/// regardless of the incoming request. Thus, knowing the type is sufficient to
/// fully determine its functionality.
///
/// # Example
///
/// Say that you have a custom type, `Person`:
///
/// ```rust
///
/// # #[allow(dead_code)]
/// struct Person {
///     name: String,
///     age: u16
/// }
/// ```
///
/// You'd like to use `Person` as a `Responder` so that you can return a
/// `Person` directly from a handler:
///
/// ```rust,ignore
/// #[get("/person/<id>")]
/// fn person(id: usize) -> Option<Person> {
///     Person::from_id(id)
/// }
/// ```
///
/// You want the `Person` responder to set two header fields: `X-Person-Name`
/// and `X-Person-Age` as well as supply a custom representation of the object
/// (`Content-Type: application/x-person`) in the body of the response. The
/// following `Responder` implementation accomplishes this:
///
/// ```rust
/// # #![feature(plugin)]
/// # #![plugin(rocket_codegen)]
/// # extern crate rocket;
/// #
/// # #[derive(Debug)]
/// # struct Person { name: String, age: u16 }
/// #
/// use std::io::Cursor;
///
/// use rocket::request::Request;
/// use rocket::response::{self, Response, Responder};
/// use rocket::http::ContentType;
///
/// impl Responder<'static> for Person {
///     fn respond_to(self, _: &Request) -> response::Result<'static> {
///         Response::build()
///             .sized_body(Cursor::new(format!("{}:{}", self.name, self.age)))
///             .raw_header("X-Person-Name", self.name)
///             .raw_header("X-Person-Age", self.age.to_string())
///             .header(ContentType::new("application", "x-person"))
///             .ok()
///     }
/// }
/// #
/// # #[get("/person")]
/// # fn person() -> Person { Person { name: "a".to_string(), age: 20 } }
/// # fn main() {  }
/// ```

/*
pub trait Responder {
    type Response: Future<Item=(Request, Response), Error=(Request, Status)> + Send + 'static;

    /// Returns `Ok` if a `Response` could be generated successfully. Otherwise,
    /// returns an `Err` with a failing `Status`.
    ///
    /// The `request` parameter is the `Request` that this `Responder` is
    /// responding to.
    ///
    /// When using Rocket's code generation, if an `Ok(Response)` is returned,
    /// the response will be written out to the client. If an `Err(Status)` is
    /// returned, the error catcher for the given status is retrieved and called
    /// to generate a final error response, which is then written out to the
    /// client.
    fn respond_to(self, request: Request) -> Self::Response;
}*/

mod sealed {
    use super::{Request, Status};
    use futures::{self, Future, IntoFuture, BoxFuture};
    use futures::future::FutureResult;

    pub trait Seal: Sized {}

    // TODO: really annoying to use R::Future: Send everywhere, RequestFuture should somehow imply that
    pub trait RequestFuture<I>: IntoFuture<Item=(Request, I), Error=(Request, Status)> + Seal + Send where Self::Future: Send { }
    impl<I, S: IntoFuture<Item=(Request, I), Error=(Request, Status)> + Send> RequestFuture<I> for S where S::Future: Send { }
    impl<I, S: IntoFuture<Item=(Request, I), Error=(Request, Status)> + Send> Seal for S where S::Future: Send { }
}
pub use self::sealed::RequestFuture;

pub trait Responder<F: RequestFuture<Self>>: Send + Sized where F::Future: Send {
    type Future: Future<Item=(Request, Response), Error=(Request, Status)>;

    fn respond_to(f: F) -> Self::Future;
}

/// Returns a response with Content-Type `text/plain` and a fixed-size body
/// containing the string `self`. Always returns `Ok`.
impl<F: RequestFuture<Self>> Responder<F> for &'static str where F::Future: Send + 'static {
    type Future = BoxFuture<(Request, Response), (Request, Status)>;

    fn respond_to(f: F) -> Self::Future {
        f.into_future().and_then(|(req, str_)| {
            Response::build()
            .header(ContentType::Plain)
            .sized_body(Cursor::new(str_))
            .ok()
            .map(|resp| (req, resp))
        }).boxed()
    }
}

/// Returns a response with Content-Type `text/plain` and a fixed-size body
/// containing the string `self`. Always returns `Ok`.
impl<F: RequestFuture<Self>> Responder<F> for String where F::Future: Send + 'static {
    type Future = BoxFuture<(Request, Response), (Request, Status)>;

    fn respond_to(f: F) -> Self::Future {
        f.into_future().and_then(|(req, str_)| {
            Response::build()
            .header(ContentType::Plain)
            .sized_body(Cursor::new(str_))
            .ok()
            .map(|resp| (req, resp))
        }).boxed()
    }
}

/// Returns a response with a sized body for the file. Always returns `Ok`.
impl<F: RequestFuture<Self>> Responder<F> for File where F::Future: Send + 'static {
    type Future = BoxFuture<(Request, Response), (Request, Status)>;

    fn respond_to(f: F) -> Self::Future {
        f.into_future().and_then(|(req, str_)| {
            Response::build().streamed_body(str_).ok()
                .map(|resp| (req, resp))
        }).boxed()
    }
}

/// Returns an empty, default `Response`. Always returns `Ok`.
impl<F: RequestFuture<Self>> Responder<F> for () where F::Future: Send + 'static {
    type Future = BoxFuture<(Request, Response), (Request, Status)>;

    fn respond_to(f: F) -> Self::Future {
        f.into_future().map(|(req, ())| (req, Response::new())).boxed()
    }
}

/// If `self` is `Some`, responds with the wrapped `Responder`. Otherwise prints
/// a warning message and returns an `Err` of `Status::NotFound`.
impl<F: RequestFuture<Self>, R: Responder<Result<(Request, R), (Request, Status)>>> Responder<F> for Option<R> 
    where F::Future: Send + 'static, R::Future: Send + 'static
{
    type Future = BoxFuture<(Request, Response), (Request, Status)>;

    fn respond_to(f: F) -> Self::Future {
        f.into_future().and_then(|(req, responder)| {
            match responder {
                Some(r) => {
                    Either::A(R::respond_to(Ok((req, r))))
                }
                None => {
                    warn_!("Response was `None`.");
                    Either::B(futures::future::err((req, Status::NotFound)))
                }
            }
        }).boxed()
    }
}

/// If `self` is `Ok`, responds with the wrapped `Responder`. Otherwise prints
/// an error message with the `Err` value returns an `Err` of
/// `Status::InternalServerError`.
impl<F: RequestFuture<Self>, R: Responder<Result<(Request, R), (Request, Status)>>, E: fmt::Debug + Send> Responder<F> for Result<R, E> 
    where F::Future: Send + 'static, R::Future: Send + 'static
{
    type Future = BoxFuture<(Request, Response), (Request, Status)>;

    default fn respond_to(f: F) -> Self::Future {
        f.into_future().and_then(|(req, responders)| {
            match responders {
                Ok(r) => Either::A(R::respond_to(Ok((req, r)))),
                Err(e) => {
                    error_!("Response was `Err`: {:?}.", e);
                    Either::B(futures::future::err((req, Status::InternalServerError)))
                }
            }
        }).boxed()
    }
}

/// Responds with the wrapped `Responder` in `self`, whether it is `Ok` or
/// `Err`.
impl<F, R, E> Responder<F> for Result<R, E> 
    where F: RequestFuture<Self>,
          F::Future: Send + 'static,
          R: Responder<Result<(Request, R), (Request, Status)>> + Sync + Send + 'static,
          R::Future: Send + 'static,
          E: Responder<Result<(Request, E), (Request, Status)>> + fmt::Debug + 'static,
          E::Future: Send + 'static
{
    fn respond_to(f: F) -> Self::Future
    {
        f.into_future().and_then(|(req, responders)| {
            match responders {
                Ok(responder) => Either::A(R::respond_to(Ok((req, responder)))),
                Err(responder) => Either::B(E::respond_to(Ok((req, responder)))),
            }
        }).boxed()
    }
}
