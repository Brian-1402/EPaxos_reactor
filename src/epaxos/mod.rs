use crate::common::{Command, EMsg, Variable, Instance, PreAcceptMsg, PreAcceptOkMsg, AcceptMsg, CommitMsg};
use reactor_actor::codec::BincodeCodec;
use reactor_actor::{BehaviourBuilder, RouteTo, RuntimeCtx, SendErrAction};

use std::collections::{HashMap, HashSet};
use std::vec;

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
        // Step 1: read-only borrow to compute deps and calculate max seq
        let (deps, max_seq) = {
            let mut deps = HashSet::new();
            let mut max_seq = 0;
    
            let cmd = self.cmds[&replica][inst_num as usize]
                .as_ref()
                .unwrap()
                .cmd
                .clone();
    
            for (r, cmds_vec) in &self.cmds {
                for (i, cmd_opt) in cmds_vec.iter().enumerate() {
                    if let Some(c) = cmd_opt {
                        if c.cmd.conflicts_with(&cmd) {
                            deps.insert(Instance {
                                replica: r.clone(),
                                instance_num: i as u64,
                            });
                            // Update max_seq with the maximum seq value from the dependency
                            max_seq = max_seq.max(c.seq);
                        }
                    }
                }
            }
    
            (deps, max_seq)
        };
    
        // Step 2: mutable borrow only after reading is done
        let cmd_entry = self
            .cmds
            .get_mut(&replica)
            .unwrap()
            .get_mut(inst_num as usize)
            .unwrap()
            .as_mut()
            .unwrap();
    
        cmd_entry.seq = cmd_entry.seq.max(1 + max_seq);
    
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
                if vec_size > 0 {
                    self.instance_num += 1;
                }
                let inst_num = self.instance_num;
                let cmd_entry = CmdEntry {
                    cmd: msg.cmd.clone(),
                    seq: 0,
                    deps: HashSet::new(),
                    status: CmdStatus::PreAccepted,
                };
                cmds_entry.push(Some(cmd_entry));
                self.get_interfs(self.replica_name.clone(), inst_num);

                let inst = Instance {
                    replica: self.replica_name.clone(),
                    instance_num: inst_num,
                };

                let entry = self.cmds[&self.replica_name][inst_num as usize]
                .as_ref()
                .unwrap();

                let pre_accept = EMsg::PreAccept(PreAcceptMsg {
                    cmd: entry.cmd.clone(),
                    seq: entry.seq,
                    deps: entry.deps.clone(),
                    instance: inst.clone(),
                });
                
                vec![pre_accept]
            }
            EMsg::PreAccept(msg) => {
                let replica = msg.instance.replica.clone();
                let inst_num = msg.instance.instance_num;

                // Ensure the cmds log can accommodate the incoming instance
                self.resize_cmds((inst_num + 1) as usize, &replica);

                // Add the incoming command to the cmds log
                let cmd_entry = CmdEntry {
                    cmd: msg.cmd.clone(),
                    seq: msg.seq,
                    deps: msg.deps.clone(),
                    status: CmdStatus::PreAccepted,
                };

                // Add the incoming command to the cmds log
                self.cmds
                    .get_mut(&replica)
                    .unwrap()
                    .insert(inst_num as usize, Some(cmd_entry));

                // Update seq and deps using get_interfs
                self.get_interfs(replica.clone(), inst_num);

                // Prepare and send PreAcceptOk message
                let entry = self.cmds[&replica][inst_num as usize]
                    .as_ref()
                    .unwrap();
                
                let pre_accept_ok = EMsg::PreAcceptOk(PreAcceptOkMsg {
                    seq: entry.seq,
                    deps: entry.deps.clone(),
                    instance: msg.instance.clone(),
                });

                vec![pre_accept_ok]
            }
            EMsg::PreAcceptOk(msg) => {
                let replica = msg.instance.replica.clone();
                let inst_num = msg.instance.instance_num;

                // should we check if this replica is same as replica name just to ensure that preAccept comes to leader only?

                // Ensure the command exists in the log
                let cmd_entry_mut = self.cmds.get_mut(&replica).unwrap()
                    .get_mut(inst_num as usize).unwrap()
                    .as_mut().expect("Command not found in log");

                // Check if already accepted or committed
                if matches!(cmd_entry_mut.status, CmdStatus::Accepted | CmdStatus::Committed) {
                    return vec![]; // Ignore the message
                }

                // Check if seq and deps match
                let mut conflicts = false;
                if cmd_entry_mut.seq != msg.seq || cmd_entry_mut.deps != msg.deps {
                    conflicts = true;

                    // Update seq and deps
                    cmd_entry_mut.seq = cmd_entry_mut.seq.max(msg.seq);
                    cmd_entry_mut.deps.extend(msg.deps.clone());
                }

                // Increment the counter for PreAcceptOk messages
                if self.quorum_ctr.len() <= inst_num as usize {
                    self.quorum_ctr.resize(inst_num as usize + 1, 0);
                }
                self.quorum_ctr[inst_num as usize] += 1;

                let ctr = self.quorum_ctr[inst_num as usize];
                let majority = (self.replica_list.len() / 2) as u32;
                let fast_quorum = (self.replica_list.len() - 1) as u32; // using unoptimized fast path quorum
                
                let inst = Instance {
                    replica: self.replica_name.clone(),
                    instance_num: self.instance_num,
                };

                // Check if majority is reached
                if ctr == majority {
                    if conflicts {
                        // Phase 2: Paxos-Accept
                        // changing msg status to accepted 
                        cmd_entry_mut.status = CmdStatus::Accepted;

                        let accept_msg = EMsg::Accept(AcceptMsg {
                            cmd: cmd_entry_mut.cmd.clone(),
                            seq: cmd_entry_mut.seq,
                            deps: cmd_entry_mut.deps.clone(),
                            instance: inst.clone(),
                        });
                        return vec![accept_msg];
                    } else {
                        // Wait for fast quorum
                        return vec![];
                    }
                }

                // Check if fast quorum is reached
                if ctr == fast_quorum {
                    if !conflicts {
                        // Commit phase
                        // changing msg status to committed 
                        cmd_entry_mut.status = CmdStatus::Committed;

                        let commit_msg = EMsg::Commit(CommitMsg {
                            cmd: cmd_entry_mut.cmd.clone(),
                            seq: cmd_entry_mut.seq,
                            deps: cmd_entry_mut.deps.clone(),
                            instance: inst.clone(),
                        });
                        return vec![commit_msg];
                    } else {
                        panic!("Quorum intersection invariant violated");
                    }
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
                let dests: Vec<String> = self.replica_list
                    .iter()
                    .filter(|r| *r != &self.replica_name)
                    .cloned()
                    .collect();

                RouteTo::Multiple(std::borrow::Cow::Owned(dests))
            }
            EMsg::PreAcceptOk(_) => RouteTo::Reply,
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
