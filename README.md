# Setup

## Run commands
- `make build`: Builds the project
- `make node`: Runs the node http server at port 3000. Builds the project as well
- `make job`: (after make node) Runs job controller to load the epaxos library, connect to node and starts epaxos

## CI/CD check commands
```bash
# Check formatting issues
cargo fmt --all -- --check

# Run clippy (linter) for post-compilation code suggestions and fixes
cargo clippy -- -D warnings
```

### Auto-fix commands for the above checks
```bash 
# Format entire codebase
cargo fmt --all

# Attempts to fix the issues suggested by clippy earlier (doesn't do all the fixes)
cargo clippy --fix --allow-dirty --allow-staged -- -A warnings
```

## Current project status
- For now just made a single server responding to reads

# System design

## Message types

- Client messages
  - WriteRequest
  - WriteResponse
  - ReadRequest
  - ReadResponse
  - RequestAndRead

- Base server messages
  - PreAccept
  - PreAcceptOk
  - Accept
  - AcceptOk
  - Commit

- Failure messages
  - Prepare
  - PrepareOk

Whats left:
- does execution require message type?

## Server state variables

### `cmds`
- 2d array - [replica][instance_num]
- Struct fields for each array element:
  - Command ($\gamma$) - The write operation
  - Sequence number `seq`
    - To break cyclic dependencies
    - ???
  - Dependency list `deps`
    - List of (replica,instance) pairs whose commands interferes with gamma
  - Status - variants:
    - `PreAccepted`
    - `Accepted`
    - `Committed`
  -`quorum_ctr`
    - extra, not specified in paper
    - Counter for how many messages received for each phase
    - Used for PreAcceptOk, AcceptOk
  -`leader`
    - extra, not specified in paper
    - optional, could remove for release
    - who is the leader L for this command

### `instance_num`
- Counter which stores latest instance number
- Incremented each time I get a write request

### `server_list`
- List of all active servers

## Actor initialization variables

> What to put in payload in toml file

## Message behaviors

Template: For each message:

- Message payload
- What state variables are being modified
- What is the output message
- Where to send output message (any state required for this)
- Invariants

### Base server receiving messages

#### WriteRequest

- Payload:

  - command ($\gamma$)

- State modifications:
  - Local variables:
    - `Interf` - set of instances whose commands interfere with the current command
    - `seq` - update (??)
    - `deps` - update (??)
    - `L` - myself, the command leader replica for this command
  - `instance_num` ($i_L$) (?) - increment
  - `cmds\[L\][$i_L$]` - ($\gamma$, `seq`, `deps`, `PreAccepted`, 0, `L`)

- What is the output:  
  - PreAccept
    payload: ($\gamma$, `seq`, `deps`, `L`, `instance_num`)
- Where to send:  
    Broadcast to all servers
    - Required state
- Invariants:
  - ??

#### PreAccept

- Payload:
  - command ($\gamma$)
  - sequence number `seq`
  - dependency list `deps`
  - command leader `L`
  - instance number at leader `i_L`

- State modifications:
  - Local variables:
    - `seq` - update (??)
    - `deps` - update (??)
    - `L` - where msg came from (?? Does reactor provide this value?)
  - `instance_num` ($i_L$) (?) - increment
  - `cmds\[L\][$i_L$]` - ($\gamma$, `seq`, `deps`, `PreAccepted`, 0, `L`)
- Output:
  - `PreAcceptOk`
    payload: ($\gamma$, `seq`, `deps`, `L`, `instance_num`)
    - here, can avoid sending $\gamma$. Leader already has it
- Where to send:
  - Command leader `L`, where `PreAccept` came from
- Invariants:
  - ??


#### PreAcceptOk
- Payload:
  - command ($\gamma$)
  - sequence number `seq`
  - dependency list `deps`
  - command leader `L`
  - instance number at leader `i_L`

- Code Logic:
  - Phase 1
    - Local variables:
      - `L` - myself, the command leader
      - `seq` - check eq with `cmd\[L\][$i_L$].seq`
      - `deps` - check eq with `cmd\[L\][$i_L$].deps`
      
    - Check if the msg hasn't already been accepted or committed else ignore
    - If eq:
      - `cmds\[L\][$i_L$]` - ($\gamma$, `seq`, `deps`, `PreAccepted`, ctr+1, `L`)
    - If not eq (else):
      - Update `seq` - max()
      - Update `deps` - union()
      - `cmds\[L\][$i_L$]` - ($\gamma$, `seq`, `deps`, `Accepted`, ctr+1, `L`)
    - If ctr == floor(n/2) 
      - if no conflicts seen so far (cmds shows `PreAccepted`):
        - go to Commit Phase
      - if conflicts seen (cmds shows `Accepted`):
        - go to Phase 2
  - Commit phase
    - Fast path quorum floor(n/2) reached 
      `cmds\[L\][$i_L$]` - ($\gamma$, `seq`, `deps`, `Committed`, ctr+1, `L`)
    - Output:
      - `Commit`
        payload: ($\gamma$, `seq`, `deps`, `L`, `instance_num`)
    - Where to send:
      - Broadcast to every replica

  - Phase 2
    - Output:
      - `Accept`
        payload: ($\gamma$, `seq`, `deps`, `L`, `instance_num`)
    - Where to send:
      Broadcast to every replica
  
#### Accept

#### AcceptOk

#### Commit
- Payload 
  - command ($\gamma$)
  - sequence number `seq`
  - dependency list `deps`
  - command leader `L`
  - instance number at leader `i_L`
- State modifications:
  `cmds\[L\][$i_L$]` - ($\gamma$, `seq`, `deps`, `Committed`, 0, `L`)
- Invariants:
  - ??

- Invariants:
  - Leader should receive PreAcceptOk only after it processed and sent PreAccept for the same command to others
    - I am the command leader for this instance, can't get the PreAcceptOk of some other command
  - In fast path: Leader cannot see a new PreAcceptOk for this command with conflicts after already seeing majority and proceeding to next phase.
  - In slow path: Leader cannot see a new PreAcceptOk for this command with *newer* conflicts after already seeing majority and proceeding to next phase. We can move forward to phase 2 after majority is formed.
    - Logic: Thats the guarantee of majority quorum intersection
    - Need to walk through a scenario to confirm this
- Assumptions:
  - The leader proceeds to the next phase (Accept or Commit) after receiving PreAcceptOk from majority


## Paper variables

- Q - replica
- Q.i - instance i of replica Q
- 


### Paper ambiguities
- When waiting for PreAcceptOk for majority, 
  - Should checking for conflicts be done for fast path quorum?
  Or should we check for *all* responses received?
  - Rephrasing: should we move on immmediately after we get fast path quorum or wait for all responses?
    - What do we do for later responses if we move on?
- In the algorithm they mention sending gamma even in the "Ok" messages (PreAcceptOk, AcceptOk)
  - But leader already has gamma, so is it necessary?
  - Maybe for failure recovery?
  
### Paper deviations
- We store quorum counter and command leader in cmds state.
- When receiving PreAcceptOk,
  - If we see any conflict, before quorum itself, we update state to `Accepted` prematurely before going to Phase 2 section in paper algorithm. Because presence of conflicts ensures we will go to phase 2 anyway.
  


## To discuss
- what are the commands we support.