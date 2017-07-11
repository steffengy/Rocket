use std::collections::HashMap;
use std::str::from_utf8_unchecked;
use std::cmp::min;
use std::net::SocketAddr;
use std::net::ToSocketAddrs;
use std::io;
use std::mem;
use std::ops::Deref;
use std::sync::Arc;
use futures::{self, BoxFuture, Async, Future, Stream, Poll};
use futures::future::{Either, IntoFuture, FutureResult};
use futures::stream;
use futures_cpupool::{CpuFuture, CpuPool};
use hyper::server::NewService;
use tokio_core::reactor::Core;
use tokio_core::net::TcpListener;
use yansi::Paint;
use state::Container;

#[cfg(feature = "tls")] use rustls;
#[cfg(feature = "tls")] use tokio_rustls::{self, ServerConfigExt};
use {logger, handler};
use config::{self, Config, LoggedValue};
use request::{Request, FormItems};
use data::Data;
use response::{Body, Response};
use router::{Router, Route};
use catcher::{self, Catcher};
use outcome::Outcome;
use error::{Error, LaunchError, LaunchErrorKind};
use fairing::{Fairing, Fairings};

use http::{Method, Status, Header};
use http::hyper::{self, header};
use http::uri::URI;

/// The main `Rocket` type: used to mount routes and catchers and launch the
/// application.
pub struct Rocket {
    pub(crate) config: Config,
    router: Router,
    default_catchers: HashMap<u16, Catcher>,
    catchers: HashMap<u16, Catcher>,
    pub(crate) state: Container,
    fairings: Fairings,
}

#[derive(Clone)]
struct RocketService {
    rocket: RocketRc,
    pool: Arc<CpuPool>,
}

// TODO: I do not like RocketRC, maybe merge with Rocket
#[derive(Clone)]
pub struct RocketRc(pub(crate) Arc<Rocket>);

impl Deref for RocketRc {
    type Target = Arc<Rocket>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl RocketRc {
    // TODO: investigate if replacing BoxFuture -> enum is faster/better
    //       and a bit too much cloning & boxing for my taste (and too many Arcs in public interfaces generally)
    #[inline]
    pub(crate) fn dispatch(self,
                           mut request: Request,
                           data: Data) -> BoxFuture<(Request, Response), Request> {
        info!("{}:", request);

        // Do a bit of preprocessing before routing; run the attached fairings.
        self.preprocess_request(&mut request, &data);
        self.fairings.handle_request(&mut request, &data);

        // Route the request to get a response.
        self.route(request, data)
            .and_then(move |(mut request, outcome)| {
                match outcome {
                    Outcome::Success(mut response) => {
                        // A user's route responded! Set the cookies.
                        for cookie in request.cookies().delta() {
                            response.adjoin_header(cookie);
                        }

                        futures::future::ok((request, response)).boxed()
                    }
                    Outcome::Forward(data) => {
                        // There was no matching route.
                        if request.method() == Method::Head {
                            info_!("Autohandling {} request.", Paint::white("HEAD"));
                            request.set_method(Method::Get);
                            self.clone().dispatch(request, data).map(|(request, mut response)| {
                                response.strip_body();
                                (request, response)
                            }).boxed()
                        } else {
                            self.handle_error(Status::NotFound, request).boxed()
                        }
                    }
                    Outcome::Failure(status) => self.handle_error(status, request).boxed(),
                }.map(|(req, resp)| (self, req, resp))
            })
            .map(move |(rocket, request, mut response)| {
                // Add the 'rocket' server header to the response and run fairings.
                // TODO: If removing Hyper, write out `Date` header too.
                response.set_header(Header::new("Server", "Rocket"));
                rocket.fairings.handle_response(&request, &mut response);
                (request, response)
            })
            .boxed()
    }

    // Finds the error catcher for the status `status` and executes it fo the
    // given request `req`. If a user has registere a catcher for `status`, the
    // catcher is called. If the catcher fails to return a good response, the
    // 500 catcher is executed. if there is no registered catcher for `status`,
    // the default catcher is used.
    fn handle_error(&self, status: Status, req: Request) -> impl Future<Item=(Request, Response), Error=Request> + 'static {
        warn_!("Responding with {} catcher.", Paint::red(&status));

        let rocket = self.clone();

        // Try to get the active catcher but fallback to user's 500 catcher.
        let error = Error::NoRoute;
        let future = {
            let catcher = rocket.catchers.get(&status.code).unwrap_or_else(|| {
                error_!("No catcher found for {}. Using 500 catcher.", status);
                rocket.catchers.get(&500).expect("500 catcher.")
            });
            catcher.handle(error, req)
        };

        // Dispatch to the user's catcher. If it fails, use the default 500.
        future.or_else(move |(req, err_status)| {
            error_!("Catcher failed with status: {}!", err_status);
            warn_!("Using default 500 error catcher.");
            let default = rocket.default_catchers.get(&500).expect("Default 500");
            default.handle(error, req)
        }).map_err(|_| panic!("Default 500 response."))
    }
}

