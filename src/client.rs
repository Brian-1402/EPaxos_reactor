use crate::common::{ClientRequest, Command, CommandResult, EMsg, Variable};
use rand::prelude::*;
use rand::rngs::StdRng;
use rand_distr::{Distribution, Exp, Zipf}; // Exp for Time, Zipf for key selection
use reactor_actor::codec::BincodeCodec;
use reactor_actor::{ActorAddr, BehaviourBuilder, RouteTo, RuntimeCtx, SendErrAction};
use serde::Deserialize;
use std::time::{Duration, Instant};
use tokio::task;
#[cfg(feature = "verbose")]
use tracing::info;

// //////////////////////////////////////////////////////////////////////////////
//                                  Configuration
// //////////////////////////////////////////////////////////////////////////////

pub enum KeyDistribution {
    /// Keys are chosen uniformly at random
    Uniform,
    /// Keys are chosen according to a Zipfian distribution with the given skew
    /// few keys are "hot" and are chosen more frequently
    /// skew = 0.99 is the YCSB default
    Zipfian { skew: f64 },
}

#[derive(Clone, Deserialize)]
pub struct Workload {
    #[serde(default)]
    pub target_rps: f64, // Target requests per second
    #[serde(default)]
    pub key_space_size: usize, // Number of unique keys
    #[serde(default)]
    pub zipf_skew: f64, // Zipfian skew parameter (0.0 for uniform)
    #[serde(default)]
    pub read_ratio: f64, // Ratio of read operations (0.0 - all writes, 1.0 - all reads)
    #[serde(default)]
    pub run_duration: u64, // Duration to run the workload in seconds
}

pub struct WorkloadConfig {
    pub target_rps: f64,               // Target requests per second
    pub key_space_size: usize,         // Number of unique keys
    pub distribution: KeyDistribution, // Key selection distribution
    pub read_ratio: f64, // Ratio of read operations (0.0 - all writes, 1.0 - all reads)
    pub run_duration: Duration, // Duration to run the workload
}

impl Default for WorkloadConfig {
    fn default() -> Self {
        WorkloadConfig {
            target_rps: 10.0,
            key_space_size: 10,
            distribution: KeyDistribution::Zipfian { skew: 0.99 },
            read_ratio: 0.5,
            run_duration: Duration::from_secs(60),
        }
    }
}

impl WorkloadConfig {
    fn new(workload: Workload) -> Self {
        let distribution = if workload.zipf_skew == 0.0 {
            KeyDistribution::Uniform
        } else {
            KeyDistribution::Zipfian {
                skew: workload.zipf_skew,
            }
        };
        WorkloadConfig {
            target_rps: workload.target_rps,
            key_space_size: workload.key_space_size,
            distribution,
            read_ratio: workload.read_ratio,
            run_duration: Duration::from_secs(workload.run_duration),
        }
    }
}

// //////////////////////////////////////////////////////////////////////////////
//                                  Generator
// //////////////////////////////////////////////////////////////////////////////

pub struct WorkloadIterator {
    // Identifies the client
    addr: ActorAddr,
    request_count: usize, // Unique message ID's

    // Lifecycle
    start_time: Instant,
    run_duration: Duration,

    // Timing (Poisson Process)
    exp_dist: Exp<f64>,
    next_arrival: Instant,

    // Key Selection
    rng: StdRng,
    key_dist: Option<Zipf<f64>>, // None if uniform distribution
    key_space_size: usize,

    read_ratio: f64, // Ratio of read operations
}

impl WorkloadIterator {
    pub fn new(addr: ActorAddr, config: WorkloadConfig) -> Self {
        let exp_dist = Exp::new(config.target_rps).expect("RPS must be positive");

        let key_dist = match config.distribution {
            KeyDistribution::Uniform => None,
            KeyDistribution::Zipfian { skew } => Some(
                Zipf::new(config.key_space_size as f64, skew).expect("Invalid Zipf parameters"),
            ),
        };

        Self {
            addr,
            request_count: 0,
            start_time: Instant::now(),
            run_duration: config.run_duration,
            exp_dist,
            next_arrival: Instant::now(),
            rng: StdRng::from_rng(&mut rand::rng()),
            key_dist,
            key_space_size: config.key_space_size,
            read_ratio: config.read_ratio,
        }
    }

    pub fn generate_key(&mut self) -> String {
        let key_index = match &self.key_dist {
            Some(zipf) => (zipf.sample(&mut self.rng)) as usize,
            None => self.rng.random_range(0..self.key_space_size),
        };

        format!("key_{}", key_index)
    }
}

impl Iterator for WorkloadIterator {
    type Item = EMsg;

