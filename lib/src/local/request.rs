use std::fmt;
use std::rc::Rc;
use std::mem::transmute;
use std::net::SocketAddr;
use std::ops::{Deref, DerefMut};

use futures::Future;

use {Rocket, Request, Response, Data};
use rocket::RocketRc;
use http::{Header, Cookie};

/// A structure representing a local request as created by [`Client`].
///
/// # Usage
///
/// A `LocalRequest` value is constructed via method constructors on [`Client`].
/// Headers can be added via the [`header`] builder method and the
/// [`add_header`] method. Cookies can be added via the [`cookie`] builder
/// method. The remote IP address can be set via the [`remote`] builder method.
/// The body of the request can be set via the [`body`] builder method or
/// [`set_body`] method.
///
/// ## Example
///
/// The following snippet uses the available builder methods to construct a
/// `POST` request to `/` with a JSON body:
///
/// ```rust
/// use rocket::local::Client;
/// use rocket::http::{ContentType, Cookie};
///
/// let client = Client::new(rocket::ignite()).expect("valid rocket");
/// let req = client.post("/")
///     .header(ContentType::JSON)
///     .remote("127.0.0.1:8000".parse().unwrap())
///     .cookie(Cookie::new("name", "value"))
///     .body(r#"{ "value": 42 }"#);
/// ```
///
/// # Dispatching
///
/// A `LocalRequest` can be dispatched in one of three ways:
///
///   1. [`dispatch`]
///
///     This method should always be preferred. The `LocalRequest` is consumed
///     and a response is returned.
///
///   2. [`cloned_dispatch`]
///
///     This method should be used when one `LocalRequest` will be dispatched
///     many times. This method clones the request and dispatches the clone, so
///     the request _is not_ consumed and can be reused.
///
///   3. [`mut_dispatch`]
///
///     This method should _only_ be used when either it is known that the
///     application will not modify the request, or it is desired to see
///     modifications to the request. No cloning occurs, and the request is not
///     consumed.
///
/// [`Client`]: /rocket/local/struct.Client.html
/// [`header`]: #method.header
/// [`add_header`]: #method.add_header
/// [`cookie`]: #method.cookie
/// [`remote`]: #method.remote
/// [`body`]: #method.body
/// [`set_body`]: #method.set_body
/// [`dispatch`]: #method.dispatch
/// [`mut_dispatch`]: #method.mut_dispatch
/// [`cloned_dispatch`]: #method.cloned_dispatch
pub struct LocalRequest {
    ptr: *mut Request,
    request: Rc<Request>,
    data: Vec<u8>
}

impl LocalRequest {
    #[inline(always)]
    pub(crate) fn new(request: Request) -> LocalRequest {
        let mut req = Rc::new(request);
        let ptr = Rc::get_mut(&mut req).unwrap() as *mut Request;
        LocalRequest { ptr: ptr, request: req, data: vec![] }
    }

    /// Retrieves the inner `Request` as seen by Rocket.
    ///
    /// # Example
    ///
    /// ```rust
    /// use rocket::local::Client;
    ///
    /// let client = Client::new(rocket::ignite()).expect("valid rocket");
    /// let req = client.get("/");
    /// let inner_req = req.inner();
    /// ```
    #[inline]
    pub fn inner(&self) -> &Request {
        &*self.request
    }

    #[inline]
    fn rocket(&self) -> &RocketRc {
        &self.request.state.rocket
    }

    #[inline(always)]
    fn request(&mut self) -> &mut Request {
        unsafe { &mut *self.ptr }
    }

