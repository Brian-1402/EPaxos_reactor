use crate::SLEEP_MS;
use crate::common::{ClientRequest, Command, CommandResult, EMsg, Variable};
use reactor_actor::codec::BincodeCodec;
use reactor_actor::{BehaviourBuilder, RouteTo, RuntimeCtx, SendErrAction};

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
            std::thread::sleep(Duration::from_millis(SLEEP_MS));
            // std::thread::sleep(Duration::from_millis(100));
            self.count += 1;

            let cmd = Command::Set {
                key: Variable {
                    name: "key1".to_string(),
                },
                // key: Variable(format!("foo{}", self.count)),
                val: format!("value{}{}", self.addr, self.count),
            };
            Some(EMsg::ClientRequest(ClientRequest {
                client_id: self.addr.clone(),
                msg_id: format!("{}_r_{}", self.addr, self.count),
                cmd,
            }))
        // } else if self.count == 1 {
        //     //send EMsg::DumpStateMsg
        //     self.count += 1;
        //     Some(EMsg::DumpStateMsg)
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
            EMsg::ClientRequest(_msg) => {
                #[cfg(feature = "verbose")]
                if let Command::Set { key, val } = &_msg.cmd {
                    info!(
                        "{} Writing: key={} val={}",
                        self.writer_client, key.name, val
                    );
                }
                vec![input]
            }

            EMsg::ClientResponse(_resp) => {
                #[cfg(feature = "verbose")]
                if let CommandResult::Set { key, status } = &_resp.cmd_result {
                    info!(
                        "{} WriteResponse: {} -> success={}",
                        self.writer_client, key.name, status
                    );
                }
                vec![]
            } // _ => panic!("Writer got unexpected message"),
            EMsg::DumpStateMsg => {
                #[cfg(feature = "verbose")]
                {
                    info!("{} Sending DumpStateMsg", self.writer_client);
                }
                vec![input]
            }
            _ => {
                panic!("Writer got unexpected message")
            }
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
            EMsg::ClientRequest(_) | EMsg::DumpStateMsg => RouteTo::from(self.server.as_str()),
            _ => panic!("Writer tried to send non WriteRequest"),
        }
    }
}

impl Sender {
    fn new(server: String) -> Self {
        Self { server }
    }
}

// //////////////////////////////////////////////////////////////////////////////
//                                  ACTORS
// //////////////////////////////////////////////////////////////////////////////

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
