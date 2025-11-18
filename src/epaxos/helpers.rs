use crate::common::{Command, Instance};
use crate::epaxos::{CmdEntry, Processor};
use core::panic;
use std::collections::HashSet;

// use tracing::{error};

impl Processor {
    pub fn get_majority(&self) -> u32 {
        let res = (self.replica_list.len() / 2) as u32;
        if res < 1 { 1 } else { res }
    }

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

    pub fn cmds_insert(&mut self, instance: &Instance, cmd_entry: CmdEntry) {
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
            Some(existing) => {
                // check if the existing command is same as cmd_entry
                if existing.cmd == cmd_entry.cmd {
                    // replace existing entry in case other fields (seq, deps, status) have changed
                    *position = Some(cmd_entry);
                } else {
                    panic!(
                        "Slot occupied with different cmd: existing: {}:{:?}, new: {}:{:?}",
                        self.replica_name, existing.cmd, instance.replica, cmd_entry.cmd
                    );
                }
            }
        }
    }
    // used to get deps of a given cmd entry
    // iterates through all CmdInstance present in cmds for all replicas, and if key is same,
    // add it to cmd_entry deps
    pub fn get_interfs(&self, cmd: &Command) -> (HashSet<Instance>, u64) {
        let mut deps = HashSet::new();
        let mut max_seq = 0;

        for (r, cmds_vec) in &self.cmds {
            for (i, cmd_opt) in cmds_vec.iter().enumerate() {
                if let Some(c) = cmd_opt
                    && c.cmd.conflicts_with(cmd)
                {
                    deps.insert(Instance {
                        replica: r.clone(),
                        instance_num: i,
                    });
                    // Update max_seq with the maximum seq value from the dependency
                    max_seq = max_seq.max(c.seq);
                }
            }
        }

        max_seq += 1; // incrementing max_seq before returning
        (deps, max_seq)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::{Command, Variable};
    use crate::epaxos::{CmdEntry, CmdStatus}; // Ensure CmdStatus is visible

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

        // Action: Resize smaller (should be ignored/no-op for Vec::resize_with if len > new_len?)
        // Actually helpers.rs logic checks: if new_size > current_size.
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
    #[should_panic(expected = "Slot occupied")]
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

        // Try filling again
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
}
