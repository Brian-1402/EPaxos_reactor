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

/// Iterator which yields read requests with a delay. Used by reactor-generator to create messages
struct ReadReqGenerator {
    count: usize,
    addr: String,
}

impl Iterator for ReadReqGenerator {
    type Item = EMsg;

    fn next(&mut self) -> Option<Self::Item> {
        if self.count == 0 {
            std::thread::sleep(Duration::from_millis(10 * SLEEP_MS));
            self.count += 1;
            let cmd = Command::Get {
                key: Variable {
                    name: "key1".to_string(),
                },
                // key: Variable(format!("foo{}", self.count)),
            };
            Some(EMsg::ClientRequest(ClientRequest {
                client_id: self.addr.clone(),
                msg_id: format!("{}_r_{}", self.addr, self.count),
                cmd,
            }))
        } else {
            None
        }
    }
}

// //////////////////////////////////////////////////////////////////////////////
//                                  Processor
// //////////////////////////////////////////////////////////////////////////////

/// Processor struct for the Reader actor
/// stores state in the struct fields
/// process() method defines how to handle incoming messages, and return corresponding output messages
struct Processor {
    #[cfg(feature = "verbose")]
    reader_client: String,
}

impl reactor_actor::ActorProcess for Processor {
    type IMsg = EMsg;
    type OMsg = EMsg;

    fn process(&mut self, input: Self::IMsg) -> Vec<Self::OMsg> {
        match &input {
            EMsg::ClientRequest(_msg) => {
                #[cfg(feature = "verbose")]
                {
                    info!("{} Getting {}", self.reader_client, _msg.cmd.key().name);
                }
                vec![input]
            }

            EMsg::ClientResponse(_msg) => {
                #[cfg(feature = "verbose")]
                if let CommandResult::Get { key, val } = &_msg.cmd_result {
                    info!(
                        "{} Get {} = {}",
                        self.reader_client,
                        key.name,
                        (val).as_deref().unwrap_or("NONE")
                    );
                }
                vec![]
            }
            _ => {
                panic!("Reader got unexpected message")
            }
        }
    }
}

// //////////////////////////////////////////////////////////////////////////////
//                                  Sender
// //////////////////////////////////////////////////////////////////////////////

/// Sender struct for the Reader actor
/// before_send() method defines how to route outgoing messages. Go to def of RouteTo for more details
struct Sender {
    server: String,
}

impl reactor_actor::ActorSend for Sender {
    type OMsg = EMsg;

    async fn before_send<'a>(&'a mut self, output: &Self::OMsg) -> RouteTo<'a> {
        match &output {
            EMsg::ClientRequest(_) => RouteTo::from(self.server.as_str()),
            _ => {
                panic!("Reader tried to send non ReadRequest")
            }
        }
    }
}

impl Sender {
    fn new(server: String) -> Self {
        Sender { server }
    }
}

// //////////////////////////////////////////////////////////////////////////////
//                                  ACTORS
// //////////////////////////////////////////////////////////////////////////////

/// Reader actor
/// - BehaviourBuilder takes input these actor components and builds the actor
/// - uses `DelayedReadIterator` to generate read requests with a delay
/// - uses `Processor` to process incoming messages
/// - uses `Sender` to route outgoing messages
/// - Only modify the method calls which take these earlier defined structs. Rest is default boilerplate
/// - `on_send_failure` is to provide setting on what to do when sending fails, retry or drop. Go to `SendErrAction` for more details
/// - Go to docs of `BehaviourBuilder` and `Behavior` struct for more details
pub async fn reader(ctx: RuntimeCtx, server: String) {
    BehaviourBuilder::new(
        Processor {
            #[cfg(feature = "verbose")]
            reader_client: ctx.addr.to_string(),
        },
        BincodeCodec::default(),
    )
    .send(Sender::new(server))
    .generator_if(true, || ReadReqGenerator {
        count: 0,
        addr: ctx.addr.to_string(),
    })
    .on_send_failure(SendErrAction::Drop)
    .build()
    .run(ctx)
    .await
    .unwrap();
}
