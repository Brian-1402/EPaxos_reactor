# Setup

## Run commands
- `make build`: Builds the project
- `make node`: Runs the node http server at port 3000. Builds the project as well
- `make job`: (after make node) Runs job controller to load the epaxos library, connect to node and starts epaxos
- `make pre_commit`: Runs all the CI/CD checks (formatting, linting, etc)

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

### `instance_num`
- Counter which stores latest instance number
- Incremented each time I get a write request

### `server_list`
- List of all active servers

> New possible additions

### `quorum_ctr`
- 1d vector indexed by instance number. stores int counter for quorum checking

### `app_meta`
> Actually this might be needed in cmds. In explicit prepare, other replicas reply to client on behalf of failed leader
- Application layer metadata for each instance. 1d vector indexed by instance number
- consists of:
  - client_id - which client sent this msg
  - msg_id - assigned by client

## Intermediate data types
- Instance
  - fields: 2
    - replica
    - inst_num
  - Used in Interf set and other places

- Command
  - variants:
    - Set(id, var, val) (Write operation)
    - Get(id, var) (Read operation)
    - any other commands we're gonna support
- Response
  - variants:
    - Set(id, var) (Write response)
    - Get(id, var, val) (Read response)
    - any other commands we're gonna support

## Actor initialization variables

> What to put in payload in toml file
- replica name - my own name
- list of all replicas names


## Message behaviors

Template: For each message:

- Message payload
- What state variables are being modified
- What is the output message
- Where to send output message (any state required for this)
- Invariants

### Base server receiving messages

#### ReadRequest
- similar to write request, but once we reach commit phase for this read command, we run the execution algorithm

#### WriteRequest

##### Payload

- command ($\gamma$)
- client_id
- msg_id

##### State modifications
- Local variables:
  - `Interf` - set of instances whose commands interfere with the current command
  - `seq` - update (??)
  - `deps` - update (??)
  - `L` - myself, the command leader replica for this command
- `instance_num` ($i_L$) (?) - increment
- cmds\[L\]\[$i_L$\] - ($\gamma$, `seq`, `deps`, `PreAccepted`, 0)

##### What is the output
- PreAccept
  payload: ($\gamma$, `seq`, `deps`, $Inst(L,i_L)$)
##### Send to
- Atleast fast quorum minus self ($F \setminus L$)
- For simplicity, broadcast to all servers
##### Invariants
- ??

#### PreAccept

##### Payload
- command ($\gamma$)
- sequence number `seq`
- dependency list `deps`
- Instance $(L, i_L)$

##### State modifications
- Local variables:
  - `seq` - update (??)
  - `deps` - update (??)
- `instance_num` ($i_L$) (?) - increment
- `cmds\[L\]\[$i_L$\]` - ($\gamma$, `seq`, `deps`, `PreAccepted`, 0)

##### Output
- `PreAcceptOk`
  payload: ($\gamma$, `seq`, `deps`, $Inst(L,i_L)$)
  - here, can avoid sending $\gamma$. Leader already has it

##### Send to
- Command leader `L`, where `PreAccept` came from
##### Invariants
- ??


#### PreAcceptOk
##### Payload
- command ($\gamma$)
- sequence number `seq`
- dependency list `deps`
- Instance $(L, i_L)$
- command leader `L`
- instance number at leader `i_L`

##### Phase 1
- Local variables:
  - `L` - myself, the command leader
  - `seq` - check eq with cmds\[L\]\[$i_L$\].`seq`
  - `deps` - check eq with cmds\[L\]\[$i_L$\].`deps`
  
- Check if the msg hasn't already been accepted or committed else ignore
- If eq:
  - cmds\[L\]\[$i_L$\] - ($\gamma$, `seq`, `deps`, `PreAccepted`, ctr+1)
- If not eq (else):
  - Update `seq` - max()
  - Update `deps` - union()
  - cmds\[L\]\[$i_L$\] - ($\gamma$, `seq`, `deps`, `Accepted`, ctr+1)
- If ctr == $\lfloor N / 2 \rfloor$ 
  - if conflicts seen (cmds shows `Accepted`):
    - go to Phase 2
  - if no conflicts seen so far (cmds shows `PreAccepted`):
    - wait for getting fast quorum
