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

use std::collections::{HashMap, HashSet, VecDeque};
use std::vec;

// //////////////////////////////////////////////////////////////////////////////
//                                  Processor
// //////////////////////////////////////////////////////////////////////////////

enum CmdStatus {
    // None,
    PreAccepted,
    Accepted,
    Committed,
    Executed,
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
    pending_reads: HashSet<Instance>, // pending list of outstanding reads
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
                    if let Some(c) = cmd_opt { // do not add the own cmd
                        // Skip the current command itself
                        if r == &replica && i as u64 == inst_num {
                            continue;
                        }

                        if matches!(c.cmd, Command::Get { .. }) {
                            continue;
                        }

                        // Skip commands that are already executed
                        if matches!(c.status, CmdStatus::Executed) {
                            continue;
                        }

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

    // precondition: all dependencies are either committed or executed
    fn build_dep_graph(&self, root: Instance)
    -> HashMap<Instance, Vec<Instance>>
    {
        let mut graph = HashMap::<Instance, Vec<Instance>>::new();
        let mut stack = vec![root.clone()];
        let mut visited = HashSet::<Instance>::new();

        while let Some(inst) = stack.pop() {
            if visited.contains(&inst) {
                continue;
            }
            visited.insert(inst.clone());

            // lookup log entry // wt if there is no entry? TODO
            let entry_opt = self.cmds
                .get(&inst.replica)
                .and_then(|v| v.get(inst.instance_num as usize))
                .and_then(|opt| opt.as_ref());

            if let Some(entry) = entry_opt {
                let mut deps_vec = Vec::new();
                for dep in &entry.deps {
                    deps_vec.push(dep.clone());
                    stack.push(dep.clone());
                }
                graph.insert(inst.clone(), deps_vec);
            }
        }
        graph
    }

    fn tarjan_scc(
        &self,
        graph: &HashMap<Instance, Vec<Instance>>
    ) -> Vec<Vec<Instance>> 
    {
        // Standard Tarjan SCC implementation
        // I give the full working version below:
        
        let mut index = 0;
        let mut stack = Vec::<Instance>::new();
        let mut on_stack = HashSet::<Instance>::new();
        let mut indices = HashMap::<Instance, i32>::new();
        let mut lowlink = HashMap::<Instance, i32>::new();
        let mut result = Vec::<Vec<Instance>>::new();
    
        fn strongconnect(
            v: Instance,
            index: &mut i32,
            stack: &mut Vec<Instance>,
            on_stack: &mut HashSet<Instance>,
            indices: &mut HashMap<Instance, i32>,
            lowlink: &mut HashMap<Instance, i32>,
            graph: &HashMap<Instance, Vec<Instance>>,
            result: &mut Vec<Vec<Instance>>,
        ) {
            indices.insert(v.clone(), *index);
            lowlink.insert(v.clone(), *index);
            *index += 1;
    
            stack.push(v.clone());
            on_stack.insert(v.clone());
    
            if let Some(neighbors) = graph.get(&v) {
                for w in neighbors {
                    if !indices.contains_key(w) {
                        strongconnect(
                            w.clone(), index, stack, on_stack,
                            indices, lowlink, graph, result
                        );
                        let low_v = *lowlink.get(&v).unwrap();
                        let low_w = *lowlink.get(w).unwrap();
                        lowlink.insert(v.clone(), low_v.min(low_w));
                    } else if on_stack.contains(w) {
                        let low_v = *lowlink.get(&v).unwrap();
                        let idx_w = *indices.get(w).unwrap();
                        lowlink.insert(v.clone(), low_v.min(idx_w));
                    }
                }
            }
    
            if lowlink[&v] == indices[&v] {
                let mut scc = Vec::<Instance>::new();
                loop {
                    let w = stack.pop().unwrap();
                    on_stack.remove(&w);
                    scc.push(w.clone());
                    if w == v { break; }
                }
                result.push(scc);
            }
        }
    
        for v in graph.keys() {
            if !indices.contains_key(v) {
                strongconnect(
                    v.clone(),
                    &mut index,
                    &mut stack,
                    &mut on_stack,
                    &mut indices,
                    &mut lowlink,
                    graph,
                    &mut result,
                );
            }
        }
    
        result
    }

    fn topo_sort_scc(&self, sccs: &Vec<Vec<Instance>>,
                 graph: &HashMap<Instance, Vec<Instance>>)
    -> Vec<Vec<Instance>>
    {
        // Build SCC ID map
        let mut comp_id = HashMap::<Instance, usize>::new();
        for (i, comp) in sccs.iter().enumerate() {
            for v in comp {
                comp_id.insert(v.clone(), i);
            }
        }

        // Build DAG
        let mut dag = vec![HashSet::<usize>::new(); sccs.len()];

        for (v, neighbors) in graph {
            let c_v = comp_id[v];
            for w in neighbors {
                let c_w = comp_id[w];
                if c_v != c_w {
                    dag[c_v].insert(c_w);
                }
            }
        }

        // Kahn topo sort
        let mut indeg = vec![0; sccs.len()];
        for u in 0..dag.len() {
            for &v in &dag[u] {
                indeg[v] += 1;
            }
        }

        let mut q = VecDeque::new();
        for i in 0..sccs.len() {
            if indeg[i] == 0 {
                q.push_back(i);
            }
        }

        let mut order = Vec::<usize>::new();
        while let Some(u) = q.pop_front() {
            order.push(u);
            for &v in &dag[u] {
                indeg[v] -= 1;
                if indeg[v] == 0 {
                    q.push_back(v);
                }
            }
        }

        order.into_iter().map(|i| sccs[i].clone()).collect()
    }

    fn lookup(&self, instance: &Instance) -> Option<&CmdEntry> {
        self.cmds
            .get(&instance.replica) // Get the vector of commands for the given replica
            .and_then(|cmds| cmds.get(instance.instance_num as usize)) // Get the command entry at the given instance number
            .and_then(|opt| opt.as_ref()) // Unwrap the Option<CmdEntry> to get a reference to CmdEntry
    }

    fn mark_executed(&mut self, instance: &Instance) {
        // Locate the command entry in the cmds log
        if let Some(Some(cmd_entry)) = self.cmds
            .get_mut(&instance.replica)
            .and_then(|cmds| cmds.get_mut(instance.instance_num as usize))
        {
            // Set the status to Executed
            cmd_entry.status = CmdStatus::Executed;
        } else {
            // If the command entry does not exist, log an error or handle appropriately
            panic!("Command not found in log for instance: {:?}", instance);
        }
    }
    
    // precondition: all dependencies are either committed or executed
    fn execute_cmd(&mut self, root: Instance) -> Vec<EMsg> {
        let mut out = Vec::new();
    
        // Build dependency graph
        let graph = self.build_dep_graph(root.clone());
    
        // Find SCCs
        let sccs = self.tarjan_scc(&graph);
    
        // topo order
        let order = self.topo_sort_scc(&sccs, &graph);
    
        // Reverse order
        for scc in order.into_iter().rev() {
    
            // Execute SCC in seq order
            let mut sorted = scc.clone();
            sorted.sort_by_key(|inst| {
                self.lookup(inst).unwrap().seq
            });
    
            for inst in sorted {
                if let Some(entry) = self.lookup(&inst) {
                    if matches!(entry.status, CmdStatus::Executed) {
                        continue;
                    }
    
                    match entry.cmd.clone() {
                        Command::Set { key, val } => {
                            self.data.insert(key, val);
                            self.mark_executed(&inst);
                        }
                        Command::Get { key } => {
                            // Check if the current replica is the command leader for this read
                            if inst.replica != self.replica_name {
                                continue; // Skip processing if not the command leader
                            }
                            let val = self.data.get(&key).cloned();
                            let meta = &self.app_meta[inst.instance_num as usize];
                            // check if an entry exists in app meta for that instance
                            // send response only if current replica is the command leader of the current read
                            // otherwise continue TODO
                            out.push(EMsg::ClientResponse(ClientResponse {
                                msg_id: meta.msg_id.clone(),
                                client_id: meta.client_id.clone(),
                                cmd_result: CommandResult::Get { key, val },
                            }));
    
                            self.pending_reads.remove(&inst);
                            self.mark_executed(&inst);
                        }
                    }
                }
            }
        }
    
        out
    }

    fn get_pending_reads(&self, write_instance: &Instance) -> Vec<Instance> {
        self.pending_reads
            .iter()
            .filter(|read_instance| {
                // Check if the value matches, not the reference
                if let Some(read_entry) = self.lookup(read_instance) {
                    read_entry.deps.iter().any(|dep| {
                        dep.replica == write_instance.replica && dep.instance_num == write_instance.instance_num
                    })
                } else {
                    false
                }
            })
            .cloned() // Clone the instances to return owned values
            .collect()
    }

    fn deps_all_ready(&self, inst: &Instance) -> bool {
        let entry = match self.lookup(inst) {
            Some(e) => e,
            None => return false,
        };

        for dep_inst in &entry.deps {
            match self.lookup(dep_inst) {
                Some(dep_entry) => {
                    if !matches!(dep_entry.status, CmdStatus::Committed | CmdStatus::Executed) {
                        return false;
                    }
                }
                None => return false,
            }
        }

        true
    }

    // get pending reads list on this particular write from pending_reads struct
    // This can be done by traversing the pending_reads struct, getting its deps list
    // and checking if this write cmd exists in the deps list
    // If the pending reads on this write cmd is none just commit the write msg n move on
    // If not empty get the deps of this write cmd and if all cmds are existing and status is committed or executed
    // call execute cmd otherwise just commit n move on
    // Execute cmd returns out
    // If out is not empty add it to the vector list of msgs to be sent
    // check the pending reads list on this write cmd deps again
    // Now if all the dependency in each read has status committed or executed call execute command on thet read       
    fn handle_pending_reads(&mut self, instance: &Instance) -> Vec<EMsg> {

        let mut out_msgs = Vec::new();
            
        let pending_reads_on_write = self.get_pending_reads(instance);
        if pending_reads_on_write.is_empty() {
            // No reads waiting → normal commit
            return out_msgs;
        } 

        if self.deps_all_ready(instance) {
            let mut exec_out = self.execute_cmd(instance.clone()); 
            out_msgs.append(&mut exec_out);
        } else {
            return out_msgs;
        }

        // 3. Re-check each pending read, execute if ready
        for read_inst in pending_reads_on_write {
            if self.deps_all_ready(&read_inst) {
                let mut exec_out = self.execute_cmd(read_inst);
                out_msgs.append(&mut exec_out);
            }
        }
        return out_msgs;
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

                            // put this code block in Commit also TODO
                            // put all code in one function TODO
                            // put as many panic statements as possible TODO
                       
                            let mut out_msgs = self.handle_pending_reads(&msg.instance);

                            let mut final_msgs = vec![commit_msg, client_response];
                            final_msgs.append(&mut out_msgs);
                            return final_msgs;
                            
                        } else {
                            let mut out_msgs = vec![commit_msg];
                            if self.deps_all_ready (&msg.instance) {
                                let mut exec_out = self.execute_cmd(msg.instance.clone());
                                out_msgs.append(&mut exec_out);
                            } else{
                                self.pending_reads.insert(msg.instance.clone());
                            }
                            return out_msgs;
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

                if matches!(msg.cmd, Command::Set { .. }) {
                    let mut out_msgs = self.handle_pending_reads(&msg.instance);

                    let mut final_msgs = vec![];
                    final_msgs.append(&mut out_msgs);
                    return final_msgs;
                } 

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

                        let mut out_msgs = self.handle_pending_reads(&msg.instance);

                        let mut final_msgs = vec![commit_msg, client_response];
                        final_msgs.append(&mut out_msgs);
                        return final_msgs;

                    } else {
                        let mut out_msgs = vec![commit_msg];
                        if self.deps_all_ready (&msg.instance) {
                            let mut exec_out = self.execute_cmd(msg.instance.clone());
                            out_msgs.append(&mut exec_out);
                        } else{
                            self.pending_reads.insert(msg.instance.clone());
                        }
                        return out_msgs;
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
            pending_reads: HashSet::new(),
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
