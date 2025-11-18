use bincode::{Decode, Encode};
use reactor_macros::{DefaultPrio, Msg as DeriveMsg};
use std::collections::HashSet;
use std::fmt;

#[derive(Encode, Decode, Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Variable {
    pub name: String,
}

impl fmt::Display for Variable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.name)
    }
}

#[derive(Encode, Decode, Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
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

impl fmt::Display for Command {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Command::Get { key } => write!(f, "Get({})", key),
            Command::Set { key, val } => write!(f, "Set({},{})", key, val),
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
    pub client_id: String,
    pub cmd_result: CommandResult,
}

#[derive(Encode, Decode, Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Instance {
    pub replica: String,
    pub instance_num: usize,
}

impl fmt::Display for Instance {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Inst({},{})", self.replica, self.instance_num)
    }
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
}
