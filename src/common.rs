use bincode::{Decode, Encode};
use reactor_macros::{DefaultPrio, Msg as DeriveMsg};
use std::collections::{HashSet};

// #[derive(Encode, Decode, Debug, Clone)]
// pub struct ReadRequest {
//     /// Unique identifier for the request -> Clientname_r/w_requestid
//     pub client_id: String,
//     pub msg_id: String,
//     pub key: String,
// }

// #[derive(Encode, Decode, Debug, Clone)]
// pub struct WriteRequest {
//     pub client_id: String,
//     pub msg_id: String,
//     pub key: String,
//     pub val: String,
// }

// #[derive(Encode, Decode, Debug, Clone)]
// pub struct ReadResponse {
//     pub msg_id: String,
//     pub key: String,
//     pub val: Option<String>,
// }

// #[derive(Encode, Decode, Debug, Clone)]
// pub struct WriteResponse {
//     pub msg_id: String,
//     pub key: String,
//     pub success: bool,
// }

#[derive(Encode, Decode, Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Variable {
    pub name: String,
}

#[derive(Encode, Decode, Debug, Clone)]
pub enum Command {
    Get { key: Variable },
    Set { key: Variable, val: String },
}

impl Command {
    #[allow(dead_code)]
    pub fn conflicts_with(&self, other: &Command) -> bool {
        self.key() == other.key()
    }

    pub fn key(&self) -> &Variable {
        match self {
            Command::Get { key } => key,
            Command::Set { key, .. } => key,
        }
    }
}

#[derive(Encode, Decode, Debug, Clone)]
pub struct ClientRequest {
    pub client_id: String,
    pub msg_id: String,
    pub cmd: Command,
}

#[derive(Encode, Decode, Debug, Clone)]
pub enum CommandResult {
    Get { key: Variable, val: Option<String> },
    Set { key: Variable, status: bool },
}
impl CommandResult {
    #[allow(dead_code)]
    pub fn key(&self) -> &Variable {
        match self {
            CommandResult::Get { key, .. } => key,
            CommandResult::Set { key, .. } => key,
        }
    }
}

#[derive(Encode, Decode, Debug, Clone)]
pub struct ClientResponse {
    pub msg_id: String,
    pub cmd_result: CommandResult,
}

#[derive(Encode, Decode, Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Instance {
    pub replica: String,
    pub instance_num: u64,
}

#[derive(Encode, Decode, Debug, Clone)]
pub struct PreAcceptMsg {
    pub cmd: Command,
    pub seq: u64,
    pub deps: HashSet<Instance>,
    pub instance: Instance,
}

#[derive(Encode, Decode, Debug, Clone)]
pub struct PreAcceptOkMsg {
    // pub cmd: Command,
    pub seq: u64,
    pub deps: HashSet<Instance>,
    pub instance: Instance,
}

#[derive(Encode, Decode, Debug, Clone)]
pub struct CommitMsg {
    pub cmd: Command,
    pub seq: u64,
    pub deps: HashSet<Instance>,
    pub instance: Instance,
}

#[derive(Encode, Decode, Debug, Clone)]
pub struct AcceptMsg {
    pub cmd: Command,
    pub seq: u64,
    pub deps: HashSet<Instance>,
    pub instance: Instance,
}

#[derive(Encode, Decode, Debug, Clone)]
pub struct AcceptOkMsg {
    // pub cmd: Command,
    pub instance: Instance,
}

#[derive(Encode, Decode, Debug, Clone, DefaultPrio, DeriveMsg)]
pub enum EMsg {
    ClientRequest(ClientRequest),
    ClientResponse(ClientResponse),
    PreAccept(PreAcceptMsg),
    PreAcceptOk(PreAcceptOkMsg),
    Commit(CommitMsg),
    Accept(AcceptMsg),
    AcceptOk(AcceptOkMsg),
    // ReadRequest(ReadRequest),
    // WriteRequest(WriteRequest),
    // ReadResponse(ReadResponse),
    // WriteResponse(WriteResponse),
}
