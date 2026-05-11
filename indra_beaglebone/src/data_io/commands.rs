use crate::{
    data_io::{
        db::Parameters,
        operator_settings::{update as update_settings, OperatorSettings},
    },
    global_state::OperationMode,
    scheduler::Events,
    statics::ChademoTx,
};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Default)]
pub enum Cmd {
    SetMode(OperationMode),
    SetSettings(OperatorSettings),
    SetEvents(Events),
    #[default]
    GetMode,
    GetEvents,
    GetRecords(Parameters),
    StartLogs,
    StopLogs,
}

#[derive(Serialize, Deserialize, Debug, Default)]
pub struct Instruction {
    pub cmd: Cmd,
}

/// Execute a mutation command. Returns true if handled, false if it is a
/// query or WS-only command that the caller must handle itself.
///
/// Both the WebSocket handler (api/mod.rs) and the MQTT handler (mqtt.rs)
/// call this so mutation logic is never duplicated.
pub async fn dispatch(cmd: Cmd, mode_tx: &ChademoTx) -> bool {
    match cmd {
        Cmd::SetMode(mode) => {
            log::info!("Command: SetMode → {mode:?}");
            if let Err(e) = mode_tx.send(mode).await {
                log::error!("Command: SetMode channel | {e}");
            }
            true
        }
        Cmd::SetSettings(settings) => {
            log::info!("Command: SetSettings");
            update_settings(settings).await;
            true
        }
        _ => false,
    }
}
