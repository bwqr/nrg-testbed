use std::cmp::min;

use actix::{Actor, Context, StreamHandler, WrapFuture};
use actix::clock::Duration;
use actix::io::SinkWrite;
use actix::prelude::*;
use actix_codec::Framed;
use awc::{BoxedSocket, Client};
use awc::error::WsProtocolError;
use awc::ws::{Codec, Frame, Message};
use futures::stream::{SplitSink, StreamExt};
use log::{error, info};

use core::SocketErrorKind;
use core::websocket_messages::{client, server};

use crate::executor::Executor;
use crate::messages::UpdateExecutorMessage;

type Write = SinkWrite<Message, SplitSink<Framed<BoxedSocket, Codec>, Message>>;

const MAX_TIMING: usize = 5;

const TIMINGS: [u8; MAX_TIMING] = [
    // 0, 15, 30, 75, 120
    0, 2, 4, 6, 8
];

pub struct Connection {
    server_url: String,
    access_token: String,
    sink: Option<Write>,
    // this is the delay until we try connecting again
    current_timing_index: usize,
    executor: Option<Addr<Executor>>,
}

impl Connection {
    pub fn new(server_url: String, access_token: String) -> Self {
        Connection {
            server_url,
            access_token,
            sink: None,
            current_timing_index: 0,
            executor: None,
        }
    }

    fn handle_frame(&mut self, frame: Frame) -> Result<(), SocketErrorKind> {
        match frame {
            Frame::Ping(_) => {
                //update hb
            }
            Frame::Pong(_) => {
                // update hb
            }
            Frame::Text(bytes) => {
                let text = String::from_utf8(bytes.to_vec())
                    .map_err(|_| SocketErrorKind::InvalidMessage)?;
                let text = text.as_str();

                let base = serde_json::from_str::<'_, client::BaseMessage>(text)
                    .map_err(|_| SocketErrorKind::InvalidMessage)?;

                match base.kind {
                    client::SocketMessageKind::RunExperiment => {
                        let run_experiment = serde_json::from_str::<'_, client::SocketMessage<client::RunExperiment>>(text)
                            .map_err(|_| SocketErrorKind::InvalidMessage)?;

                        info!("received run from server, id {}", run_experiment.data.run_id);

                        if let Some(sink) = &mut self.sink {
                            sink.write(Message::Text(serde_json::to_string(&server::SocketMessage {
                                kind: server::SocketMessageKind::RunResult,
                                data: server::RunResult {
                                    run_id: run_experiment.data.run_id,
                                    successful: true,
                                },
                            }).unwrap()));
                        }
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    async fn connect(server_url: String, access_token: String) -> Result<Framed<BoxedSocket, Codec>, Error> {
        Ok(Client::new()
            .ws(format!("{}?token={}", server_url, access_token))
            .connect()
            .await
            .map_err(|e| {
                error!("{:?}", e);
                Error::ServerNotReachable
            })?.1)
    }

    fn try_connect(act: &mut Connection, ctx: &mut <Self as Actor>::Context) {
        Self::connect(act.server_url.clone(), act.access_token.clone())
            .into_actor(act)
            .then(move |framed, act, ctx| {
                if let Ok(framed) = framed {
                    info!("Connected to server");

                    let (sink, stream) = framed.split();
                    Self::add_stream(stream, ctx);
                    act.sink = Some(SinkWrite::new(sink, ctx));
                    // we have connected now, reset timing
                    act.current_timing_index = 0;
                } else {
                    act.current_timing_index = min(act.current_timing_index + 1, MAX_TIMING - 1);

                    info!("Could not connect to server, will retry in {} seconds", TIMINGS[act.current_timing_index]);

                    ctx.run_later(Duration::from_secs(TIMINGS[act.current_timing_index] as u64), |act, ctx| {
                        Self::try_connect(act, ctx);
                    });
                }

                fut::ready(())
            })
            .spawn(ctx);
    }
}

impl Actor for Connection {
    type Context = Context<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {
        Self::try_connect(self, ctx);
    }

    fn stopped(&mut self, _: &mut Self::Context) {}
}

impl StreamHandler<Result<Frame, WsProtocolError>> for Connection {
    fn handle(&mut self, frame: Result<Frame, WsProtocolError>, ctx: &mut Context<Self>) {
        match frame {
            Ok(frame) => {
                if let Err(e) = self.handle_frame(frame) {
                    error!("{:?}", e);
                }
            }
            Err(e) => error!("{:?}", e)
        };
    }

    fn finished(&mut self, ctx: &mut Context<Self>) {
        info!("Server disconnected, trying to reconnect");
        self.sink = None;
        Self::try_connect(self, ctx);
    }
}

impl Handler<UpdateExecutorMessage> for Connection {
    type Result = ();

    fn handle(&mut self, msg: UpdateExecutorMessage, _: &mut Self::Context) {
        self.executor = Some(msg.executor);
    }
}

impl actix::io::WriteHandler<WsProtocolError> for Connection {}

pub enum Error {
    ServerNotReachable
}