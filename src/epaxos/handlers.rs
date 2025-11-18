use crate::common::{
    AcceptMsg, AcceptOkMsg, ClientRequest, ClientResponse, Command, CommandResult, CommitMsg, EMsg,
    Instance, PreAcceptMsg, PreAcceptOkMsg,
};
use crate::epaxos::{CmdEntry, CmdMetadata, CmdStatus, Processor};

use std::vec;
use tracing::{debug, error};

impl Processor {
    pub fn client_request_handler(&mut self, msg: ClientRequest) -> Vec<EMsg> {
        let ClientRequest {
            cmd,
            msg_id,
            client_id,
        } = msg;

        match &cmd {
            Command::Set { .. } => {
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

                // Store client metadata in app_meta
                self.app_meta.push(CmdMetadata { client_id, msg_id });

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

            Command::Get { .. } => {
                // Handle Read Request (to be implemented later)
                vec![]
            }
        }
    }

    pub fn pre_accept_handler(&mut self, msg: PreAcceptMsg) -> Vec<EMsg> {
        let PreAcceptMsg {
            cmd,
            seq,
            deps,
            instance,
        } = msg;

        // Get Interfering instances and max seq, check with incoming msg and update
        let (mut interf_deps, mut interf_seq) = self.get_interfs(&cmd);
        interf_deps.extend(deps);
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
    pub fn pre_accept_ok_handler(&mut self, msg: PreAcceptOkMsg) -> Vec<EMsg> {
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
                debug!("PreAcceptOk received for already accepted command with sufficient quorum");
                return vec![]; // Ignore the message if already accepted and quorum is reached
                // TODO: Again, same invariant of no conflict possible
            }
        }

        // Check if seq and deps match
        if cmd_entry_mut.seq != seq || cmd_entry_mut.deps != deps {
            // Update seq and deps
            cmd_entry_mut.seq = cmd_entry_mut.seq.max(seq);
            cmd_entry_mut.deps.extend(deps);
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
                    instance,
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

                // Only for write commands (Set)
                if matches!(cmd_entry_mut.cmd, Command::Set { .. }) {
                    let CmdMetadata { msg_id, client_id } = &self.app_meta[inst_num];

                    let client_response = EMsg::ClientResponse(ClientResponse {
                        msg_id: msg_id.clone(),
                        client_id: client_id.clone(),
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
    pub fn commit_handler(&mut self, msg: CommitMsg) -> Vec<EMsg> {
        let CommitMsg {
            cmd,
            seq,
            deps,
            instance,
        } = msg;

        // Create a new CmdEntry with the Committed status
        let cmd_entry = CmdEntry {
            cmd,
            seq,
            deps,
            status: CmdStatus::Committed,
        };

        // Insert the CmdEntry into the cmds array
        self.cmds_insert(&instance, cmd_entry);

        vec![]
    }
    pub fn accept_handler(&mut self, msg: AcceptMsg) -> Vec<EMsg> {
        let AcceptMsg {
            cmd,
            seq,
            deps,
            instance,
        } = msg;

        // Create a new CmdEntry with the Accepted status
        let cmd_entry = CmdEntry {
            cmd: cmd.clone(),
            seq,
            deps: deps.clone(),
            status: CmdStatus::Accepted,
        };

        // Create or update the CmdEntry with the Accepted status
        self.cmds_insert(&instance, cmd_entry);

        // Prepare and send AcceptOk message
        let accept_ok_msg = EMsg::AcceptOk(AcceptOkMsg { instance });

        vec![accept_ok_msg]
    }
    pub fn accept_ok_handler(&mut self, msg: AcceptOkMsg) -> Vec<EMsg> {
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
                instance,
            });

            return vec![commit_msg];
        }
        vec![]
    }
}
