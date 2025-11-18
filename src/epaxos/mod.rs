use crate::common::{Command, EMsg, Instance, Variable};
use reactor_actor::codec::BincodeCodec;
use reactor_actor::{BehaviourBuilder, RouteTo, RuntimeCtx, SendErrAction};

use std::collections::{HashMap, HashSet};
// use tracing::info;
mod handlers;
mod helpers;

// //////////////////////////////////////////////////////////////////////////////
//                                  Processor
// //////////////////////////////////////////////////////////////////////////////
enum CmdStatus {
    PreAccepted,
    Accepted,
    Committed,
}

struct CmdEntry {
    cmd: Command,

    /// Sequence number, used to break dependency cycles.
    seq: u64,

    /// Dependencies on other (replica, instance) pairs.
    deps: HashSet<Instance>, // Can be ordered set.
    status: CmdStatus,
}

#[allow(dead_code)]
struct CmdMetadata {
    client_id: String,
    msg_id: String,
}

struct Processor {
    #[allow(dead_code)]
    data: HashMap<Variable, String>,
    // cmds: HashMap<String, Vec<CmdInstance>>,
    cmds: HashMap<String, Vec<Option<CmdEntry>>>,

    // instance_num: u64,
    instance_num: usize,
    quorum_ctr: Vec<u32>, // Counter for PreAcceptOk messages, // Indexed by instance number
    #[allow(dead_code)]
    app_meta: Vec<CmdMetadata>, // Indexed by instance number

    replica_list: Vec<String>,
    replica_name: String, // Myself
}

impl reactor_actor::ActorProcess for Processor {
    type IMsg = EMsg;
    type OMsg = EMsg;

    fn process(&mut self, input: Self::IMsg) -> Vec<Self::OMsg> {
        match input {
            EMsg::ClientRequest(msg) => self.client_request_handler(msg),

            EMsg::PreAccept(msg) => self.pre_accept_handler(msg),
            EMsg::PreAcceptOk(msg) => self.pre_accept_ok_handler(msg),
            EMsg::Commit(msg) => self.commit_handler(msg),
            EMsg::Accept(msg) => self.accept_handler(msg),
            EMsg::AcceptOk(msg) => self.accept_ok_handler(msg),
            _ => {
                panic!("Server got an unexpected message")
            }
        }
    }
}

impl Processor {
    fn new(replica_list: Vec<String>, replica_name: String) -> Self {
        // initialize cmds for each replica
        let mut cmds: HashMap<String, Vec<Option<CmdEntry>>> = HashMap::new();
        for replica in &replica_list {
            cmds.insert(replica.clone(), vec![]);
        }
        Processor {
            data: HashMap::new(),
            cmds,
            instance_num: 0,
            quorum_ctr: vec![],
            app_meta: vec![],
            replica_list,
            replica_name,
        }
    }
}

struct Sender {
    replica_name: String,
    replica_list: Vec<String>,
}

impl reactor_actor::ActorSend for Sender {
    type OMsg = EMsg;

    async fn before_send<'a>(&'a mut self, output: &Self::OMsg) -> RouteTo<'a> {
        match &output {
            EMsg::ClientResponse(response) => {
                // Use client_id to route the message to the correct client
                let client_id = &response.client_id; // Assuming msg_id contains client_id
                RouteTo::Single(std::borrow::Cow::Owned(client_id.clone()))
            }
            EMsg::PreAccept(_) | EMsg::Accept(_) | EMsg::Commit(_) => {
                // Broadcast PreAccept to all replicas except itself
                let mut dests: Vec<String> = self
                    .replica_list
                    .iter()
                    .filter(|r| *r != &self.replica_name)
                    .cloned()
                    .collect();

                if dests.is_empty() {
                    // send to myself. add myself into dests
                    dests.push(self.replica_name.clone());
                }
                // info!("Broadcasting message to : {:?}", dests);
                RouteTo::Multiple(std::borrow::Cow::Owned(dests))
            }
            EMsg::PreAcceptOk(_) | EMsg::AcceptOk(_) => RouteTo::Reply,
            _ => {
                panic!("Server tried to send non ClientResponse")
            }
        }
    }
}

// //////////////////////////////////////////////////////////////////////////////
//                                  ACTORS
// //////////////////////////////////////////////////////////////////////////////

/// Epaxos server actor
pub async fn server(ctx: RuntimeCtx, replica_list: Vec<String>) {
    let replica_name = ctx.addr.to_string();
    BehaviourBuilder::new(
        Processor::new(replica_list.clone(), replica_name.clone()),
        BincodeCodec::default(),
    )
    .send(Sender {
        replica_name,
        replica_list,
    })
    .on_send_failure(SendErrAction::Drop)
    .build()
    .run(ctx)
    .await
    .unwrap();
}
