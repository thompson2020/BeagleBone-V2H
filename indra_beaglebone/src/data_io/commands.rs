use crate::{
    data_io::{
        db::Parameters,
        operator_settings::{patch as patch_setting, update as update_settings, OperatorSettings, SettingPatch},
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
    SetSetting(SettingPatch),
    SetEvents(Events),
    #[default]
    GetMode,
    GetEvents,
    GetRecords(Parameters),
    StartLogs,
    StopLogs,
    SetLogLevel(String),
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
            log::info!("Command: SetMode -> {mode:?}");
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
        Cmd::SetSetting(patch) => {
            log::info!("Command: SetSetting");
            patch_setting(patch).await;
            true
        }
        Cmd::SetLogLevel(level_str) => {
            let level = match level_str.to_lowercase().as_str() {
                "trace" => log::LevelFilter::Trace,
                "debug" => log::LevelFilter::Debug,
                "info"  => log::LevelFilter::Info,
                "warn"  => log::LevelFilter::Warn,
                "error" => log::LevelFilter::Error,
                "off"   => log::LevelFilter::Off,
                _ => {
                    log::warn!("SetLogLevel: unknown level '{level_str}'");
                    return false;
                }
            };
            crate::logger::set_level(level);
            log::info!("Log level set to {level}");
            true
        }
        _ => false,
    }
}