#[doc(hidden)]
impl hyper::NewService for RocketService {
    type Request = hyper::Request;
    type Response = hyper::Response;
    type Error = hyper::Error;
    type Instance = Self;

    fn new_service(&self) -> Result<Self::Instance, io::Error> {
        Ok(self.clone())
    }
}

#[doc(hidden)]
impl hyper::Service for RocketService {
    type Request = hyper::Request;
    type Response = hyper::Response;
    type Error = hyper::Error;
    type Future = CpuFuture<Self::Response, Self::Error>;

    // This function tries to hide all of the Hyper-ness from Rocket. It
    // essentially converts Hyper types into Rocket types, then calls the
    // `dispatch` function, which knows nothing about Hyper. Because responding
    // depends on the `HyperResponse` type, this function does the actual
    // response processing.
    fn call(&self, hyp_req: hyper::Request) -> Self::Future {
        let rocket = self.rocket.clone();
        self.pool.spawn_fn(move || {
            // Get all of the information from Hyper.
            let h_addr = hyp_req.remote_addr().unwrap();
            let (h_method, h_uri, _, h_headers, h_body) = hyp_req.deconstruct();

            // Convert the Hyper request into a Rocket request.
            let req_res = Request::from_hyp(rocket.clone(), h_method, h_headers, h_uri, h_addr);
            let future = match req_res {
                Ok(req) => {
                    // Retrieve the data from the hyper body.
                    let data = Data::from_hyp(h_body);
                    // Dispatch the request to get a response, then write that response out.
                    Either::A(rocket.clone().dispatch(req, data))
                }
                Err(e) => {
                    error!("Bad incoming request: {}", e);
                    let dummy = Request::new(rocket.clone(), Method::Get, URI::new("<unknown>"));
                    let r = rocket.handle_error(Status::InternalServerError, dummy);
                    Either::B(r)
                }
            };
            future
                .map(|(_, resp)| resp)
                .map_err(|_| io::Error::new(io::ErrorKind::Other, "internal()").into())
                .and_then(move |resp| rocket.issue_response(resp))
        })
    }
}

impl Rocket {
    #[inline]
    fn issue_response(&self, response: Response) -> FutureResult<hyper::Response, hyper::Error> {
        // TODO: move info_ and error_?
        match self.write_response(response) {
            Ok(ret) => {
                info_!("{}", Paint::green("Response succeeded."));
                futures::future::ok(ret)
            }
            Err(e) => {
                error_!("Failed to write response: {:?}.", e);
                futures::future::err(e.into())
            }
        }
    }

