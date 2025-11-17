use crate::common::{ClientResponse, Command, CommandResult, EMsg};
use reactor_actor::codec::BincodeCodec;
use reactor_actor::{BehaviourBuilder, RouteTo, RuntimeCtx, SendErrAction};

use std::collections::HashMap;

// //////////////////////////////////////////////////////////////////////////////
//                                  Processor
// //////////////////////////////////////////////////////////////////////////////
struct Processor {
    data: HashMap<String, String>,
}

impl reactor_actor::ActorProcess for Processor {
    type IMsg = EMsg;
    type OMsg = EMsg;

    fn process(&mut self, input: Self::IMsg) -> Vec<Self::OMsg> {
        match &input {
            EMsg::ClientRequest(msg) => {
                let cmd_result = match &msg.cmd {
                    Command::Get { key } => {
                        let val = self.data.get(&key.name).cloned();
                        CommandResult::Get {
                            key: key.clone(),
                            val,
                        }
                    }
                    Command::Set { key, val } => {
                        self.data.insert(key.name.clone(), val.clone());
                        CommandResult::Set {
                            key: key.clone(),
                            status: true,
                        }
                    }
                };

                vec![EMsg::ClientResponse(ClientResponse {
                    msg_id: msg.msg_id.clone(),
                    cmd_result,
                })]
            }
            _ => {
                panic!("Server got an unexpected message")
            }
        }
    }
}

impl Processor {
    fn new() -> Self {
        let mut map = HashMap::new();
        map.insert("key1".to_string(), "val1".to_string());
        Processor { data: map }
    }
}

struct Sender {}
impl reactor_actor::ActorSend for Sender {
    type OMsg = EMsg;

    async fn before_send<'a>(&'a mut self, _output: &Self::OMsg) -> RouteTo<'a> {
        match &_output {
            EMsg::ClientResponse(_) => RouteTo::Reply,
            _ => {
                panic!("Server tried to send non ClientResponse")
            }
        }
    }
}

// //////////////////////////////////////////////////////////////////////////////
//                                  ACTORS
// //////////////////////////////////////////////////////////////////////////////

pub async fn server(ctx: RuntimeCtx) {
    BehaviourBuilder::new(Processor::new(), BincodeCodec::default())
        .send(Sender {})
        .on_send_failure(SendErrAction::Drop)
        .build()
        .run(ctx)
        .await
        .unwrap();
}
