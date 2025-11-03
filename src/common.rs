use bincode::{Decode, Encode};
use reactor_macros::{DefaultPrio, Msg as DeriveMsg};

#[derive(Encode, Decode, Debug, Clone)]
pub struct ReadRequest {
    /// Unique identifier for the request -> Clientname_r/w_requestid
    pub msg_id: String,
    pub key: String,
}

#[derive(Encode, Decode, Debug, Clone)]
pub struct ReadResponse {
    pub msg_id: String,
    pub key: String,
    pub val: Option<String>,
}

#[derive(Encode, Decode, Debug, Clone, DefaultPrio, DeriveMsg)]
pub enum EMsg {
    ReadRequest(ReadRequest),
    ReadResponse(ReadResponse),
}
