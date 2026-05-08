use crate::{
    chademo::state::CHADEMO,
    data_io::{
        db::{ChademoDbRow, Parameters},
        operator_settings::{update as update_settings, OperatorSettings},
        status::{snapshot, ChargerSnapshot},
    },
    global_state::OperationMode,
    scheduler::{get_eventfile_sync, Events},
    statics::{ChademoTx, EventsTx},
    POOL,
};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use std::{
    io::Error,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::{
    net::{TcpListener, TcpStream},
    sync::{mpsc, watch},
    time::sleep,
};
use tokio_tungstenite::tungstenite::{self, Message};

const BAD_ACK: &str = r#"{"ack":"err"}"#;
const PUSH_INTERVAL: Duration = Duration::from_secs(1);

pub async fn run(events_tx: EventsTx, mode_tx: ChademoTx) -> Result<(), Error> {
    let listener = TcpListener::bind("0.0.0.0:5555").await?;
    log::info!("WebSockets listening on: 0.0.0.0:5555");

    // Single producer serialises snapshot once per second; all clients share the result.
    let (snap_tx, snap_rx) = watch::channel(String::new());
    tokio::spawn(async move {
        loop {
            sleep(PUSH_INTERVAL).await;
            let snap = snapshot().await;
            match serde_json::to_string(&Response::Data(snap)) {
                Ok(json) => { let _ = snap_tx.send(json); }
                Err(e) => log::error!("WS snapshot serialise error: {e}"),
            }
        }
    });

    while let Ok((stream, _)) = listener.accept().await {
        tokio::spawn(accept_connection(stream, events_tx.clone(), mode_tx.clone(), snap_rx.clone()));
    }
    Ok(())
}

async fn accept_connection(stream: TcpStream, events_tx: EventsTx, mode_tx: ChademoTx, snap_rx: watch::Receiver<String>) {
    let addr = match stream.peer_addr() {
        Ok(a) => a,
        Err(e) => {
            log::error!("WS peer addr error: {e}");
            return;
        }
    };

    let ws_stream = match tokio_tungstenite::accept_async(stream).await {
        Ok(ws) => ws,
        Err(e) => {
            log::error!("WS handshake failed ({addr}): {e}");
            return;
        }
    };
    log::info!("WebSocket connected: {addr}");

    let (write, mut read) = ws_stream.split();

    // All outbound messages go through this channel so the push task, log task,
    // and command handler can share the write half without a mutex.
    let (outbox_tx, mut outbox_rx) = mpsc::channel::<Message>(256);

    // Drain the outbox to the socket.
    let sink_task = tokio::spawn(async move {
        let mut write = write;
        while let Some(msg) = outbox_rx.recv().await {
            if let Err(e) = write.send(msg).await {
                log::debug!("WS sink closed ({addr}): {e}");
                break;
            }
        }
    });

    // Forward pre-serialised snapshot to this client whenever the shared producer fires.
    let push_tx = outbox_tx.clone();
    let push_task = tokio::spawn(async move {
        let mut rx = snap_rx;
        loop {
            if rx.changed().await.is_err() { break; }
            let json = rx.borrow().clone();
            if !json.is_empty() && push_tx.send(Message::Text(json)).await.is_err() {
                break;
            }
        }
    });

    // Forward live log entries only when the client has opted in via StartLogs.
    // try_send so a slow client drops log messages rather than blocking data pushes.
    let logging_enabled = Arc::new(AtomicBool::new(false));
    let log_tx = outbox_tx.clone();
    let log_enabled = logging_enabled.clone();
    let log_task = tokio::spawn(async move {
        let Some(mut rx) = crate::logger::subscribe() else { return };
        loop {
            match rx.recv().await {
                Ok(entry) => {
                    if log_enabled.load(Ordering::Relaxed) {
                        if let Ok(json) = serde_json::to_string(&Response::Log(entry)) {
                            let _ = log_tx.try_send(Message::Text(json));
                        }
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => break,
            }
        }
    });

    // Read loop — process incoming commands, send acks back through the outbox.
    while let Some(result) = read.next().await {
        let text = match result {
            Ok(Message::Text(t)) => t,
            Ok(Message::Binary(b)) => String::from_utf8_lossy(&b).to_string(),
            Ok(Message::Close(_)) => break,
            Ok(_) => continue,
            Err(e) => {
                log::debug!("WS read closed ({addr}): {e}");
                break;
            }
        };

        log::debug!("WS received ({addr}): {text}");
        match process_ws_message(&text, &events_tx, &mode_tx, &logging_enabled).await {
            Ok(msg) => {
                if outbox_tx.send(msg).await.is_err() {
                    break;
                }
            }
            Err(e) => {
                log::error!("WS process error ({addr}): {e}");
                break;
            }
        }
    }

    log::info!("WebSocket disconnected: {addr}");
    log_task.abort();
    push_task.abort();
    sink_task.abort();
}

async fn process_ws_message(
    cmd: &str,
    events_tx: &EventsTx,
    mode_tx: &ChademoTx,
    logging: &Arc<AtomicBool>,
) -> Result<Message, tungstenite::Error> {
    // {"cmd": {"SetMode": "V2h"}}
    // {"cmd": {"SetMode": "Idle"}}
    // {"cmd": {"SetMode": "Charge"}}
    // {"cmd": {"SetMode": "Discharge"}}
    // {"cmd": "GetMode"}
    // {"cmd": "GetEvents"}
    // {"cmd": {"SetEvents": [{"time": "00:01:02", "action": "Charge"}, ...]}}
    // {"cmd": {"SetSettings": {...}}}
    // {"cmd": {"GetRecords": {parameters...}}}
    match serde_json::from_str::<Instruction>(cmd) {
        Ok(d) => match d.cmd {
            Cmd::SetMode(mode) => {
                log::info!("WS SetMode: {mode:?}");
                if let Err(e) = mode_tx.send(mode).await {
                    log::error!("SetMode channel error: {e}");
                }
                Ok(Message::Text(
                    serde_json::to_string(&Response::Mode(mode)).unwrap(),
                ))
            }
            Cmd::GetMode => {
                let mode = *CHADEMO.lock().await.state();
                Ok(Message::Text(
                    serde_json::to_string(&Response::Mode(mode)).unwrap(),
                ))
            }
            Cmd::SetEvents(events) => {
                log::info!("WS SetEvents: {events:?}");
                if let Err(e) = events_tx.send(events).await {
                    log::error!("SetEvents channel error: {e}");
                    return Ok(Message::Text(BAD_ACK.to_string()));
                }
                Ok(Message::Text(r#"{"ack":"ok"}"#.to_string()))
            }
            Cmd::GetEvents => {
                match get_eventfile_sync() {
                    Ok(events) => Ok(Message::Text(
                        serde_json::to_string(&Response::Events(events)).unwrap(),
                    )),
                    Err(e) => {
                        log::error!("GetEvents error: {e:?}");
                        Ok(Message::Text(BAD_ACK.to_string()))
                    }
                }
            }
            Cmd::SetSettings(new_settings) => {
                log::info!("[OPSETTINGS] SetSettings via WebSocket");
                update_settings(new_settings).await;
                Ok(Message::Text(r#"{"ack":"ok"}"#.to_string()))
            }
            Cmd::StartLogs => {
                logging.store(true, Ordering::Relaxed);
                Ok(Message::Text(r#"{"ack":"ok"}"#.to_string()))
            }
            Cmd::StopLogs => {
                logging.store(false, Ordering::Relaxed);
                Ok(Message::Text(r#"{"ack":"ok"}"#.to_string()))
            }
            Cmd::GetRecords(params) => {
                if let Some(db) = POOL.get() {
                    match db.process_request(params).await {
                        Ok(rows) => {
                            log::info!("GetRecords: {} rows returned", rows.len());
                            Ok(Message::Text(
                                serde_json::to_string(&Response::Records(rows)).unwrap(),
                            ))
                        }
                        Err(e) => {
                            log::error!("GetRecords db error: {e:?}");
                            Ok(Message::Text(BAD_ACK.to_string()))
                        }
                    }
                } else {
                    log::error!("GetRecords: DB pool not initialised");
                    Ok(Message::Text(BAD_ACK.to_string()))
                }
            }
        },
        Err(e) => {
            log::error!("WS deserialise error: {cmd} — {e:?}");
            Ok(Message::Text(BAD_ACK.to_string()))
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    #[test]
    fn test() {
        let cmd: &str = r#"{
  "cmd": {
    "SetEvents": [
      {"time": "01:02:03", "action": "Sleep"},
      {"time": "02:03:03", "action": "Sleep"}
    ]
  }
}"#;
        let _result = match serde_json::from_str::<Instruction>(cmd) {
            Ok(d) => match d.cmd {
                Cmd::GetEvents => {
                    let events = match get_eventfile_sync() {
                        Ok(events) => events,
                        Err(e) => panic!("GetEvents failed: {cmd:?} {e:?}"),
                    };
                    let output = serde_json::to_string(&Response::Events(events)).unwrap();
                    log::debug!("GetEvents test | {output}");
                }
                Cmd::SetEvents(events) => {
                    log::info!("SetEvents test | {events:?}");
                }
                _ => {
                    log::error!("Unknown command | {cmd}");
                }
            },
            _ => {
                log::error!("Invalid JSON | {cmd}");
            }
        };
    }
}

#[derive(Serialize, Deserialize, Debug, Default)]
enum Cmd {
    SetMode(OperationMode),
    #[default]
    GetMode,
    SetEvents(Events),
    GetEvents,
    GetRecords(Parameters),
    SetSettings(OperatorSettings),
    StartLogs,
    StopLogs,
}

#[derive(Serialize, Deserialize, Default, Debug)]
struct Instruction {
    cmd: Cmd,
}

#[derive(Serialize, Debug)]
enum Response {
    Data(ChargerSnapshot),
    Mode(OperationMode),
    Events(Events),
    Records(Vec<ChademoDbRow>),
    Log(crate::logger::LogEntry),
}
