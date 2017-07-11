#![feature(plugin)]
#![plugin(rocket_codegen)]
#![feature(conservative_impl_trait)]

extern crate rocket;

use rocket::futures;
use rocket::Future;
use rocket::http::Status;

#[cfg(test)] mod tests;

#[get("/")]
fn hello() -> impl Future<Item=&'static str, Error=Status> {
    futures::future::ok("Hello, world!")
}

fn main() {
    rocket::ignite().mount("/", routes![hello]).launch();
}
