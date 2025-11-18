use crate::common::{
    AcceptMsg, AcceptOkMsg, ClientRequest, Command, CommitMsg, EMsg, Instance, PreAcceptMsg,
    PreAcceptOkMsg, Variable,
};
use reactor_actor::codec::BincodeCodec;
use reactor_actor::{BehaviourBuilder, RouteTo, RuntimeCtx, SendErrAction};

use core::panic;
use std::collections::{HashMap, HashSet};
use std::vec;
use tracing::{debug, error, info, warn};

// //////////////////////////////////////////////////////////////////////////////
//                                  Processor
// //////////////////////////////////////////////////////////////////////////////

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

    // instance_num: u64,
    instance_num: usize,
    quorum_ctr: Vec<u32>, // Counter for PreAcceptOk messages, // Indexed by instance number
    app_meta: Vec<CmdMetadata>, // Indexed by instance number

    replica_list: Vec<String>,
    replica_name: String, // Myself
}

impl Processor {
    fn get_majority(&self) -> u32 {
        (self.replica_list.len() / 2) as u32
    }

    fn fast_quorum(&self) -> u32 {
        (self.replica_list.len() - 1) as u32
    }
    // for given new size and replica, increase the cmds[replica] vector to that size with empty values in extra slots
    fn resize_cmds(&mut self, new_size: usize, replica: &String) {
        let cmds_for_replica = self.cmds.get_mut(replica).expect("replica not found");
        let current_size = cmds_for_replica.len();
        if new_size > current_size {
            cmds_for_replica.resize_with(new_size, || None);
        }
    }

    fn cmds_insert(&mut self, instance: &Instance, cmd_entry: CmdEntry) {
        let index = instance.instance_num;
        let required_size = index + 1;

        self.resize_cmds(required_size, &instance.replica);

        let cmds_for_replica = self
            .cmds
            .get_mut(&instance.replica)
            .expect("Internal error: Replica vector should exist after initialization.");

        let position = cmds_for_replica
            .get_mut(index)
            .expect("Index should be valid after resize_cmds was called.");

        match position {
            None => {
                *position = Some(cmd_entry);
            }
            Some(_) => {
                panic!("Slot occupied");
            }
        }
    }
    // used to get deps of a given cmd entry
    // iterates through all CmdInstance present in cmds for all replicas, and if key is same,
    // add it to cmd_entry deps
    fn get_interfs(&self, cmd: &Command) -> (HashSet<Instance>, u64) {
        let mut deps = HashSet::new();
        let mut max_seq = 0;

        for (r, cmds_vec) in &self.cmds {
            for (i, cmd_opt) in cmds_vec.iter().enumerate() {
                if let Some(c) = cmd_opt {
                    if c.cmd.conflicts_with(&cmd) {
                        deps.insert(Instance {
                            replica: r.clone(),
                            instance_num: i,
                        });
                        // Update max_seq with the maximum seq value from the dependency
                        max_seq = max_seq.max(c.seq);
                    }
                }
            }
        }

        max_seq += 1; // incrementing max_seq before returning
        return (deps, max_seq);
    }
}

impl reactor_actor::ActorProcess for Processor {
    type IMsg = EMsg;
    type OMsg = EMsg;

