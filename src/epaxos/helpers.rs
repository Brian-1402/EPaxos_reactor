use crate::common::{ClientResponse, Command, CommandResult, EMsg, Instance};
use crate::epaxos::{CmdEntry, CmdStatus, Processor};
use core::panic;
use std::collections::{HashMap, HashSet, VecDeque};

#[cfg(debug_assertions)]
use tracing::info;
// use tracing::{error};

impl Processor {
    /// Calculate the majority size based on the number of replicas. Excludes self.
    /// Simple invariants:
    /// Must be >= 1
    /// majority + 1 should have intersection with another majority. Hence 2*(majority+1)
    pub fn get_majority(&self) -> u32 {
        let res = (self.replica_list.len() / 2) as u32;
        if res < 1 { 1 } else { res }
    }

    /// Calculate the fast quorum size based on the number of replicas. Excludes self.
    /// Simple invariants:
    /// Must be >= majority
    /// Must be >= 1
    pub fn fast_quorum(&self) -> u32 {
        let res = (self.replica_list.len() as i32) - 2;
        if res < 1 { 1 } else { res as u32 }
    }
    // for given new size and replica, increase the cmds[replica] vector to that size with empty values in extra slots
    pub fn resize_cmds(&mut self, new_size: usize, replica: &String) {
        let cmds_for_replica = self.cmds.get_mut(replica).expect("replica not found");
        let current_size = cmds_for_replica.len();
        if new_size > current_size {
            cmds_for_replica.resize_with(new_size, || None);
        }
    }

    /// Insert a CmdEntry into cmds at the position specified by instance
    /// Overwrites if the position is empty or has the same command
    /// Panics if the position is already occupied with a different command
    pub fn cmds_insert(&mut self, instance: &Instance, cmd_entry: CmdEntry) {
        let index = instance.instance_num;
        let required_size = index + 1;

        self.resize_cmds(required_size, &instance.replica);

        let cmds_vec = self
            .cmds
            .get_mut(&instance.replica)
            .expect("Internal error: Replica vector should exist after initialization.");

        let position = cmds_vec
            .get_mut(index)
            .expect("Index should be valid after resize_cmds was called.");

        match position {
            None => {
                *position = Some(cmd_entry);
            }
            Some(existing) => {
                // check if the existing command is same as cmd_entry
                if existing.cmd == cmd_entry.cmd {
                    // replace existing entry in case other fields (seq, deps, status) have changed
                    // do checks, that the seq and deps of the new entry are >= existing
                    // if cfg!(debug_assertions) {
                    //     if cmd_entry.seq < existing.seq {
                    //         panic!(
                    //             "{}: {} slot - trying to insert cmd lesser seq: existing seq: {:?}, new seq: {:?}",
                    //             self.replica_name, instance, existing.seq, cmd_entry.seq
                    //         );
                    //     }
                    //     if !existing.deps.is_subset(&cmd_entry.deps) {
                    //         panic!(
                    //             "{}: {} slot - trying to insert cmd with missing dep: existing dep: {:?}, new deps: {:?}",
                    //             self.replica_name, instance, existing.deps, cmd_entry.deps
                    //         );
                    //     }
                    // }
                    *position = Some(cmd_entry);
                } else if cfg!(debug_assertions) {
                    panic!(
                        "{}: {} slot - occupied with different cmd: existing: {:?}, new: {:?}",
                        self.replica_name, instance, existing.cmd, cmd_entry.cmd
                    );
                }
            }
        }
    }

