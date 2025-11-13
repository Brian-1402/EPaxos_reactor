mod common;
mod reader;
mod server;
mod writer;

use crate::reader::reader as reader_behaviour;
use crate::server::server as server_behaviour;
use crate::writer::writer as writer_behaviour;
use reactor_actor::RuntimeCtx;
use std::collections::HashMap;

pub use reactor_actor::{actor, setup_shared_logger_ref};

pub const SLEEP_MS: u64 = 100;

lazy_static::lazy_static! {
    static ref RUNTIME: tokio::runtime::Runtime = tokio::runtime::Runtime::new().unwrap();
}

#[actor]
fn reader(ctx: RuntimeCtx, mut payload: HashMap<String, serde_json::Value>) {
    let server = payload.remove("server")
        .expect("server field missing")
        .as_str()
        .expect("server must be a string")
        .to_string();
    RUNTIME.spawn(reader_behaviour(ctx, server));
}

#[actor]
fn server(ctx: RuntimeCtx, _payload: HashMap<String, serde_json::Value>) {
    RUNTIME.spawn(server_behaviour(ctx));
}

#[actor]
fn writer(ctx: RuntimeCtx, mut payload: HashMap<String, serde_json::Value>) {
    let server = payload.remove("server")
        .expect("server field missing")
        .as_str()
        .expect("server must be a string")
        .to_string();
    RUNTIME.spawn(writer_behaviour(ctx, server));
}