    fn next(&mut self) -> Option<Self::Item> {
        // Check if run duration exceeded
        let now = Instant::now();
        if now.duration_since(self.start_time) >= self.run_duration {
            return None;
        }

        if self.next_arrival > now {
            let sleep_time = self.next_arrival - now;
            task::block_in_place(|| {
                std::thread::sleep(sleep_time);
            });
        } else {
            // Reset arrival time if behind schedule to prevent burstiness
            self.next_arrival = now;
        }

        // Calculate next arrival time
        let interval_secs = self.exp_dist.sample(&mut self.rng);
        self.next_arrival += Duration::from_secs_f64(interval_secs);

        // Decide if read or write
        let is_write = !self.rng.random_bool(self.read_ratio);

        // Generate request
        self.request_count += 1;
        let key = self.generate_key();

        let msg_id = self.request_count.to_string(); // Unique message ID

        if is_write {
            Some(EMsg::ClientRequest(ClientRequest {
                msg_id,
                client_id: self.addr.to_string(),
                cmd: Command::Set {
                    key: Variable { name: key },
                    val: format!("value_{}_{}", self.addr, self.request_count),
                },
            }))
        } else {
            Some(EMsg::ClientRequest(ClientRequest {
                msg_id,
                client_id: self.addr.to_string(),
                cmd: Command::Get {
                    key: Variable { name: key },
                },
            }))
        }
    }
}

// //////////////////////////////////////////////////////////////////////////////
//                                  Processor
// //////////////////////////////////////////////////////////////////////////////
struct Processor {
    #[cfg(feature = "verbose")]
    store: std::collections::HashMap<String, (String, String)>, // Storing msg-id to key-value pairs at client for lchecker
}

impl reactor_actor::ActorProcess for Processor {
    type IMsg = EMsg;
    type OMsg = EMsg;

    fn process(&mut self, input: Self::IMsg) -> Vec<Self::OMsg> {
        match &input {
            // For CP read client, it gets CPReadRequest messages from the generator
            // and just directly sends to the Actor::Sender
            EMsg::ClientRequest(req) => {
                match &req.cmd {
                    Command::Get { key } => {
                        #[cfg(feature = "verbose")]
                        {
                            info!(
                                "{} [Req: {}] Getting {}",
                                req.client_id, req.msg_id, key.name
                            );
                        }
                        vec![input]
                    }
                    Command::Set { key, val } => {
                        #[cfg(feature = "verbose")]
                        {
                            // Store msg_id, key, and value for lchecker
                            self.store
                                .insert(req.msg_id.clone(), (key.name.clone(), val.clone()));
                            info!(
                                "{} [Req: {}] Setting {} = {}",
                                req.client_id, req.msg_id, key.name, val
                            );
                        }
                        vec![input]
                    }
                }
            }

            EMsg::ClientResponse(resp) => {
                match &resp.cmd_result {
                    CommandResult::Get { key, val } => {
                        #[cfg(feature = "verbose")]
                        info!(
                            "{} [Req: {}] Get {} = {}",
                            resp.client_id,
                            resp.msg_id,
                            key.name,
                            val.as_deref().unwrap_or("NONE")
                        );
                        vec![]
                    }
                    CommandResult::Set { key, status: _ } => {
                        #[cfg(feature = "verbose")]
                        info!(
                            "{} [Req: {}] Set {} = {}",
                            resp.client_id,
                            resp.msg_id,
                            key.name,
                            self.store.get(&resp.msg_id).unwrap().1
                        ); // Will exist
                        vec![]
                    }
                }
            }

            _ => {
                panic!("Client got an unexpected message")
            }
        }
    }
}

// //////////////////////////////////////////////////////////////////////////////
//                                  Sender
// //////////////////////////////////////////////////////////////////////////////
struct Sender {
    servers: Vec<String>,
}

impl reactor_actor::ActorSend for Sender {
    type OMsg = EMsg;

    async fn before_send<'a>(&'a mut self, _output: &Self::OMsg) -> RouteTo<'a> {
        match &_output {
            EMsg::ClientRequest(_) => {
                // Send randomly to any server in the list
                let mut rng = rand::rng();
                let choice = self.servers.choose(&mut rng).unwrap();

                RouteTo::from(choice.as_str())
            }

            _ => {
                panic!("Reader tried to send non ReadRequest")
            }
        }
    }
}

impl Sender {
    fn new(servers: Vec<String>) -> Self {
        Sender { servers }
    }
}

// //////////////////////////////////////////////////////////////////////////////
//                                  ACTORS
// //////////////////////////////////////////////////////////////////////////////

pub async fn cp_client(ctx: RuntimeCtx, servers: Vec<String>, workload: Option<Workload>) {
    let mut config = WorkloadConfig::default();
    if workload.is_some() {
        config = WorkloadConfig::new(workload.unwrap());
    }

    BehaviourBuilder::new(
        Processor {
            #[cfg(feature = "verbose")]
            store: std::collections::HashMap::new(),
        },
        BincodeCodec::default(),
    )
    .send(Sender::new(servers))
    .generator_if(true, || WorkloadIterator::new(ctx.addr.to_string(), config))
    .on_send_failure(SendErrAction::Drop)
    .build()
    .run(ctx)
    .await
    .unwrap();
}
