use crate::SLEEP_MS;
use crate::common::{EMsg, ReadRequest};
use reactor_actor::{SendErrAction, BehaviourBuilder, RouteTo, RuntimeCtx};
use reactor_actor::codec::BincodeCodec;


use std::time::Duration;

#[cfg(feature = "verbose")]
use tracing::info;

// //////////////////////////////////////////////////////////////////////////////
//                                  Generator
// //////////////////////////////////////////////////////////////////////////////
struct DelayedReadIterator {
    count: usize,
    addr: String,
}

// Generator which creates
impl Iterator for DelayedReadIterator {
    type Item = EMsg;

    fn next(&mut self) -> Option<Self::Item> {
        if self.count < 10 {
            std::thread::sleep(Duration::from_millis(10 * SLEEP_MS));
            self.count += 1;
            Some(EMsg::ReadRequest(ReadRequest {
                msg_id: format!("{}_r_{}", self.addr, self.count),
                key: format!("key1"),
                // key: format!("foo{}", self.count),
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
    reader_client: String,
}

impl reactor_actor::ActorProcess for Processor {
    type IMsg = EMsg;
    type OMsg = EMsg;

    fn process(&mut self, input: Self::IMsg) -> Vec<Self::OMsg> {
        match &input {
            // For CR read client, it gets CRReadRequest messages from the generator
            // and just directly sends to the Actor::Sender
            EMsg::ReadRequest(_msg) => {
                #[cfg(feature = "verbose")]
                {
                    info!("{} Getting {}", self.reader_client, _msg.key);
                }
                vec![input]
            }

            EMsg::ReadResponse(_msg) => {
                #[cfg(feature = "verbose")]
                info!(
                    "{} Get {} = {}",
                    self.reader_client,
                    _msg.key,
                    _msg.val.as_deref().unwrap_or("NONE")
                );
                vec![]
            }

            // _ => {
            //     panic!("Reader got an unexpected message")
            // }
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

    async fn before_send<'a>(&'a mut self, _output: &Self::OMsg) -> RouteTo<'a> {
        match &_output {
            EMsg::ReadRequest(_) => RouteTo::from(self.server.as_str()),
            _ => {
                panic!("Reader tried to send non ReadRequest")
            }
        }
    }
}

impl Sender {
    fn new(server: String) -> Self {
        Sender {server}
    }
}

// //////////////////////////////////////////////////////////////////////////////
//                                  ACTORS
// //////////////////////////////////////////////////////////////////////////////

pub async fn reader(ctx: RuntimeCtx, server: String) {
    BehaviourBuilder::new(
        Processor {
            #[cfg(feature = "verbose")]
            reader_client: ctx.addr.to_string(),
        },
        BincodeCodec::default(),
    )
    .send(Sender::new(server))
    .generator_if(true, || DelayedReadIterator {
        count: 0,
        addr: ctx.addr.to_string(),
    })
    .on_send_failure(SendErrAction::Drop)
    .build()
    .run(ctx)
    .await
    .unwrap();
}