    #[inline]
    fn write_response(&self, mut response: Response) -> hyper::Result<hyper::Response>
    {
        let mut hyp_res = hyper::Response::new();
        hyp_res.set_status(hyper::StatusCode::try_from(
            response.status().code).map_err(|_| hyper::Error::Status)?);

        for header in response.headers().iter() {
            // FIXME: Using hyper here requires two allocations.
            let name = header.name.into_string();
            let value = Vec::from(header.value.as_bytes());
            hyp_res.headers_mut().append_raw(name, value);
        }

        match response.take_body() {
            None => {
                hyp_res.headers_mut().set(header::ContentLength(0));
            }
            Some(Body::Sized(mut body, size)) => {
                // TODO: Figure out how to get rid of the extra copy (own Body/wrapping?)
                hyp_res.headers_mut().set(header::ContentLength(size));
                let mut buf = Vec::with_capacity(size as usize);
                body.read_to_end(&mut buf)?;
                hyp_res.set_body(hyper::Body::from(buf).boxed());
            }
            Some(Body::Chunked(body, chunk_size)) => {
                // This _might_ happen on a 32-bit machine!
                if chunk_size > (usize::max_value() as u64) {
                    let msg = "chunk size exceeds limits of usize type";
                    return Err(io::Error::new(io::ErrorKind::Other, msg).into());
                }
                let chunk_size = chunk_size as usize;
                // sync-async hybrid for now that will block for each chunk (but not the entire data)
                struct Chunked<R: io::Read + Send> {
                    pos: usize,
                    buf: Vec<u8>,
                    reader: R,
                    chunk_size: usize,
                }

                impl<R: io::Read + Send> Stream for Chunked<R> {
                    type Item = hyper::Chunk;
                    type Error = hyper::Error;

                    fn poll(&mut self) -> Poll<Option<Self::Item>, Self::Error> {
                        // we only do synchronous reads here, until Rockets body is rewritten to async
                        loop {
                            let amount = self.reader.read(&mut self.buf[self.pos..])?;
                            if amount == 0 {
                                return Ok(Async::Ready(None));
                            }
                            self.pos += amount;
                            if self.pos == self.chunk_size {
                                self.pos = 0;
                                let chunk = mem::replace(&mut self.buf, vec![0u8; self.chunk_size]).into();
                                return Ok(Async::Ready(Some(chunk)));
                            }
                        }
                    }
                }
                let body = Chunked { pos: 0, buf: vec![0u8; chunk_size], reader: body, chunk_size };
                hyp_res.set_body(body.boxed());
            }
        }

        Ok(hyp_res)
    }

    /// Preprocess the request for Rocket things. Currently, this means:
    ///
    ///   * Rewriting the method in the request if _method form field exists.
    ///   * Rewriting the remote IP if the 'X-Real-IP' header is set.
    ///
    /// Keep this in-sync with derive_form when preprocessing form fields.
    fn preprocess_request(&self, req: &mut Request, data: &Data) {
        // Rewrite the remote IP address. The request must already have an
        // address associated with it to do this since we need to know the port.
        if let Some(current) = req.remote() {
            let ip = req.headers()
                .get_one("X-Real-IP")
                .and_then(|ip_str| ip_str.parse().map_err(|_| {
                    warn_!("The 'X-Real-IP' header is malformed: {}", ip_str)
                }).ok());

            if let Some(ip) = ip {
                req.set_remote(SocketAddr::new(ip, current.port()));
            }
        }

        // Check if this is a form and if the form contains the special _method
        // field which we use to reinterpret the request's method.
        let data_len = data.peek().len();
        let (min_len, max_len) = ("_method=get".len(), "_method=delete".len());
        let is_form = req.content_type().map_or(false, |ct| ct.is_form());
        if is_form && req.method() == Method::Post && data_len >= min_len {
            // We're only using this for comparison and throwing it away
            // afterwards, so it doesn't matter if we have invalid UTF8.
            let form = unsafe {
                from_utf8_unchecked(&data.peek()[..min(data_len, max_len)])
            };

            if let Some((key, value)) = FormItems::from(form).next() {
                if key == "_method" {
                    if let Ok(method) = value.parse() {
                        req.set_method(method);
                    }
                }
            }
        }
    }

    /// Tries to find a `Responder` for a given `request`. It does this by
    /// routing the request and calling the handler for each matching route
    /// until one of the handlers returns success or failure, or there are no
    /// additional routes to try (forward). The corresponding outcome for each
    /// condition is returned.
    //
    // TODO: We _should_ be able to take an `&mut` here and mutate the request
    // at any pointer _before_ we pass it to a handler as long as we drop the
    // outcome. That should be safe. Since no mutable borrow can be held
    // (ensuring `handler` takes an immutable borrow), any caller to `route`
    // should be able to supply an `&mut` and retain an `&` after the call.
    #[inline]
    pub(crate) fn route(&self,
                            req: Request,
                            mut data: Data) -> impl Future<Item=(Request, handler::Outcome), Error=Request> + 'static {

        // Go through the list of matching routes until we fail or succeed.

        // TODO: this should really be optimized, we collect all matching routes AND clone the ARC
        let routes: Vec<_> = self.router.route(&req).into_iter().cloned().collect();