    /// Used to get deps of a given cmd entry
    /// Iterates through all CmdInstance present in cmds for all replicas, and if key is same,
    /// add it to cmd_entry deps
    pub fn get_interfs(&self, cmd: &Command) -> (HashSet<Instance>, u64) {
        let mut deps = HashSet::new();
        let mut max_seq = 0;

        let is_read = matches!(cmd, Command::Get { .. });

        for (r, cmds_vec) in &self.cmds {
            for (i, cmd_opt) in cmds_vec.iter().enumerate() {
                if let Some(c) = cmd_opt {
                    if matches!(c.cmd, Command::Get { .. }) {
                        continue;
                    }

                    // Skip commands that are already executed
                    if matches!(c.status, CmdStatus::Executed) {
                        continue;
                    }

                    let entry_is_read = matches!(c.cmd, Command::Get { .. });

                    // RULE:
                    // - If incoming command is READ, ignore READ dependencies.
                    // - If incoming command is WRITE, include both READ and WRITE deps.
                    if is_read && entry_is_read {
                        continue; // READ should not depend on READ
                    }

                    if c.cmd.conflicts_with(cmd) {
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
        (deps, max_seq)
    }

    fn build_dep_graph(&self, root: &Instance) -> HashMap<Instance, Vec<Instance>> {
        let mut graph = HashMap::<Instance, Vec<Instance>>::new();
        let mut stack = vec![root.clone()];
        let mut visited = HashSet::<Instance>::new();

        while let Some(inst) = stack.pop() {
            if visited.contains(&inst) {
                continue;
            }
            visited.insert(inst.clone());

            let entry_opt = self
                .cmds
                .get(&inst.replica)
                .and_then(|v| v.get(inst.instance_num))
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

    fn tarjan_scc(&self, graph: &HashMap<Instance, Vec<Instance>>) -> Vec<Vec<Instance>> {
        // Standard Tarjan SCC implementation
        // I give the full working version below:

        let mut index = 0;
        let mut stack = Vec::<Instance>::new();
        let mut on_stack = HashSet::<Instance>::new();
        let mut indices = HashMap::<Instance, i32>::new();
        let mut lowlink = HashMap::<Instance, i32>::new();
        let mut result = Vec::<Vec<Instance>>::new();

        #[allow(clippy::too_many_arguments)]
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
                            w.clone(),
                            index,
                            stack,
                            on_stack,
                            indices,
                            lowlink,
                            graph,
                            result,
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
                    if w == v {
                        break;
                    }
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

    fn topo_sort_scc(
        &self,
        // sccs: &Vec<Vec<Instance>>,
        sccs: &[Vec<Instance>],
        graph: &HashMap<Instance, Vec<Instance>>,
    ) -> Vec<Vec<Instance>> {
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
        // for u in 0..dag.len() {
        // for &v in &dag[u] {
        for dag_u in &dag {
            for &v in dag_u {
                indeg[v] += 1;
            }
        }

        let mut q = VecDeque::new();
        // for i in 0..sccs.len() {
        // if indeg[i] == 0 {
        for (i, element) in indeg.iter().enumerate().take(sccs.len()) {
            if *element == 0 {
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
            .and_then(|cmds| cmds.get(instance.instance_num)) // Get the command entry at the given instance number
            .and_then(|opt| opt.as_ref()) // Unwrap the Option<CmdEntry> to get a reference to CmdEntry
    }

    pub fn mark_executed(&mut self, instance: &Instance) {
        // Locate the command entry in the cmds log
        if let Some(Some(cmd_entry)) = self
            .cmds
            .get_mut(&instance.replica)
            .and_then(|cmds| cmds.get_mut(instance.instance_num))
        {
            // Set the status to Executed
            cmd_entry.status = CmdStatus::Executed;
        } else {
            // If the command entry does not exist, log an error or handle appropriately
            panic!("Command not found in log for instance: {:?}", instance);
        }
    }

    // precondition: all dependencies are either committed or executed
    pub fn execute_cmd(&mut self, root: &Instance) -> Vec<EMsg> {
        let mut out = Vec::new();

        // Build dependency graph
        let graph = self.build_dep_graph(root);

        // Find SCCs
        let sccs = self.tarjan_scc(&graph);

        // topo order
        let order = self.topo_sort_scc(&sccs, &graph);

        // Reverse order
        for mut sorted in order.into_iter().rev() {
            // Execute SCC in seq order
            // let mut sorted = scc.clone();
            sorted.sort_by_key(|inst| self.lookup(inst).unwrap().seq);

            for inst in sorted {
                if let Some(entry) = self.lookup(&inst) {
                    if matches!(entry.status, CmdStatus::Executed) {
                        continue;
                    }

                    match entry.cmd.clone() {
                        Command::Set { key, val } => {
                            #[cfg(debug_assertions)]
                            info!("{}: Write executed for {}", self.replica_name, inst);

                            self.data.insert(key, val);
                            self.mark_executed(&inst);
                        }
                        Command::Get { key } => {
                            // Check if the current replica is the command leader for this read
                            if inst.replica != self.replica_name {
                                continue; // Skip processing if not the command leader
                            }
                            let val = self.data.get(&key).cloned();
                            let meta = &self.app_meta[inst.instance_num];
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
                    read_entry.deps.contains(write_instance)
                } else {
                    false
                }
            })
            .cloned() // Clone the instances to return owned values
            .collect()
    }

    pub fn deps_all_ready(&self, inst: &Instance) -> bool {
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
    pub fn handle_pending_reads(&mut self, instance: &Instance) -> Vec<EMsg> {
        let mut out_msgs = Vec::new();

        let pending_reads_on_write = self.get_pending_reads(instance);
        if pending_reads_on_write.is_empty() {
            // No reads waiting: normal commit
            #[cfg(debug_assertions)]
            info!(
                "{}: No pending reads on write {}, proceeding with commit",
                self.replica_name, instance
            );
            return vec![];
        }

        #[cfg(debug_assertions)]
        info!(
            "{}: Found {} pending reads on write {}",
            self.replica_name,
            pending_reads_on_write.len(),
            instance
        );

        // If pending reads present, check if all deps of write cmd are ready. If no, return empty vec
        // Means the write cmd itself is waiting on other write cmds to commit/execute
        if !self.deps_all_ready(instance) {
            #[cfg(debug_assertions)]
            info!(
                "{}: Write {} has pending dependencies, cannot execute pending reads yet",
                self.replica_name, instance
            );
            return vec![];
        }
        // out_msgs = self.execute_cmd(instance.clone());
        self.execute_cmd(instance);

        #[cfg(debug_assertions)]
        info!(
            "{}: Executed write {}, checking {} pending reads",
            self.replica_name,
            instance,
            pending_reads_on_write.len()
        );

        // Re-check each pending read, execute if ready
        for read_inst in pending_reads_on_write {
            if self.deps_all_ready(&read_inst) {
                let mut exec_out = self.execute_cmd(&read_inst);
                out_msgs.append(&mut exec_out);
                #[cfg(debug_assertions)]
                info!(
                    "{}: Executed pending read {} after write {}",
                    self.replica_name, read_inst, instance
                );
            }
        }
        out_msgs
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::{Command, Variable};
    use crate::epaxos::{CmdEntry, CmdStatus};

    // --- Helpers ---

    fn mock_processor(peers: usize) -> Processor {
        let mut list = vec![];
        for i in 0..peers {
            list.push(format!("r{}", i));
        }
        // "r0" is assumed to be self
        Processor::new(list, "r0".to_string())
    }

    fn mock_cmd(key: &str) -> Command {
        Command::Set {
            key: Variable { name: key.into() },
            val: "val".into(),
        }
    }

    // --- Tests ---
    #[test]
    fn test_quorum_calculations() {
        let cases = vec![
            // (peers, expected_majority, expected_fast_quorum)
            (1, 1, 1), // Edge case: 1 node
            (3, 1, 1), // 3 nodes: maj=1, fq=1
            (5, 2, 3), // 5 nodes: maj=2, fq=3
            (7, 3, 5), // 7 nodes: maj=3, fq=5
        ];

        for (peers, exp_maj, exp_fq) in cases {
            let p = mock_processor(peers);
            assert_eq!(
                p.get_majority(),
                exp_maj,
                "Majority mismatch for peers={}",
                peers
            );
            assert_eq!(
                p.fast_quorum(),
                exp_fq,
                "Fast Quorum mismatch for peers={}",
                peers
            );
        }
    }

    #[test]
    fn test_resize_cmds() {
        let mut p = mock_processor(3);
        let target_replica = "r1".to_string();

        // Pre-condition: Empty
        let len_before = p.cmds.get(&target_replica).unwrap().len();
        assert_eq!(len_before, 0);

        // Action: Resize to 5
        p.resize_cmds(5, &target_replica);

        // Assert: Vector resized, padded with None
        let vec = p.cmds.get(&target_replica).unwrap();
        assert_eq!(vec.len(), 5);
        assert!(vec[4].is_none());

        // Action: Resize smaller (should be ignored by implementation logic)
        // helpers.rs logic checks: if new_size > current_size.
        // If new_size < current_size, it does nothing.
        p.resize_cmds(2, &target_replica);
        assert_eq!(p.cmds.get(&target_replica).unwrap().len(), 5);
    }

    #[test]
    fn test_cmds_insert_normal() {
        let mut p = mock_processor(3);
        let inst = Instance {
            replica: "r1".into(),
            instance_num: 2,
        };

        let entry = CmdEntry {
            cmd: mock_cmd("key1"),
            seq: 10,
            deps: HashSet::new(),
            status: CmdStatus::PreAccepted,
        };

        p.cmds_insert(&inst, entry);

        let vec = p.cmds.get("r1").unwrap();
        // Should have auto-resized to accommodate index 2 (size 3)
        assert!(vec.len() >= 3);

        let stored = vec[2].as_ref().expect("Slot should be occupied");
        assert_eq!(stored.seq, 10);
        assert!(matches!(stored.status, CmdStatus::PreAccepted));
    }

    #[test]
    fn test_cmds_insert_update_existing() {
        let mut p = mock_processor(3);
        let inst = Instance {
            replica: "r1".into(),
            instance_num: 1,
        };

        // 1. Initial insert
        let cmd = mock_cmd("key1");
        let mut entry = CmdEntry {
            cmd: cmd.clone(),
            seq: 10,
            deps: HashSet::from([Instance {
                replica: "r2".into(),
                instance_num: 0,
            }]),
            status: CmdStatus::PreAccepted,
        };
        p.cmds_insert(&inst, entry.clone());

        // 2. Valid Update: Higher Seq, Superset Deps, Status Change
        entry.seq = 20;
        entry.deps.insert(Instance {
            replica: "r3".into(),
            instance_num: 0,
        });
        entry.status = CmdStatus::Accepted;

        p.cmds_insert(&inst, entry);

        // 3. Verify Update Persisted
        let stored = p.cmds.get("r1").unwrap()[1].as_ref().unwrap();
        assert_eq!(stored.seq, 20);
        assert_eq!(stored.deps.len(), 2);
        assert!(matches!(stored.status, CmdStatus::Accepted));
    }

    // #[test]
    // #[should_panic(expected = "lesser seq: existing seq")]
    fn _test_cmds_insert_panic_lower_seq() {
        let mut p = mock_processor(3);
        let inst = Instance {
            replica: "r1".into(),
            instance_num: 0,
        };
        let cmd = mock_cmd("key1");

        // High Seq first
        p.cmds_insert(
            &inst,
            CmdEntry {
                cmd: cmd.clone(),
                seq: 20,
                deps: HashSet::new(),
                status: CmdStatus::PreAccepted,
            },
        );

        // Attempt Low Seq update
        p.cmds_insert(
            &inst,
            CmdEntry {
                cmd,
                seq: 10,
                deps: HashSet::new(),
                status: CmdStatus::PreAccepted,
            },
        );
    }

    // #[test]
    // #[should_panic(expected = "missing dep: existing dep")]
    fn _test_cmds_insert_panic_missing_deps() {
        let mut p = mock_processor(3);
        let inst = Instance {
            replica: "r1".into(),
            instance_num: 0,
        };
        let cmd = mock_cmd("key1");
        let dep_inst = Instance {
            replica: "r2".into(),
            instance_num: 5,
        };

        // Insert with dependencies
        p.cmds_insert(
            &inst,
            CmdEntry {
                cmd: cmd.clone(),
                seq: 10,
                deps: HashSet::from([dep_inst]),
                status: CmdStatus::PreAccepted,
            },
        );

        // Attempt update with empty dependencies (subset violation)
        p.cmds_insert(
            &inst,
            CmdEntry {
                cmd,
                seq: 10,
                deps: HashSet::new(),
                status: CmdStatus::PreAccepted,
            },
        );
    }

    #[test]
    // Updated expectation to match the actual panic message format: "{}: {} slot - occupied..."
    #[should_panic(expected = "slot - occupied")]
    fn test_cmds_insert_panic_on_occupied() {
        let mut p = mock_processor(3);
        let inst = Instance {
            replica: "r1".into(),
            instance_num: 0,
        };

        let entry1 = CmdEntry {
            cmd: mock_cmd("key1"),
            seq: 1,
            deps: HashSet::new(),
            status: CmdStatus::PreAccepted,
        };

        // Fill the slot
        p.cmds_insert(&inst, entry1);

        // Try filling again with a different command (triggers panic in debug mode)
        let entry2 = CmdEntry {
            cmd: mock_cmd("key2"),
            seq: 2,
            deps: HashSet::new(),
            status: CmdStatus::PreAccepted,
        };
        p.cmds_insert(&inst, entry2);
    }

    #[test]
    fn test_get_interfs_logic() {
        let mut p = mock_processor(3);

        // 1. Populate Log with a command on Key "A"
        let inst_a = Instance {
            replica: "r1".into(),
            instance_num: 0,
        };
        let entry_a = CmdEntry {
            cmd: mock_cmd("A"),
            seq: 50,
            deps: HashSet::new(),
            status: CmdStatus::Accepted,
        };
        p.cmds_insert(&inst_a, entry_a);

        // 2. Populate Log with a command on Key "B"
        let inst_b = Instance {
            replica: "r2".into(),
            instance_num: 0,
        };
        let entry_b = CmdEntry {
            cmd: mock_cmd("B"),
            seq: 20,
            deps: HashSet::new(),
            status: CmdStatus::Committed,
        };
        p.cmds_insert(&inst_b, entry_b);

        // Case: Conflict with A
        let cmd_new_a = mock_cmd("A");
        let (deps, seq) = p.get_interfs(&cmd_new_a);

        assert!(deps.contains(&inst_a), "Should depend on r1:0 (Key A)");
        assert!(!deps.contains(&inst_b), "Should NOT depend on r2:0 (Key B)");
        assert_eq!(seq, 51, "Seq should be max(50) + 1");

        // Case: No Conflict
        let cmd_new_c = mock_cmd("C");
        let (deps_c, seq_c) = p.get_interfs(&cmd_new_c);

        assert!(deps_c.is_empty());
        assert_eq!(seq_c, 1, "Default seq should be 0 + 1");
    }
    #[test]
    fn test_get_interfs_max_seq_aggregation() {
        let mut p = mock_processor(3);

        // Insert conflict 1 (Seq 100)
        p.cmds_insert(
            &Instance {
                replica: "r1".into(),
                instance_num: 0,
            },
            CmdEntry {
                cmd: mock_cmd("A"),
                seq: 100,
                deps: HashSet::new(),
                status: CmdStatus::PreAccepted,
            },
        );

        // Insert conflict 2 (Seq 200)
        p.cmds_insert(
            &Instance {
                replica: "r2".into(),
                instance_num: 0,
            },
            CmdEntry {
                cmd: mock_cmd("A"),
                seq: 200,
                deps: HashSet::new(),
                status: CmdStatus::PreAccepted,
            },
        );

        // Check interference
        let (_, seq) = p.get_interfs(&mock_cmd("A"));
        assert_eq!(
            seq, 201,
            "Should define seq based on the maximum conflicting seq (200)"
        );
    }

    // ------------------------------------------------------------------------
    // GRAPH & TOPOLOGY TESTS
    // ------------------------------------------------------------------------

    fn make_inst(r: &str, i: usize) -> Instance {
        Instance {
            replica: r.to_string(),
            instance_num: i,
        }
    }

    // Helper to quickly create entries with specific seq and deps
    fn make_entry(seq: u64, deps: Vec<Instance>) -> CmdEntry {
        CmdEntry {
            cmd: mock_cmd("key"), // command content irrelevant for graph tests
            seq,
            deps: HashSet::from_iter(deps),
            status: CmdStatus::PreAccepted,
        }
    }

    #[test]
    fn test_tarjan_simple_chain() {
        // A -> B -> C
        let mut p = mock_processor(3);
        let inst_a = make_inst("r0", 0);
        let inst_b = make_inst("r1", 0);
        let inst_c = make_inst("r2", 0);

        // A deps on B, B deps on C
        p.cmds_insert(&inst_a, make_entry(10, vec![inst_b.clone()]));
        p.cmds_insert(&inst_b, make_entry(10, vec![inst_c.clone()]));
        p.cmds_insert(&inst_c, make_entry(10, vec![]));

        let graph = p.build_dep_graph(&inst_a);
        let sccs = p.tarjan_scc(&graph);

        // 3 distinct SCCs
        assert_eq!(sccs.len(), 3);
        // Check all are size 1
        for scc in &sccs {
            assert_eq!(scc.len(), 1);
        }
    }

    #[test]
    fn test_tarjan_cycle_detection() {
        // A -> B -> A (Cycle)
        let mut p = mock_processor(2);
        let inst_a = make_inst("r0", 0);
        let inst_b = make_inst("r1", 0);

        p.cmds_insert(&inst_a, make_entry(10, vec![inst_b.clone()]));
        p.cmds_insert(&inst_b, make_entry(10, vec![inst_a.clone()]));

        let graph = p.build_dep_graph(&inst_a);
        let sccs = p.tarjan_scc(&graph);

        // Should result in 1 SCC containing both nodes
        assert_eq!(sccs.len(), 1);
        assert_eq!(sccs[0].len(), 2);
        assert!(sccs[0].contains(&inst_a));
        assert!(sccs[0].contains(&inst_b));
    }

    #[test]
    fn test_tarjan_self_loop() {
        // A -> A
        let mut p = mock_processor(1);
        let inst_a = make_inst("r0", 0);

        p.cmds_insert(&inst_a, make_entry(10, vec![inst_a.clone()]));

        let graph = p.build_dep_graph(&inst_a);
        let sccs = p.tarjan_scc(&graph);

        assert_eq!(sccs.len(), 1);
        assert_eq!(sccs[0].len(), 1);
        assert_eq!(sccs[0][0], inst_a);
    }

    #[test]
    fn test_topo_sort_diamond_structure() {
        // Diamond:
        //    A
        //   / \
        //  B   C
        //   \ /
        //    D
        // A depends on B & C. B and C depend on D.
        // Execution order (reverse topo) should be: D -> {B, C} -> A.

        let mut p = mock_processor(4);
        let inst_a = make_inst("r0", 0);
        let inst_b = make_inst("r1", 0);
        let inst_c = make_inst("r2", 0);
        let inst_d = make_inst("r3", 0);

        p.cmds_insert(
            &inst_a,
            make_entry(10, vec![inst_b.clone(), inst_c.clone()]),
        );
        p.cmds_insert(&inst_b, make_entry(10, vec![inst_d.clone()]));
        p.cmds_insert(&inst_c, make_entry(10, vec![inst_d.clone()]));
        p.cmds_insert(&inst_d, make_entry(10, vec![]));

        let graph = p.build_dep_graph(&inst_a);
        let sccs = p.tarjan_scc(&graph);
        let order = p.topo_sort_scc(&sccs, &graph);

        // Flatten the result
        let flat: Vec<Instance> = order.into_iter().flatten().collect();

        // Topo sort output is [Depender, ..., Dependency]
        // So A should appear before B and C, and B/C before D.

        let pos_a = flat.iter().position(|x| *x == inst_a).unwrap();
        let pos_b = flat.iter().position(|x| *x == inst_b).unwrap();
        let pos_c = flat.iter().position(|x| *x == inst_c).unwrap();
        let pos_d = flat.iter().position(|x| *x == inst_d).unwrap();

        assert!(pos_a < pos_b, "A must come before B in topo order");
        assert!(pos_a < pos_c, "A must come before C in topo order");
        assert!(pos_b < pos_d, "B must come before D in topo order");
        assert!(pos_c < pos_d, "C must come before D in topo order");
    }

    #[test]
    fn test_execution_logic_seq_breaks_cycles() {
        // Cycle: A <-> B
        // A has Seq=100, B has Seq=50.
        // EPaxos rule: Execute lowest Seq first within an SCC.
        // Expected Execution: B (50) then A (100).

        let mut p = mock_processor(2);
        let inst_a = make_inst("r0", 0);
        let inst_b = make_inst("r1", 0);

        p.cmds_insert(&inst_a, make_entry(100, vec![inst_b.clone()]));
        p.cmds_insert(&inst_b, make_entry(50, vec![inst_a.clone()]));

        let graph = p.build_dep_graph(&inst_a);
        let sccs = p.tarjan_scc(&graph);

        // We manually verify the sort logic used inside execute_cmd
        // (Since we can't easily spy on the internal loop of execute_cmd)
        let mut scc_members = sccs[0].clone();
        assert_eq!(scc_members.len(), 2);

        // Mimic the sort logic in execute_cmd:
        scc_members.sort_by_key(|inst| p.lookup(inst).unwrap().seq);

        assert_eq!(
            scc_members[0], inst_b,
            "Instance B (seq 50) should execute first"
        );
        assert_eq!(
            scc_members[1], inst_a,
            "Instance A (seq 100) should execute second"
        );
    }

    #[test]
    fn test_topo_sort_disconnected_components() {
        // A -> B
        // C -> D
        // Disconnected graph logic.
        let p = mock_processor(0);
        let inst_a = make_inst("r0", 0);
        let inst_b = make_inst("r0", 1);
        let inst_c = make_inst("r0", 2);
        let inst_d = make_inst("r0", 3);

        // Manually constructing graph to bypass traversal limits of build_dep_graph(root)
        let mut graph = HashMap::new();
        graph.insert(inst_a.clone(), vec![inst_b.clone()]);
        graph.insert(inst_b.clone(), vec![]);
        graph.insert(inst_c.clone(), vec![inst_d.clone()]);
        graph.insert(inst_d.clone(), vec![]);

        let sccs = p.tarjan_scc(&graph);
        assert_eq!(sccs.len(), 4);

        let order = p.topo_sort_scc(&sccs, &graph);
        let flat: Vec<Instance> = order.into_iter().flatten().collect();

        let pos_a = flat.iter().position(|x| *x == inst_a).unwrap();
        let pos_b = flat.iter().position(|x| *x == inst_b).unwrap();
        let pos_c = flat.iter().position(|x| *x == inst_c).unwrap();
        let pos_d = flat.iter().position(|x| *x == inst_d).unwrap();

        // Dependency order preserved locally?
        assert!(pos_a < pos_b);
        assert!(pos_c < pos_d);
    }
}