    /// Add a header to this request.
    ///
    /// Any type that implements `Into<Header>` can be used here. Among others,
    /// this includes [`ContentType`] and [`Accept`].
    ///
    /// [`ContentType`]: /rocket/http/struct.ContentType.html
    /// [`Accept`]: /rocket/http/struct.Accept.html
    ///
    /// # Examples
    ///
    /// Add the Content-Type header:
    ///
    /// ```rust
    /// use rocket::local::Client;
    /// use rocket::http::ContentType;
    ///
    /// # #[allow(unused_variables)]
    /// let client = Client::new(rocket::ignite()).unwrap();
    /// let req = client.get("/").header(ContentType::JSON);
    /// ```
    #[inline]
    pub fn header<H: Into<Header<'static>>>(mut self, header: H) -> Self {
        self.request().add_header(header.into());
        self
    }

    /// Adds a header to this request without consuming `self`.
    ///
    /// # Examples
    ///
    /// Add the Content-Type header:
    ///
    /// ```rust
    /// use rocket::local::Client;
    /// use rocket::http::ContentType;
    ///
    /// let client = Client::new(rocket::ignite()).unwrap();
    /// let mut req = client.get("/");
    /// req.add_header(ContentType::JSON);
    /// ```
    #[inline]
    pub fn add_header<H: Into<Header<'static>>>(&mut self, header: H) {
        self.request().add_header(header.into());
    }

    /// Set the remote address of this request.
    ///
    /// # Examples
    ///
    /// Set the remote address to "8.8.8.8:80":
    ///
    /// ```rust
    /// use rocket::local::Client;
    ///
    /// let client = Client::new(rocket::ignite()).unwrap();
    /// let address = "8.8.8.8:80".parse().unwrap();
    /// let req = client.get("/").remote(address);
    /// ```
    #[inline]
    pub fn remote(mut self, address: SocketAddr) -> Self {
        self.request().set_remote(address);
        self
    }

    /// Add a cookie to this request.
    ///
    /// # Examples
    ///
    /// Add `user_id` cookie:
    ///
    /// ```rust
    /// use rocket::local::Client;
    /// use rocket::http::Cookie;
    ///
    /// let client = Client::new(rocket::ignite()).unwrap();
    /// # #[allow(unused_variables)]
    /// let req = client.get("/")
    ///     .cookie(Cookie::new("username", "sb"))
    ///     .cookie(Cookie::new("user_id", "12"));
    /// ```
    #[inline]
    pub fn cookie(self, cookie: Cookie<'static>) -> Self {
        self.request.cookies().add_original(cookie);
        self
    }

    // TODO: For CGI, we want to be able to set the body to be stdin without
    // actually reading everything into a vector. Can we allow that here while
    // keeping the simplicity? Looks like it would require us to reintroduce a
    // NetStream::Local(Box<Read>) or something like that.

    /// Set the body (data) of the request.
    ///
    /// # Examples
    ///
    /// Set the body to be a JSON structure; also sets the Content-Type.
    ///
    /// ```rust
    /// use rocket::local::Client;
    /// use rocket::http::ContentType;
    ///
    /// let client = Client::new(rocket::ignite()).unwrap();
    /// # #[allow(unused_variables)]
    /// let req = client.post("/")
    ///     .header(ContentType::JSON)
    ///     .body(r#"{ "key": "value", "array": [1, 2, 3], }"#);
    /// ```
    #[inline]
    pub fn body<S: AsRef<[u8]>>(mut self, body: S) -> Self {
        self.data = body.as_ref().into();
        self
    }

    /// Set the body (data) of the request without consuming `self`.
    ///
    /// # Examples
    ///
    /// Set the body to be a JSON structure; also sets the Content-Type.
    ///
    /// ```rust
    /// use rocket::local::Client;
    /// use rocket::http::ContentType;
    ///
    /// let client = Client::new(rocket::ignite()).unwrap();
    /// let mut req = client.post("/").header(ContentType::JSON);
    /// req.set_body(r#"{ "key": "value", "array": [1, 2, 3], }"#);
    /// ```
    #[inline]
    pub fn set_body<S: AsRef<[u8]>>(&mut self, body: S) {
        self.data = body.as_ref().into();
    }

    /// Dispatches the request, returning the response.
    ///
    /// This method consumes `self` and is the preferred mechanism for
    /// dispatching.
    ///
    /// # Example
    ///
    /// ```rust
    /// use rocket::local::Client;
    ///
    /// let client = Client::new(rocket::ignite()).unwrap();
    /// let response = client.get("/").dispatch();
    /// ```
    #[inline(always)]
    pub fn dispatch<'a>(mut self) -> impl Future<Item=LocalResponse, Error=()> + 'a {
        let rocket = self.rocket().clone();
        let req = Rc::try_unwrap(self.request).unwrap();
        rocket.dispatch(req, Data::local(self.data)).map(move |(req, response)| {
            LocalResponse {
                _request: Rc::new(req),
                response: response
            }
        }).map_err(|_| ())
    }

    /// Dispatches the request, returning the response.
    ///
    /// This method _does not_ consume `self`. Instead, it clones `self` and
    /// dispatches the clone. As such, `self` can be reused.
    ///
    /// # Example
    ///
    /// ```rust
    /// use rocket::local::Client;
    ///
    /// let client = Client::new(rocket::ignite()).unwrap();
    ///
    /// let req = client.get("/");
    /// let response_a = req.cloned_dispatch();
    /// let response_b = req.cloned_dispatch();
    /// ```
    #[inline(always)]
    pub fn cloned_dispatch<'a>(&self) -> impl Future<Item=LocalResponse, Error=()> + 'a {
        let cloned = (*self.request).clone();
        let mut req = LocalRequest::new(cloned);
        req.data = self.data.clone();
        req.dispatch()
    }
}