        struct RoutingFuture {
            req: Option<Request>,
            data: Option<Data>,
            pos: usize,
            routes: Vec<Arc<Route>>,
            pending_outcome: Option<BoxFuture<(Request, handler::Outcome), Request>>,
        }

        impl Future for RoutingFuture {
            type Item = (Request, handler::Outcome);
            type Error = Request;

            fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
                loop {
                    if let Some(ref mut pending_outcome) = self.pending_outcome {
                        let (request, outcome) = try_ready!(pending_outcome.poll());

                        // Check if the request processing completed or if the request needs
                        // to be forwarded. If it does, continue the loop to try again.
                        info_!("{} {}", Paint::white("Outcome:"), outcome);
                        match outcome {
                            o @ Outcome::Success(_) | o @ Outcome::Failure(_) => {
                                // TODO: prevent further polling
                                return Ok(Async::Ready((request, o)));
                            }
                            Outcome::Forward(unused_data) => {
                                self.data = Some(unused_data);
                                self.req = Some(request);
                            }
                        }
                    }
                    
                    let request = self.req.take().unwrap();
                    let data = self.data.take().unwrap();
                    if self.pos < self.routes.len() {
                        let route = self.routes[self.pos].clone();
                        self.pos += 1;
                        // Retrieve and set the requests parameters.
                        info_!("Matched: {}", route);
                        let handler = route.handler;
                        request.set_route(route);

                        // Dispatch the request to the handler.
                        self.pending_outcome = Some(handler(request, data));
                    } else {
                        error_!("No matching routes for {}.", request);
                        return Ok(Async::Ready((request, Outcome::Forward(data))));
                    }
                }
            }
        }

        RoutingFuture {
            req: Some(req),
            data: Some(data),
            pos: 0,
            routes,
            pending_outcome: None,
        }
    }

    /// Create a new `Rocket` application using the configuration information in
    /// `Rocket.toml`. If the file does not exist or if there is an I/O error
    /// reading the file, the defaults are used. See the
    /// [config](/rocket/config/index.html) documentation for more information
    /// on defaults.
    ///
    /// This method is typically called through the `rocket::ignite` alias.
    ///
    /// # Panics
    ///
    /// If there is an error parsing the `Rocket.toml` file, this functions
    /// prints a nice error message and then exits the process.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # {
    /// rocket::ignite()
    /// # };
    /// ```
    #[inline]
    pub fn ignite() -> Rocket {
        // Note: init() will exit the process under config errors.
        Rocket::configured(config::init(), true)
    }

    /// Creates a new `Rocket` application using the supplied custom
    /// configuration information. The `Rocket.toml` file, if present, is
    /// ignored. Any environment variables setting config parameters are
    /// ignored. If `log` is `true`, logging is enabled.
    ///
    /// This method is typically called through the `rocket::custom` alias.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use rocket::config::{Config, Environment};
    /// # use rocket::config::ConfigError;
    ///
    /// # #[allow(dead_code)]
    /// # fn try_config() -> Result<(), ConfigError> {
    /// let config = Config::build(Environment::Staging)
    ///     .address("1.2.3.4")
    ///     .port(9234)
    ///     .finalize()?;
    ///
    /// # #[allow(unused_variables)]
    /// let app = rocket::custom(config, false);
    /// # Ok(())
    /// # }
    /// ```
    #[inline]
    pub fn custom(config: Config, log: bool) -> Rocket {
        Rocket::configured(config, log)
    }

    #[inline]
    fn configured(config: Config, log: bool) -> Rocket {
        if log {
            logger::try_init(config.log_level, false);
        }

        info!("🔧  Configured for {}.", config.environment);
        info_!("address: {}", Paint::white(&config.address));
        info_!("port: {}", Paint::white(&config.port));
        info_!("log: {}", Paint::white(config.log_level));
        info_!("workers: {}", Paint::white(config.workers));
        info_!("secret key: {}", Paint::white(&config.secret_key));
        info_!("limits: {}", Paint::white(&config.limits));

        let tls_configured = config.tls.is_some();
        if tls_configured && cfg!(feature = "tls") {
            info_!("tls: {}", Paint::white("enabled"));
        } else if tls_configured {
            error_!("tls: {}", Paint::white("disabled"));
            error_!("tls is configured, but the tls feature is disabled");
        } else {
            info_!("tls: {}", Paint::white("disabled"));
        }

        if config.secret_key.is_generated() && config.environment.is_prod() {
            warn!("environment is 'production', but no `secret_key` is configured");
        }

        for (name, value) in config.extras() {
            info_!("{} {}: {}",
                   Paint::yellow("[extra]"), name, Paint::white(LoggedValue(value)));
        }

        Rocket {
            config: config,
            router: Router::new(),
            default_catchers: catcher::defaults::get(),
            catchers: catcher::defaults::get(),
            state: Container::new(),
            fairings: Fairings::new()
        }
    }

    /// Mounts all of the routes in the supplied vector at the given `base`
    /// path. Mounting a route with path `path` at path `base` makes the route
    /// available at `base/path`.
    ///
    /// # Panics
    ///
    /// The `base` mount point must be a static path. That is, the mount point
    /// must _not_ contain dynamic path parameters: `<param>`.
    ///
    /// # Examples
    ///
    /// Use the `routes!` macro to mount routes created using the code
    /// generation facilities. Requests to the `/hello/world` URI will be
    /// dispatched to the `hi` route.
    ///
    /// ```rust
    /// # #![feature(plugin)]
    /// # #![plugin(rocket_codegen)]
    /// # extern crate rocket;
    /// #
    /// #[get("/world")]
    /// fn hi() -> &'static str {
    ///     "Hello!"
    /// }
    ///
    /// fn main() {
    /// # if false { // We don't actually want to launch the server in an example.
    ///     rocket::ignite().mount("/hello", routes![hi])
    /// #       .launch();
    /// # }
    /// }
    /// ```
    ///
    /// Manually create a route named `hi` at path `"/world"` mounted at base
    /// `"/hello"`. Requests to the `/hello/world` URI will be dispatched to the
    /// `hi` route.
    ///
    /// ```rust
    /// use rocket::{Request, Route, Data};
    /// use rocket::handler::Outcome;
    /// use rocket::http::Method::*;
    ///
    /// fn hi(req: &Request, _: Data) -> Outcome<'static> {
    ///     Outcome::from(req, "Hello!")
    /// }
    ///
    /// # if false { // We don't actually want to launch the server in an example.
    /// rocket::ignite().mount("/hello", vec![Route::new(Get, "/world", hi)])
    /// #     .launch();
    /// # }
    /// ```
    #[inline]
    pub fn mount(mut self, base: &str, routes: Vec<Route>) -> Self {
        info!("🛰  {} '{}':", Paint::purple("Mounting"), base);

        if base.contains('<') {
            error_!("Bad mount point: '{}'.", base);
            error_!("Mount points must be static paths!");
            panic!("Bad mount point.")
        }

        for mut route in routes {
            let uri = URI::new(format!("{}/{}", base, route.uri));

            route.set_base(base);
            route.set_uri(uri.to_string());

            info_!("{}", route);
            self.router.add(route);
        }

        self
    }

    /// Registers all of the catchers in the supplied vector.
    ///
    /// # Examples
    ///
    /// ```rust
    /// #![feature(plugin)]
    /// #![plugin(rocket_codegen)]
    ///
    /// extern crate rocket;
    ///
    /// use rocket::Request;
    ///
    /// #[error(500)]
    /// fn internal_error() -> &'static str {
    ///     "Whoops! Looks like we messed up."
    /// }
    ///
    /// #[error(400)]
    /// fn not_found(req: &Request) -> String {
    ///     format!("I couldn't find '{}'. Try something else?", req.uri())
    /// }
    ///
    /// fn main() {
    /// # if false { // We don't actually want to launch the server in an example.
    ///     rocket::ignite().catch(errors![internal_error, not_found])
    /// #       .launch();
    /// # }
    /// }
    /// ```
    #[inline]
    pub fn catch(mut self, catchers: Vec<Catcher>) -> Self {
        info!("👾  {}:", Paint::purple("Catchers"));
        for c in catchers {
            if self.catchers.get(&c.code).map_or(false, |e| !e.is_default()) {
                let msg = "(warning: duplicate catcher!)";
                info_!("{} {}", c, Paint::yellow(msg));
            } else {
                info_!("{}", c);
            }

            self.catchers.insert(c.code, c);
        }

        self
    }

    /// Add `state` to the state managed by this instance of Rocket.
    ///
    /// This method can be called any number of times as long as each call
    /// refers to a different `T`.
    ///
    /// Managed state can be retrieved by any request handler via the
    /// [State](/rocket/struct.State.html) request guard. In particular, if a
    /// value of type `T` is managed by Rocket, adding `State<T>` to the list of
    /// arguments in a request handler instructs Rocket to retrieve the managed
    /// value.
    ///
    /// # Panics
    ///
    /// Panics if state of type `T` is already being managed.
    ///
    /// # Example
    ///
    /// ```rust
    /// # #![feature(plugin)]
    /// # #![plugin(rocket_codegen)]
    /// # extern crate rocket;
    /// use rocket::State;
    ///
    /// struct MyValue(usize);
    ///
    /// #[get("/")]
    /// fn index(state: State<MyValue>) -> String {
    ///     format!("The stateful value is: {}", state.0)
    /// }
    ///
    /// fn main() {
    /// # if false { // We don't actually want to launch the server in an example.
    ///     rocket::ignite()
    ///         .mount("/", routes![index])
    ///         .manage(MyValue(10))
    ///         .launch();
    /// # }
    /// }
    /// ```
    #[inline]
    pub fn manage<T: Send + Sync + 'static>(self, state: T) -> Self {
        if !self.state.set::<T>(state) {
            error!("State for this type is already being managed!");
            panic!("Aborting due to duplicately managed state.");
        }

        self
    }

    /// Attaches a fairing to this instance of Rocket.
    ///
    /// # Example
    ///
    /// ```rust
    /// # #![feature(plugin)]
    /// # #![plugin(rocket_codegen)]
    /// # extern crate rocket;
    /// use rocket::Rocket;
    /// use rocket::fairing::AdHoc;
    ///
    /// fn main() {
    /// # if false { // We don't actually want to launch the server in an example.
    ///     rocket::ignite()
    ///         .attach(AdHoc::on_launch(|_| println!("Rocket is launching!")))
    ///         .launch();
    /// # }
    /// }
    /// ```
    #[inline]
    pub fn attach<F: Fairing>(mut self, fairing: F) -> Self {
        // Attach the fairings, which requires us to move `self`.
        let mut fairings = mem::replace(&mut self.fairings, Fairings::new());
        self = fairings.attach(Box::new(fairing), self);

        // Make sure we keep the fairings around!
        self.fairings = fairings;
        self
    }

    pub(crate) fn prelaunch_check(&self) -> Option<LaunchError> {
        if self.router.has_collisions() {
            Some(LaunchError::from(LaunchErrorKind::Collision))
        } else if self.fairings.had_failure() {
            Some(LaunchError::from(LaunchErrorKind::FailedFairing))
        } else {
            None
        }
    }

    /// Starts the application server and begins listening for and dispatching
    /// requests to mounted routes and catchers. Unless there is an error, this
    /// function does not return and blocks until program termination.
    ///
    /// # Error
    ///
    /// If there is a problem starting the application, a [`LaunchError`] is
    /// returned. Note that a value of type `LaunchError` panics if dropped
    /// without first being inspected. See the [`LaunchError`] documentation for
    /// more information.
    ///
    /// [`LaunchError`]: /rocket/error/struct.LaunchError.html
    ///
    /// # Example
    ///
    /// ```rust
    /// # if false {
    /// rocket::ignite().launch();
    /// # }
    /// ```
    pub fn launch(mut self) -> LaunchError {
        if let Some(error) = self.prelaunch_check() {
            return error;
        }

        self.fairings.pretty_print_counts();

        let full_addr = format!("{}:{}", self.config.address, self.config.port);

        let pool = Arc::new(CpuPool::new(self.config.workers as usize));
        let rocket = RocketRc(Arc::new(self));
        let srv = RocketService {
            rocket: rocket.clone(),
            pool
        };

        let bind_addrs = match full_addr.to_socket_addrs() {
            Ok(x) => x,
            Err(e) => return LaunchError::from(e),
        };

        for bind_addr in bind_addrs {
            let mut reactor = match Core::new() {
                Ok(x) => x,
                Err(err) => return LaunchError::from(err),
            };
            let handle = reactor.handle();
            let listener = match TcpListener::bind(&bind_addr, &handle) {
                Ok(x) => x,
                Err(e) => return LaunchError::from(e),
            };
            
            // Determine the address and port we actually binded to.
            self.config.port = match listener.local_addr() {
                Ok(server_addr) => server_addr.port(),
                Err(e) => return LaunchError::from(e),
            };

            // Run the launch fairings. (TODO: do we really need the ARC-rocket here?)
            rocket.0.fairings.handle_launch(&rocket.0);

            let tls_config = match rocket.0.config.tls.clone() {
                #[cfg(feature = "tls")]
                Some(tls) => {
                    let mut cfg = rustls::ServerConfig::new();
                    let cache = rustls::ServerSessionMemoryCache::new(1024);
                    cfg.set_persistence(cache);
                    cfg.ticketer = rustls::Ticketer::new();
                    cfg.set_single_cert(tls.certs, tls.key);
                    Some(Arc::new(cfg))
                },
                _ => None
            };
            // a little hack to infer the type when tls is disabled
            #[cfg(not(feature = "tls"))] {tls_config as Option<bool>};
            let proto = tls_config.as_ref().map(|_| "https").unwrap_or("http");

            launch_info!("🚀  {} {}{}",
                Paint::white("Rocket has launched from"),
                Paint::white(proto).bold(),
                Paint::white(&full_addr).bold());

            macro_rules! serve {
                (inner, $listener:ident, $continue:expr) => ({
                    let fut = listener.incoming().for_each($continue);
                    reactor.run(fut)
                });
                ($tls_config:expr, $listener:ident, $continue:expr) => ({
                    match $tls_config.clone() {
                        #[cfg(feature = "tls")]
                        Some(cfg) => serve!(inner, $listener, |(socket, addr)| {
                            cfg.accept_async(socket).map(move |x| (x, addr)).and_then($continue)
                        }),
                        _ => serve!(inner, $listener, |(socket, addr)| {
                            futures::future::ok((socket, addr)).and_then($continue)
                        }),
                    }
                });
            }

            let ret = serve!(tls_config, listener, |(stream, addr)| {
                srv.new_service().into_future().map(move |x| (x, addr)).and_then(|(service, addr)| {
                    hyper::Http::new().bind_connection(&handle, stream, addr, service);
                    Ok(())
                })
            });
            if let Err(e) = ret {
                return LaunchError::from(e);
            }
            
            // we could support binding to multiple IPs (e.g. localhost:8080)
            // but we do not support that (yet? TODO?)
            break;
        }

        unreachable!("the event loop should block")
    }

    /// Returns an iterator over all of the routes mounted on this instance of
    /// Rocket.
    ///
    /// # Example
    ///
    /// ```rust
    /// # #![feature(plugin)]
    /// # #![plugin(rocket_codegen)]
    /// # extern crate rocket;
    /// use rocket::Rocket;
    /// use rocket::fairing::AdHoc;
    ///
    /// #[get("/hello")]
    /// fn hello() -> &'static str {
    ///     "Hello, world!"
    /// }
    ///
    /// fn main() {
    ///     let rocket = rocket::ignite()
    ///         .mount("/", routes![hello])
    ///         .mount("/hi", routes![hello]);
    ///
    ///     for route in rocket.routes() {
    ///         match route.base() {
    ///             "/" => assert_eq!(route.uri.path(), "/hello"),
    ///             "/hi" => assert_eq!(route.uri.path(), "/hi/hello"),
    ///             _ => unreachable!("only /hello, /hi/hello are expected")
    ///         }
    ///     }
    ///
    ///     assert_eq!(rocket.routes().count(), 2);
    /// }
    /// ```
    #[inline(always)]
    pub fn routes<'a>(&'a self) -> impl Iterator<Item=&'a Arc<Route>> + 'a {
        self.router.routes()
    }

    /// Returns the active configuration.
    ///
    /// # Example
    ///
    /// ```rust
    /// # #![feature(plugin)]
    /// # #![plugin(rocket_codegen)]
    /// # extern crate rocket;
    /// use rocket::Rocket;
    /// use rocket::fairing::AdHoc;
    ///
    /// fn main() {
    /// # if false { // We don't actually want to launch the server in an example.
    ///     rocket::ignite()
    ///         .attach(AdHoc::on_launch(|rocket| {
    ///             println!("Rocket launch config: {:?}", rocket.config());
    ///         }))
    ///         .launch();
    /// # }
    /// }
    /// ```
    #[inline(always)]
    pub fn config(&self) -> &Config {
        &self.config
    }
}