    fn process(&mut self, input: Self::IMsg) -> Vec<Self::OMsg> {
        match input {
            EMsg::ClientRequest(msg) => {
                let ClientRequest { cmd, .. } = msg;

                // Purely for checking starting case where inst_num is already 0, no need to increment
                let vec_size = self.cmds.get(&self.replica_name).unwrap().len();
                if vec_size > 0 {
                    self.instance_num += 1;
                }

                self.quorum_ctr.push(0); // push 0 to quorum_ctr list to not resize later

                let (deps, seq) = self.get_interfs(&cmd);

                let cmd_entry = CmdEntry {
                    cmd: cmd.clone(),
                    seq,
                    deps: deps.clone(),
                    status: CmdStatus::PreAccepted,
                };

                let cmds_vec = self.cmds.get_mut(&self.replica_name).unwrap();
                cmds_vec.push(Some(cmd_entry));

                let instance = Instance {
                    replica: self.replica_name.clone(),
                    instance_num: self.instance_num,
                };

                let pre_accept = EMsg::PreAccept(PreAcceptMsg {
                    cmd,
                    seq,
                    deps,
                    instance,
                });

                vec![pre_accept]
            }

            EMsg::PreAccept(msg) => {
                let PreAcceptMsg {
                    cmd,
                    seq,
                    deps,
                    instance,
                } = msg;

                // Get Interfering instances and max seq, check with incoming msg and update
                let (mut interf_deps, mut interf_seq) = self.get_interfs(&cmd);
                interf_deps.extend(deps.into_iter());
                interf_seq = interf_seq.max(seq);

                // Add the incoming command to the cmds log
                let cmd_entry = CmdEntry {
                    cmd,
                    seq: interf_seq,
                    deps: interf_deps.clone(),
                    status: CmdStatus::PreAccepted,
                };
                // Add the incoming command to the cmds log
                self.cmds_insert(&instance, cmd_entry);

                // Prepare and send PreAcceptOk message
                let pre_accept_ok = EMsg::PreAcceptOk(PreAcceptOkMsg {
                    seq: interf_seq,
                    deps: interf_deps,
                    instance,
                });

                vec![pre_accept_ok]
            }
            EMsg::PreAcceptOk(msg) => {
                let PreAcceptOkMsg {
                    seq,
                    deps,
                    instance,
                } = msg;

                let Instance {
                    replica,
                    instance_num: inst_num,
                } = instance.clone();

                // should we check if this replica is same as replica name just to ensure that preAcceptok comes to leader only?
                if replica != self.replica_name {
                    error!("PreAcceptOk received by non-leader replica");
                    return vec![];
                }

                // Defining quorum constants
                let majority = self.get_majority();
                let fast_quorum = self.fast_quorum();

                // Ensure the command exists in the log
                let cmd_entry_mut: &mut CmdEntry = self
                    .cmds
                    .get_mut(&replica)
                    .unwrap()
                    .get_mut(inst_num)
                    .unwrap()
                    .as_mut()
                    .expect("Command not found in log");

                // Check if already committed
                if matches!(cmd_entry_mut.status, CmdStatus::Committed) {
                    debug!("PreAcceptOk received for already committed command");
                    return vec![]; // Ignore the message 
                    // TODO: Can add optional debug checks to prove invariance that newer messages would not have unseen interfering commands
                }

                // Check if accepted
                if matches!(cmd_entry_mut.status, CmdStatus::Accepted) {
                    // Ensure quorum counter is less than majority
                    if self.quorum_ctr[inst_num] >= majority {
                        debug!(
                            "PreAcceptOk received for already accepted command with sufficient quorum"
                        );
                        return vec![]; // Ignore the message if already accepted and quorum is reached
                        // TODO: Again, same invariant of no conflict possible
                    }
                }

                // Check if seq and deps match
                if cmd_entry_mut.seq != seq || cmd_entry_mut.deps != deps {
                    // Update seq and deps
                    cmd_entry_mut.seq = cmd_entry_mut.seq.max(seq);
                    cmd_entry_mut.deps.extend(deps.into_iter());
                    cmd_entry_mut.status = CmdStatus::Accepted;
                }

                // Increment the counter for PreAcceptOk messages
                self.quorum_ctr[inst_num] += 1;

                let ctr = self.quorum_ctr[inst_num];

                // Check if majority is reached
                if ctr == majority {
                    if matches!(cmd_entry_mut.status, CmdStatus::Accepted) {
                        // check if status is Accepted
                        // Phase 2: Paxos-Accept

                        // Reset quorum counter for reuse
                        self.quorum_ctr[inst_num] = 0;

                        let accept_msg = EMsg::Accept(AcceptMsg {
                            cmd: cmd_entry_mut.cmd.clone(),
                            seq: cmd_entry_mut.seq,
                            deps: cmd_entry_mut.deps.clone(),
                            instance: instance,
                        });
                        return vec![accept_msg];
                    } else {
                        // Wait for fast quorum
                        return vec![];
                    }
                }

                // Check if fast quorum is reached
                if ctr == fast_quorum {
                    if matches!(cmd_entry_mut.status, CmdStatus::PreAccepted) {
                        // status is PreAccpeted
                        // Commit phase
                        // changing msg status to committed
                        cmd_entry_mut.status = CmdStatus::Committed;

                        let commit_msg = EMsg::Commit(CommitMsg {
                            cmd: cmd_entry_mut.cmd.clone(),
                            seq: cmd_entry_mut.seq,
                            deps: cmd_entry_mut.deps.clone(),
                            instance: instance.clone(),
                        });
                        return vec![commit_msg];
                    } else {
                        panic!("Quorum intersection invariant violated");
                    }
                }

                vec![]
            }
            EMsg::Commit(msg) => {
                let CommitMsg {
                    cmd,
                    seq,
                    deps,
                    instance,
                } = msg;

                // Create a new CmdEntry with the Committed status
                let cmd_entry = CmdEntry {
                    cmd: cmd,
                    seq: seq,
                    deps: deps,
                    status: CmdStatus::Committed,
                };

                // Insert the CmdEntry into the cmds array
                self.cmds_insert(&instance, cmd_entry);

                vec![]
            }
            EMsg::Accept(msg) => {
                let AcceptMsg {
                    cmd,
                    seq,
                    deps,
                    instance,
                } = msg;

                // Create a new CmdEntry with the Accepted status
                let cmd_entry = CmdEntry {
                    cmd: cmd.clone(),
                    seq: seq,
                    deps: deps.clone(),
                    status: CmdStatus::Accepted,
                };

                // Create or update the CmdEntry with the Accepted status
                self.cmds_insert(&instance, cmd_entry);

                // Prepare and send AcceptOk message
                let accept_ok_msg = EMsg::AcceptOk(AcceptOkMsg { instance });

                vec![accept_ok_msg]
            }
            EMsg::AcceptOk(msg) => {
                let AcceptOkMsg { instance } = msg;
                let Instance {
                    replica,
                    instance_num: inst_num,
                } = instance.clone();

                // should we check if this replica is same as replica name just to ensure that acceptok comes to leader only?
                if replica != self.replica_name {
                    return vec![];
                }

                let majority = self.get_majority();

                // Ensure the command exists in the log
                let cmd_entry_mut = self
                    .cmds
                    .get_mut(&replica)
                    .unwrap()
                    .get_mut(inst_num)
                    .unwrap()
                    .as_mut()
                    .expect("Command not found in log");

                // Check if already committed
                if matches!(cmd_entry_mut.status, CmdStatus::Committed) {
                    return vec![]; // Ignore the message
                }

                // Increment the counter for AcceptOk messages
                self.quorum_ctr[inst_num] += 1; // reused quorum_ctr

                let ctr = self.quorum_ctr[inst_num];

                // Check if majority is reached
                if ctr == majority {
                    // Commit phase
                    cmd_entry_mut.status = CmdStatus::Committed;

                    let commit_msg = EMsg::Commit(CommitMsg {
                        cmd: cmd_entry_mut.cmd.clone(),
                        seq: cmd_entry_mut.seq,
                        deps: cmd_entry_mut.deps.clone(),
                        instance: instance,
                    });

                    return vec![commit_msg];
                }
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

struct Sender {
    replica_name: String,
    replica_list: Vec<String>,
}

impl reactor_actor::ActorSend for Sender {
    type OMsg = EMsg;

    async fn before_send<'a>(&'a mut self, _output: &Self::OMsg) -> RouteTo<'a> {
        match &_output {
            EMsg::ClientResponse(_) => RouteTo::Reply,
            EMsg::PreAccept(_) | EMsg::Accept(_) | EMsg::Commit(_) => {
                // Broadcast PreAccept to all replicas except itself
                let dests: Vec<String> = self
                    .replica_list
                    .iter()
                    .filter(|r| *r != &self.replica_name)
                    .cloned()
                    .collect();

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