- If ctr == fast quorum $F\setminus L$
  - if no conflicts seen so far (cmds shows `PreAccepted`):
    - go to Commit phase
  - if conflicts seen (cmds shows `Accepted`):
    - Not possible, quorum intersection invariant violated, panic

##### Commit phase
- Fast path quorum $F\setminus L$ reached
  cmds\[L\]\[$i_L$\] - ($\gamma$, `seq`, `deps`, `Committed`, ctr+1)
- Output:
  - `Commit`
    payload: ($\gamma$, `seq`, `deps`, $Inst(L,i_L)$)
    - Where to send:
      - Broadcast to every replica
  - WriteResponse
    payload:

##### Phase 2 - Paxos-Accept
- Output:
  - `Accept`
    payload: ($\gamma$, `seq`, `deps`, $Inst(L,i_L)$)
- Where to send:
  Broadcast to every replica
  
##### Invariants
- Any messages we get after we've moved on to a phase (commit or accept) should not have any new conflicts. 
  - Proof: Through quorum intersection, since every previous *committed* conflicting command involves atleast a quorum, 
    all those quorums for each command should intersect with the current quorum. So through a quorum we must've seen all possible
    conflicting commands
- Leader should receive PreAcceptOk only after it processed and sent PreAccept for the same command to others
  - I am the command leader for this instance, can't get the PreAcceptOk of some other command
- In fast path: Leader cannot see a new PreAcceptOk for this command with conflicts after already seeing majority and proceeding to next phase.
- In slow path: Leader cannot see a new PreAcceptOk for this command with *newer* conflicts after already seeing majority and proceeding to next phase. We can move forward to phase 2 after majority is formed.
  - Logic: Thats the guarantee of majority quorum intersection
  - Need to walk through a scenario to confirm this
##### Assumptions
  - The leader proceeds to the next phase (Accept or Commit) after receiving PreAcceptOk from majority

#### Accept

##### Payload
- command ($\gamma$)
- sequence number `seq`
- dependency list `deps`
- Instance $(L, i_L)$

##### State modifications
- `cmds\[L\]\[$i_L$\]` - ($\gamma$, `seq`, `deps`, `Accepted`, 0)
##### Output
- `AcceptOk`
  payload: ($\gamma$, $Inst(L,i_L)$)
##### Send to
- L, where it came from

#### AcceptOk

##### Payload
- command ($\gamma$)
- Instance $(L, i_L)$
> We run commit phase
##### State modifications
  cmds\[L\]\[$i_L$\] - ($\gamma$, `seq`, `deps`, `Committed`, 0)
##### Output
  - `Commit`
    payload: ($\gamma$, `seq`, `deps`, $Inst(L,i_L)$)
##### Send to
  - Broadcast to every replica

#### Commit
##### Payload 
  - command ($\gamma$)
  - sequence number `seq`
  - dependency list `deps`
  - command leader `L`
  - instance number at leader `i_L`
##### State modifications:
- cmds\[L\]\[$i_L$\] - ($\gamma$, `seq`, `deps`, `Committed`, 0, `L`)
##### Additional code logic
- If there's a read request whose leader myself R is waiting for this commit, continue the execution algorithm and if need to wait for
  another commit, then move on. Otherwise, if execution is finished, give read response to client
##### Output
- possibly read response
##### Invariants:
  - ??



## Paper variables

- Q - replica
- Q.i - A single instance, whose command leader is replica Q, and it's corresponding instance number i
- 


### Paper ambiguities
- When waiting for PreAcceptOk for majority, 
  - Should checking for conflicts be done for fast path quorum? Ans: Yes. if conflict, go slow path
  Or should we check for *all* responses received? Ans: No, only for quorum
  - Rephrasing: should we move on immmediately after we get fast path quorum or wait for all responses? Ans: Move on
    - What do we do for later responses if we move on? Ans: Ignore. quorum intersection guarantees we won't see any new useful information
      from new replies
- In the algorithm they mention sending gamma even in the "Ok" messages (PreAcceptOk, AcceptOk)
  - But leader already has gamma, so is it necessary?
  - Maybe for failure recovery?
- For quorum sizes, In the algo, usage of "waiting for $\lfloor N / 2 \rfloor$ Ok responses" probably means excluding the self from that count. so in
  total, $\lfloor N / 2 \rfloor$ + 1 replicas are aware of the same info. so quorum intersection is guaranteed.
  - ASSUMPTION: We are actively assuming that the self is excluded in such a count. that a replica doesn't send PreAccept/PreAcceptOk
    messages to itself.
  