impl<'c> fmt::Debug for LocalRequest {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Debug::fmt(&self.request, f)
    }
}

/// A structure representing a response from dispatching a local request.
///
/// This structure is a thin wrapper around [`Response`]. It implements no
/// methods of its own; all functionality is exposed via the `Deref` and
/// `DerefMut` implementations with a target of `Response`. In other words, when
/// invoking methods, a `LocalResponse` can be treated exactly as if it were a
/// `Response`.
///
/// [`Response`]: /rocket/struct.Response.html
pub struct LocalResponse {
    _request: Rc<Request>,
    response: Response,
}

impl<'c> Deref for LocalResponse {
    type Target = Response;

    #[inline(always)]
    fn deref(&self) -> &Response {
        &self.response
    }
}

impl<'c> DerefMut for LocalResponse {
    #[inline(always)]
    fn deref_mut(&mut self) -> &mut Response {
        &mut self.response
    }
}

impl<'c> fmt::Debug for LocalResponse {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Debug::fmt(&self.response, f)
    }
}

// fn test() {
//     use local::Client;

//     let rocket = Rocket::ignite();
//     let res = {
//         let mut client = Client::new(rocket).unwrap();
//         client.get("/").dispatch()
//     };

//     // let client = Client::new(rocket).unwrap();
//     // let res1 = client.get("/").dispatch();
//     // let res2 = client.get("/").dispatch();
// }

// fn test() {
//     use local::Client;

//     let rocket = Rocket::ignite();
//     let res = {
//         Client::new(rocket).unwrap()
//             .get("/").dispatch();
//     };

//     // let client = Client::new(rocket).unwrap();
//     // let res1 = client.get("/").dispatch();
//     // let res2 = client.get("/").dispatch();
// }

// fn test() {
//     use local::Client;

//     let rocket = Rocket::ignite();
//     let client = Client::new(rocket).unwrap();

//     let res = {
//         let x = client.get("/").dispatch();
//         let y = client.get("/").dispatch();
//     };

//     let x = client;
// }

// fn test() {
//     use local::Client;

//     let rocket1 = Rocket::ignite();
//     let rocket2 = Rocket::ignite();

//     let client1 = Client::new(rocket1).unwrap();
//     let client2 = Client::new(rocket2).unwrap();

//     let res = {
//         let mut res1 = client1.get("/");
//         res1.set_client(&client2);
//         res1
//     };

//     drop(client1);
// }
