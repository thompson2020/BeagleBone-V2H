use crate::error::IndraError;
use lazy_static::lazy_static;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::RwLock;

const SETTINGS_FILE: &str = "ui_settings.json";
const TAG: &str = "[OPSETTINGS]";

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct OperatorSettings {
    // Smart Self-Powered
    pub v2h_soc_min:          u8,
    pub v2h_soc_max:          u8,
    pub v2h_max_amps:         u8,
    pub self_use:             bool,
    pub export_excess_solar:  bool,
    pub ev_drain_protection:  bool,
    // Time / Event Based
    pub ready_to_drive:            bool,
    pub ready_to_drive_end_time:   String,
    #[serde(default = "default_rtd_start_time")]
    pub ready_to_drive_start_time: String,
    pub ready_to_drive_soc:        u8,
    pub ready_to_drive_days:       [bool; 7],
    pub off_peak_charging:    bool,
    pub off_peak_start:       String,
    pub off_peak_end:         String,
    pub smart_charge:         bool,
    pub smart_export:         bool,
    pub smart_export_limit_w: u32,
    pub smart_export_excess_solar: bool,
    // Charge card
    pub charge_soc_limit:     u8,
    pub charge_amps:          u8,
    pub charge_eco:           bool,
}

fn default_rtd_start_time() -> String { "--:--".to_string() }

impl Default for OperatorSettings {
    fn default() -> Self {
        Self {
            v2h_soc_min:          31,
            v2h_soc_max:          90,
            v2h_max_amps:         16,
            self_use:             true,
            export_excess_solar:  false,
            ev_drain_protection:  false,
            ready_to_drive:            false,
            ready_to_drive_end_time:   "08:00".to_string(),
            ready_to_drive_start_time: "--:--".to_string(),
            ready_to_drive_soc:        90,
            ready_to_drive_days:       [false; 7],
            off_peak_charging:    true,
            off_peak_start:       "23:30".to_string(),
            off_peak_end:         "05:30".to_string(),
            smart_charge:         true,
            smart_export:         false,
            smart_export_limit_w: 2500,
            smart_export_excess_solar:  false,
            charge_soc_limit:     90,
            charge_amps:          16,
            charge_eco:           false,
        }
    }
}

impl OperatorSettings {
    pub fn validate_and_clamp(&mut self) {
        let max_soc  = crate::MAX_SOC;
        let min_soc  = crate::MIN_SOC;
        let max_amps = crate::MAX_AMPS;

        // SoC bounds
        self.v2h_soc_min       = self.v2h_soc_min.clamp(min_soc, max_soc);
        self.v2h_soc_max       = self.v2h_soc_max.clamp(min_soc, max_soc);
        if self.v2h_soc_min >= self.v2h_soc_max {
            log::warn!("{TAG} v2h_soc_min ({}) >= v2h_soc_max ({}) — pulling min down", self.v2h_soc_min, self.v2h_soc_max);
            self.v2h_soc_min = self.v2h_soc_max.saturating_sub(5).max(min_soc);
        }
        self.charge_soc_limit  = self.charge_soc_limit.clamp(min_soc, max_soc);
        self.ready_to_drive_soc = self.ready_to_drive_soc.clamp(min_soc, max_soc);

        // Amps bounds
        self.v2h_max_amps = self.v2h_max_amps.min(max_amps);
        self.charge_amps  = self.charge_amps.min(max_amps);
    }
}

fn to_json(settings: &OperatorSettings) -> String {
    serde_json::to_string(settings).unwrap_or_else(|_| "<serialise error>".to_string())
}

lazy_static! {
    pub static ref OPERATOR_SETTINGS: Arc<RwLock<OperatorSettings>> =
        Arc::new(RwLock::new(OperatorSettings::default()));
}

/// Load settings from disk on startup. Creates the file with defaults if absent or unparseable.
pub async fn load() -> OperatorSettings {
    match tokio::fs::read_to_string(SETTINGS_FILE).await {
        Ok(contents) => match serde_json::from_str::<OperatorSettings>(&contents) {
            Ok(mut settings) => {
                settings.validate_and_clamp();
                log::info!("{TAG} loaded from {} | {}", SETTINGS_FILE, to_json(&settings));
                settings
            }
            Err(e) => {
                log::warn!("{TAG} parse error in {} ({}) — reverting to defaults", SETTINGS_FILE, e);
                let defaults = OperatorSettings::default();
                log::info!("{TAG} defaults | {}", to_json(&defaults));
                let _ = save(&defaults).await;
                defaults
            }
        },
        Err(_) => {
            log::info!("{TAG} {} not found — creating with defaults", SETTINGS_FILE);
            let defaults = OperatorSettings::default();
            log::info!("{TAG} defaults | {}", to_json(&defaults));
            let _ = save(&defaults).await;
            defaults
        }
    }
}

async fn save(settings: &OperatorSettings) -> Result<(), IndraError> {
    let json = serde_json::to_string_pretty(settings).map_err(IndraError::Serialise)?;
    tokio::fs::write(SETTINGS_FILE, json)
        .await
        .map_err(IndraError::FileAccess)?;
    log::debug!("{TAG} saved to {}", SETTINGS_FILE);
    Ok(())
}

/// Single entry point for all settings changes.
/// Call from both the WebSocket handler and (future) MQTT handler.
pub async fn update(mut new_settings: OperatorSettings) {
    new_settings.validate_and_clamp();
    log::info!("{TAG} update received | {}", to_json(&new_settings));
    *OPERATOR_SETTINGS.write().await = new_settings.clone();
    if let Err(e) = save(&new_settings).await {
        log::error!("{TAG} failed to persist: {:?}", e);
    }
    // TODO: trigger broadcast to all WS clients when broadcast channel is added
}