### Paper deviations
- We store quorum counter and command leader in cmds state.
- When receiving PreAcceptOk,
  - If we see any conflict, before quorum itself, we update state to `Accepted` prematurely before going to Phase 2 section in paper algorithm. Because presence of conflicts ensures we will go to phase 2 anyway.
  

## Possible optimizations
- in all the \*Ok messages, no need to send $\gamma$.


## To discuss
- what are the commands we support.
- walk through and define how conflicts work. I expect instance numbers to conflict between two command leaders

- In what situations do we have deps conflict instance where cmds shows blank for that instance? I know this conflicts with me but I don't
  have it's value yet to execute. It's possible anyways, the paper talks about it.
  - When some of the commit message broadcasts fail. So there may be a set of replicas who do not have a particular committed command. Now
    suppose one such replica received a request. As the command leader doing preaccept, in one of the preacceptok responses I'll see that
    there is a conflict (and hence do slow path). But the preacceptok response will only have the conflicting instance, not the command
    stored in it. And I can't get it from my local store of cmds either because as established earlier, I missed that commit message of the
    earlier command broadcast, so I do not have it stored with me.
    - If commit broadcast guarantees that everyone receives it, we will never have missing values: true or false?
      - true, because for each committed instance, everyone will receive that value eventually.
      - false if you're considering message delays. Then yes in that time frame before commit msg reaches everyone, we can have missing
        values

### About conflicts and executions
- Need to define clearly what is a conflict.
  - Is it when different commands on same instance num for diff replica? No
  - Is it two commands interacting with the same state variable? regardless of which instance,replica? Yes
  - When looking for previous conflicting instances, do we consider only *committed* instances only or incomplete(pre-accept/accept) requests as well?
    - In execution algorithm it mentions "waiting for R.i to be committed". This implies that dependencies are formed on instances before
      they are even committed.
- When are conflicts considered to be resolved? After resolved (in the execute phase perhaps), are they updated in the state to no longer cause
  conflicts for newer commands?
  - Theoretically for a limited set of keys, after enough requests, every single new request afterwards will have atleast one conflicting
    older instance and hence be forced to go to slow path. This is clearly not ideal. So must understand how conflicts are "cleared".
    - CLARIFICATION: There's a misunderstanding that existence of conflicts/dependencies are what forces slow path. Which is wrong. Finding
      previous conflicts/dependencies within command leader storage is fine actually. Only when we find *new* conflicts that leader hasn't
      seen before, do we go for slow path.
- Instance numbers give a clear ordering. Does this mean conflicts are only relevant if they have same instance number? (diff replicas)

### More invariants to test/consider
- For any set of interfering instances, are their sequence numbers unique and continuously increasing?



## Realizations and clarifications
- Instance number is local to each replica. By itself it has no global cross-server meaning. So you can't use it as some kind of ordering
  number
  - For example, every replica starts with inst num 0. R1 gets command, its set as inst num 1, sends to everyone, fast path, committed, all
    good. Now if R2 gets a new command, its inst num is still 1. Just that commands are assigned via (Replica, inst_num) pairs, not with
    inst num alone. So it's totally normal and expected in cmds table to have different commands in the same inst num column.
  - Situation to support why this is good:
    - If you forced instance num to be unique for every command received by the system as a whole, if there are two concurrent writes
      happening with same inst num (possible with two diff replicas getting req at the same time), then one is bound to fail/rejected, and
      that write request is considered to fail. So in all scenarios of concurrent requests, we'll get a lot of write failures which is bad.
- For $Interf_{L,\gamma}$, definition is: Set of instances $Q.j$ such that commands in `cmds_L[Q][j]` inferferes with $\gamma$. 
  - Note that the definition has nothing to do with the current command leader L's instance num $i_L$. So we literally do not care about any
    positioning in the cmds state. instance number being same has nothing to do with interference, which we already know as stated before.
  - How to find interfering commansd
  - 
- What conflicts mean, case by case:
  - For concurrent writes, if the commands arent semantically linked (totally different state variables) or just literally commutative, we
    don't have to care. Both are non-conflicting writes and we can commit both in fast path without issues. Ordering we don't have to care.



