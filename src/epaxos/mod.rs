use crate::common::{ClientResponse, Command, CommandResult, EMsg, Variable};
use reactor_actor::codec::BincodeCodec;
use reactor_actor::{BehaviourBuilder, RouteTo, RuntimeCtx, SendErrAction};

use std::collections::{HashMap, HashSet};
use std::vec;

// //////////////////////////////////////////////////////////////////////////////
//                                  Processor
// //////////////////////////////////////////////////////////////////////////////

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct Instance {
    replica: String,
    instance_num: u64,
}

enum CmdStatus {
    None,
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

struct CmdMetadata {
    client_id: String,
    msg_id: String,
}

struct Processor {
    data: HashMap<Variable, String>,
    // cmds: HashMap<String, Vec<CmdInstance>>,
    cmds: HashMap<String, Vec<Option<CmdEntry>>>,

    instance_num: u64,
    quorum_ctr: Vec<u32>,       // Indexed by instance number
    app_meta: Vec<CmdMetadata>, // Indexed by instance number

    replica_list: Vec<String>,
    replica_name: String, // Myself
}

impl Processor {
    // for given new size and replica, increase the cmds[replica] vector to that size with empty values in extra slots
    fn resize_cmds(&mut self, new_size: usize, replica: &String) {
        let cmds_for_replica = self.cmds.get_mut(replica).expect("replica not found");
        let current_size = cmds_for_replica.len();
        if new_size > current_size {
            cmds_for_replica.resize_with(new_size, || None);
        }
    }

    // used to get deps of a given cmd entry
    // iterates through all CmdInstance present in cmds for all replicas, and if key is same,
    // add it to cmd_entry deps
    // fn get_interfs(&self, cmd_entry: &mut CmdEntry) {
    fn get_interfs(&mut self, replica: String, inst_num: u64) {
        let &mut cmd_entry = self
            .cmds
            .get_mut(&replica)
            .unwrap()
            .get_mut(inst_num as usize)
            .unwrap();
        // let &mut deps = cmd_entry.deps;
        // get a mutable reference of deps and work on that
        let mut deps: HashSet<Instance> = HashSet::new();
        for (replica, cmds_vec) in &self.cmds {
            for (inst_num, cmd_opt) in cmds_vec.iter().enumerate() {
                if let Some(cmd) = cmd_opt {
                    let cmd_cmd = &cmd.cmd;
                    if cmd_cmd.conflicts_with(&cmd_entry.unwrap().cmd) {
                        deps.insert(Instance {
                            replica: replica.clone(),
                            instance_num: inst_num as u64,
                        });
                    }
                }
            }
        }
        // do set union of deps with cmd_entry.deps
        cmd_entry.deps.extend(deps);
    }
}

impl reactor_actor::ActorProcess for Processor {
    type IMsg = EMsg;
    type OMsg = EMsg;

    fn process(&mut self, input: Self::IMsg) -> Vec<Self::OMsg> {
        match &input {
            EMsg::ClientRequest(msg) => {
                let cmds_entry = self.cmds.get_mut(&self.replica_name).unwrap();
                let vec_size = cmds_entry.len();
                if (vec_size > 0) {
                    self.instance_num += 1;
                }
                let inst_num = self.instance_num;
                let mut cmd_entry = CmdEntry {
                    cmd: msg.cmd.clone(),
                    seq: 0,
                    deps: HashSet::new(),
                    status: CmdStatus::PreAccepted,
                };
                cmds_entry.push(Some(cmd_entry));
                // self.get_interfs(&mut cmd_entry);
                self.get_interfs(self.replica_name.clone(), inst_num);

                // self.resize_cmds(inst_num.try_into().unwrap(), &self.replica_name);

                vec![]
            }
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

struct Sender {}
impl reactor_actor::ActorSend for Sender {
    type OMsg = EMsg;

    async fn before_send<'a>(&'a mut self, _output: &Self::OMsg) -> RouteTo<'a> {
        match &_output {
            EMsg::ClientResponse(_) => RouteTo::Reply,
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
        Processor::new(replica_list, replica_name),
        BincodeCodec::default(),
    )
    .send(Sender {})
    .on_send_failure(SendErrAction::Drop)
    .build()
    .run(ctx)
    .await
    .unwrap();
}
