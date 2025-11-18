use crate::common::{Command, 
    EMsg, 
    Variable, 
    Instance, 
    PreAcceptMsg, 
    PreAcceptOkMsg, 
    AcceptMsg, 
    AcceptOkMsg, 
    CommitMsg, 
    ClientResponse,
    CommandResult
};
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
    quorum_ctr: Vec<u32>,       // Counter for PreAcceptOk messages, // Indexed by instance number
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
                match &msg.cmd {
                    Command::Set { .. } => {
                        let cmds_entry = self.cmds.get_mut(&self.replica_name).unwrap();
                        let vec_size = cmds_entry.len();
                        if vec_size > 0 {
                            self.instance_num += 1;
                        }
                        let inst_num = self.instance_num;

                        self.quorum_ctr.push(0); // push 0 to quorum_ctr list to not resize later

                        let cmd_entry = CmdEntry {
                            cmd: msg.cmd.clone(),
                            seq: 0,
                            deps: HashSet::new(),
                            status: CmdStatus::PreAccepted,
                        };
                        cmds_entry.push(Some(cmd_entry));
                        self.get_interfs(self.replica_name.clone(), inst_num);

                        // Store client metadata in app_meta
                        self.app_meta.push(CmdMetadata {
                            client_id: msg.client_id.clone(),
                            msg_id: msg.msg_id.clone(),
                        });

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
                    Command::Get { .. } => {
                        // Handle Read Request (to be implemented later)
                        vec![]
                    }
                }
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

                // should we check if this replica is same as replica name just to ensure that preAcceptok comes to leader only?
                if msg.instance.replica != self.replica_name {
                    return vec![];
                }

                // Ensure the command exists in the log
                let cmd_entry_mut = self.cmds.get_mut(&replica).unwrap()
                    .get_mut(inst_num as usize).unwrap()
                    .as_mut().expect("Command not found in log");

                // Check if already committed
                if matches!(cmd_entry_mut.status, CmdStatus::Committed) {
                    return vec![]; // Ignore the message 
                }

                // Check if accepted
                let majority = (self.replica_list.len() / 2) as u32;
                if matches!(cmd_entry_mut.status, CmdStatus::Accepted) {
                    // Ensure quorum counter is less than majority
                    if self.quorum_ctr.len() > inst_num as usize && self.quorum_ctr[inst_num as usize] >= majority {
                        return vec![]; // Ignore the message if already accepted and quorum is reached
                    }
                }

                // Check if seq and deps match
                if cmd_entry_mut.seq != msg.seq || cmd_entry_mut.deps != msg.deps {

                    // Update seq and deps
                    cmd_entry_mut.seq = cmd_entry_mut.seq.max(msg.seq);
                    cmd_entry_mut.deps.extend(msg.deps.clone());
                    cmd_entry_mut.status = CmdStatus::Accepted;
                }

                // Increment the counter for PreAcceptOk messages
                self.quorum_ctr[inst_num as usize] += 1;

                let ctr = self.quorum_ctr[inst_num as usize];
                let fast_quorum = (self.replica_list.len() - 1) as u32; // using unoptimized fast path quorum

                // Check if majority is reached
                if ctr == majority {
                    if matches!(cmd_entry_mut.status, CmdStatus::Accepted) { // check if status is Accepted
                        // Phase 2: Paxos-Accept

                        // Reset quorum counter for reuse
                        self.quorum_ctr[inst_num as usize] = 0;

                        let accept_msg = EMsg::Accept(AcceptMsg {
                            cmd: cmd_entry_mut.cmd.clone(),
                            seq: cmd_entry_mut.seq,
                            deps: cmd_entry_mut.deps.clone(),
                            instance: msg.instance.clone(),
                        });
                        return vec![accept_msg];
                    } else {
                        // Wait for fast quorum
                        return vec![];
                    }
                }

                // Check if fast quorum is reached
                if ctr == fast_quorum {
                    if matches!(cmd_entry_mut.status, CmdStatus::PreAccepted) { // status is PreAccpeted
                        // Commit phase
                        // changing msg status to committed 
                        cmd_entry_mut.status = CmdStatus::Committed;

                        let commit_msg = EMsg::Commit(CommitMsg {
                            cmd: cmd_entry_mut.cmd.clone(),
                            seq: cmd_entry_mut.seq,
                            deps: cmd_entry_mut.deps.clone(),
                            instance: msg.instance.clone(),
                        });

                        // Only for write commands (Set)
                        if matches!(cmd_entry_mut.cmd, Command::Set { .. }) {
                            let client_meta = &self.app_meta[inst_num as usize];

                            let client_response = EMsg::ClientResponse(ClientResponse {
                                msg_id: client_meta.msg_id.clone(),
                                client_id: client_meta.client_id.clone(),
                                cmd_result: CommandResult::Set {
                                    key: cmd_entry_mut.cmd.key().clone(),
                                    status: true,
                                },
                            });

                            return vec![
                                commit_msg,
                                client_response, // send to correct client
                            ];
                        } else {
                            return vec![commit_msg];
                        }
                    } else {
                        panic!("Quorum intersection invariant violated");
                    }
                }

                vec![]
            }
            EMsg::Commit(msg) => {
                let replica = msg.instance.replica.clone();
                let inst_num = msg.instance.instance_num;

                // Step 1: Resize the cmds array for the given replica
                self.resize_cmds((inst_num + 1) as usize, &replica);

                // Step 2: Create a new CmdEntry with the Committed status
                let cmd_entry = CmdEntry {
                    cmd: msg.cmd.clone(),
                    seq: msg.seq,
                    deps: msg.deps.clone(),
                    status: CmdStatus::Committed,
                };

                // Step 3: Insert the CmdEntry into the cmds array
                self.cmds
                    .get_mut(&replica)
                    .unwrap()
                    .insert(inst_num as usize, Some(cmd_entry));
                
                vec![]
            }
            EMsg::Accept(msg) => {
                let replica = msg.instance.replica.clone();
                let inst_num = msg.instance.instance_num;

                // Step 1: Resize the cmds array for the given replica
                self.resize_cmds((inst_num + 1) as usize, &replica);

                let cmd_entry = CmdEntry {
                    cmd: msg.cmd.clone(),
                    seq: msg.seq,
                    deps: msg.deps.clone(),
                    status: CmdStatus::Accepted,
                };

                // Step 2: Create or update the CmdEntry with the Accepted status
                self.cmds
                    .get_mut(&replica)
                    .unwrap()
                    .insert(inst_num as usize, Some(cmd_entry));

                // Step 3: Prepare and send AcceptOk message
                let accept_ok_msg = EMsg::AcceptOk(AcceptOkMsg {
                    instance: msg.instance.clone(),
                });

                vec![accept_ok_msg]
            }
            EMsg::AcceptOk(msg) => {
                let replica = msg.instance.replica.clone();
                let inst_num = msg.instance.instance_num;

                // should we check if this replica is same as replica name just to ensure that acceptok comes to leader only?
                if msg.instance.replica != self.replica_name {
                    return vec![];
                }

                // Ensure the command exists in the log
                let cmd_entry_mut = self.cmds.get_mut(&replica).unwrap()
                    .get_mut(inst_num as usize).unwrap()
                    .as_mut().expect("Command not found in log");

                // Check if already committed
                if matches!(cmd_entry_mut.status, CmdStatus::Committed) {
                    return vec![]; // Ignore the message
                }

                // Increment the counter for AcceptOk messages
                self.quorum_ctr[inst_num as usize] += 1; // reused quorum_ctr

                let ctr = self.quorum_ctr[inst_num as usize];
                let majority = (self.replica_list.len() / 2) as u32;

                // Check if majority is reached
                if ctr == majority {
                    // Commit phase
                    cmd_entry_mut.status = CmdStatus::Committed;

                    let commit_msg = EMsg::Commit(CommitMsg {
                        cmd: cmd_entry_mut.cmd.clone(),
                        seq: cmd_entry_mut.seq,
                        deps: cmd_entry_mut.deps.clone(),
                        instance: msg.instance.clone(),
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
            EMsg::ClientResponse(response) => {
                // Use client_id to route the message to the correct client
                let client_id = &response.client_id; // Assuming msg_id contains client_id
                RouteTo::Single(std::borrow::Cow::Owned(client_id.clone()))
            }
            EMsg::PreAccept(_) | EMsg::Accept(_) | EMsg::Commit(_) => {
                // Broadcast PreAccept to all replicas except itself
                let dests: Vec<String> = self.replica_list
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
