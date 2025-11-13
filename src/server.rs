use crate::common::{EMsg, ReadResponse, WriteResponse};
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
            EMsg::ReadRequest(msg) => {
                let val = self.data.get(&msg.key).cloned();
                vec![EMsg::ReadResponse(ReadResponse {
                    msg_id: msg.msg_id.clone(),
                    key: msg.key.clone(),
                    val,
                })]
            }

            EMsg::WriteRequest(msg) => {
                self.data.insert(msg.key.clone(), msg.val.clone());
                vec![EMsg::WriteResponse(WriteResponse {
                    msg_id: msg.msg_id.clone(),
                    key: msg.key.clone(),
                    success: true,
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
        Processor{data: map}
    }
}

struct Sender {}
impl reactor_actor::ActorSend for Sender {
    type OMsg = EMsg;

    async fn before_send<'a>(&'a mut self, _output: &Self::OMsg) -> RouteTo<'a> {
        match &_output {
            EMsg::ReadResponse(_) | EMsg::WriteResponse(_) => RouteTo::Reply,
            _ => {
                panic!("Server tried to send non ReadResponse")
            }
        }
    }
}

// //////////////////////////////////////////////////////////////////////////////
//                                  ACTORS
// //////////////////////////////////////////////////////////////////////////////

pub async fn server(ctx: RuntimeCtx) {
    BehaviourBuilder::new(
        Processor::new(),
        BincodeCodec::default(),
    )
    .send(Sender {})
    .on_send_failure(SendErrAction::Drop)
    .build()
    .run(ctx)
    .await
    .unwrap();
}
