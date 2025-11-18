mod common;
mod reader;
// mod ss;
mod writer;

mod epaxos;

use crate::reader::reader as reader_behaviour;
// use crate::ss::server as ss_behaviour;
use crate::epaxos::server as epaxos_behaviour;
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
    let server = payload
        .remove("server")
        .expect("server field missing")
        .as_str()
        .expect("server must be a string")
        .to_string();
    RUNTIME.spawn(reader_behaviour(ctx, server));
}

// #[actor]
// fn ss(ctx: RuntimeCtx, _payload: HashMap<String, serde_json::Value>) {
//     RUNTIME.spawn(ss_behaviour(ctx));
// }

#[actor]
fn epaxos_server(ctx: RuntimeCtx, mut payload: HashMap<String, serde_json::Value>) {
    let replica_list: Vec<String> = payload
        .remove("replica_list")
        .expect("replica_list field missing")
        .as_array()
        .expect("replica_list must be an array")
        .iter()
        .map(|v| {
            v.as_str()
                .expect("replica name must be a string")
                .to_string()
        })
        .collect::<Vec<String>>();
    RUNTIME.spawn(epaxos_behaviour(ctx, replica_list));
}

#[actor]
fn writer(ctx: RuntimeCtx, mut payload: HashMap<String, serde_json::Value>) {
    let server = payload
        .remove("server")
        .expect("server field missing")
        .as_str()
        .expect("server must be a string")
        .to_string();
    RUNTIME.spawn(writer_behaviour(ctx, server));
}
