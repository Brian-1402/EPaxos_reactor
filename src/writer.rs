use crate::SLEEP_MS;
use crate::common::{EMsg, WriteRequest};
use reactor_actor::{SendErrAction, BehaviourBuilder, RouteTo, RuntimeCtx};
use reactor_actor::codec::BincodeCodec;

use std::time::Duration;

#[cfg(feature = "verbose")]
use tracing::info;

// //////////////////////////////////////////////////////////////////////////////
//                                  Generator
// //////////////////////////////////////////////////////////////////////////////
/// Iterator which yields write requests with a delay. Used by reactor-generator to create messages

struct WriteReqGenerator {
    count: usize,
    addr: String,
}

impl Iterator for WriteReqGenerator {
    type Item = EMsg;

    fn next(&mut self) -> Option<Self::Item> {
        if self.count < 10 {
            std::thread::sleep(Duration::from_millis(10 * SLEEP_MS));
            self.count += 1;
            Some(EMsg::WriteRequest(WriteRequest {
                msg_id: format!("{}_w_{}", self.addr, self.count),
                key: format!("key1"),
                val: format!("value{}", self.count),
            }))
        } else {
            None
        }
    }
}

// //////////////////////////////////////////////////////////////////////////////
//                                  Processor
// //////////////////////////////////////////////////////////////////////////////

struct Processor {
    #[cfg(feature = "verbose")]
    writer_client: String,
}

impl reactor_actor::ActorProcess for Processor {
    type IMsg = EMsg;
    type OMsg = EMsg;

    fn process(&mut self, input: Self::IMsg) -> Vec<Self::OMsg> {
        match &input {
            EMsg::WriteRequest(_msg) => {
                #[cfg(feature = "verbose")]
                {
                    info!("{} Writing: key={} val={}", self.writer_client, _msg.key, _msg.val);
                }
                vec![input]
            },  // forward to server

            EMsg::WriteResponse(resp) => {
                #[cfg(feature = "verbose")]
                info!("{} WriteResponse: {} -> success={}", 
                      self.writer_client, resp.key, resp.success);
                vec![]
            }

            _ => panic!("Writer got unexpected message"),
        }
    }
}

// //////////////////////////////////////////////////////////////////////////////
//                                  Sender
// //////////////////////////////////////////////////////////////////////////////

struct Sender {
    server: String,
}

impl reactor_actor::ActorSend for Sender {
    type OMsg = EMsg;

    async fn before_send<'a>(&'a mut self, output: &Self::OMsg) -> RouteTo<'a> {
        match &output {
            EMsg::WriteRequest(_) => RouteTo::from(self.server.as_str()),
            _ => panic!("Writer tried to send non WriteRequest"),
        }
    }
}

impl Sender {
    fn new(server: String) -> Self {
        Self { server }
    }
}

// Build actor
pub async fn writer(ctx: RuntimeCtx, server: String) {
    BehaviourBuilder::new(
        Processor {
            #[cfg(feature = "verbose")]
            writer_client: ctx.addr.to_string(),
        },
        BincodeCodec::default(),
    )
    .send(Sender::new(server))
    .generator_if(true, || WriteReqGenerator {
        count: 0,
        addr: ctx.addr.to_string(),
    })
    .on_send_failure(SendErrAction::Drop)
    .build()
    .run(ctx)
    .await
    .unwrap();
}
