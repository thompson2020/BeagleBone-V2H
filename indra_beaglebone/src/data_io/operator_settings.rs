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
    pub v2h_soc_max:          u8,   // SoC ceiling for solar V2H mode
    #[serde(default = "default_v2h_soc_max_boost")]
    pub v2h_soc_max_boost:    u8,   // SoC ceiling for off-peak and smart-charge
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
    #[serde(default = "default_smart_export_soc_min")]
    pub smart_export_soc_min: u8,
    pub smart_export_excess_solar: bool,
    // Charge card
    pub charge_soc_limit:     u8,
    pub charge_amps:          u8,
    pub charge_eco:           bool,
}

fn default_rtd_start_time() -> String { "--:--".to_string() }
fn default_v2h_soc_max_boost() -> u8 { 80 }
fn default_smart_export_soc_min() -> u8 { 31 }

impl Default for OperatorSettings {
    fn default() -> Self {
        Self {
            v2h_soc_min:          31,
            v2h_soc_max:          90,
            v2h_soc_max_boost:    80,
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
            smart_export_soc_min: 31,
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
        self.v2h_soc_max_boost  = self.v2h_soc_max_boost.clamp(min_soc, self.v2h_soc_max);
        self.smart_export_soc_min = self.smart_export_soc_min.clamp(self.v2h_soc_min, self.v2h_soc_max);
        self.charge_soc_limit   = self.charge_soc_limit.clamp(min_soc, max_soc);
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
/// Call from both the WebSocket handler and MQTT handler.
pub async fn update(mut new_settings: OperatorSettings) {
    new_settings.validate_and_clamp();
    log::info!("{TAG} update received | {}", to_json(&new_settings));
    *OPERATOR_SETTINGS.write().await = new_settings.clone();
    if let Err(e) = save(&new_settings).await {
        log::error!("{TAG} failed to persist: {:?}", e);
    }
}

/// Partial-update counterpart to `update()`. Only fields wrapped in `Some` are applied;
/// absent fields leave the current value untouched. Used by `Cmd::SetSetting`.
pub async fn patch(p: SettingPatch) {
    let mut s = OPERATOR_SETTINGS.read().await.clone();
    if let Some(v) = p.v2h_soc_min               { s.v2h_soc_min = v; }
    if let Some(v) = p.v2h_soc_max               { s.v2h_soc_max = v; }
    if let Some(v) = p.v2h_soc_max_boost         { s.v2h_soc_max_boost = v; }
    if let Some(v) = p.v2h_max_amps              { s.v2h_max_amps = v; }
    if let Some(v) = p.self_use                  { s.self_use = v; }
    if let Some(v) = p.export_excess_solar       { s.export_excess_solar = v; }
    if let Some(v) = p.ev_drain_protection       { s.ev_drain_protection = v; }
    if let Some(v) = p.ready_to_drive            { s.ready_to_drive = v; }
    if let Some(v) = p.ready_to_drive_end_time   { s.ready_to_drive_end_time = v; }
    if let Some(v) = p.ready_to_drive_start_time { s.ready_to_drive_start_time = v; }
    if let Some(v) = p.ready_to_drive_soc        { s.ready_to_drive_soc = v; }
    if let Some(v) = p.ready_to_drive_days       { s.ready_to_drive_days = v; }
    if let Some(v) = p.off_peak_charging         { s.off_peak_charging = v; }
    if let Some(v) = p.off_peak_start            { s.off_peak_start = v; }
    if let Some(v) = p.off_peak_end              { s.off_peak_end = v; }
    if let Some(v) = p.smart_charge              { s.smart_charge = v; }
    if let Some(v) = p.smart_export              { s.smart_export = v; }
    if let Some(v) = p.smart_export_limit_w      { s.smart_export_limit_w = v; }
    if let Some(v) = p.smart_export_soc_min      { s.smart_export_soc_min = v; }
    if let Some(v) = p.smart_export_excess_solar { s.smart_export_excess_solar = v; }
    if let Some(v) = p.charge_soc_limit          { s.charge_soc_limit = v; }
    if let Some(v) = p.charge_amps               { s.charge_amps = v; }
    if let Some(v) = p.charge_eco                { s.charge_eco = v; }
    update(s).await;
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct SettingPatch {
    pub v2h_soc_min:               Option<u8>,
    pub v2h_soc_max:               Option<u8>,
    pub v2h_soc_max_boost:         Option<u8>,
    pub v2h_max_amps:              Option<u8>,
    pub self_use:                  Option<bool>,
    pub export_excess_solar:       Option<bool>,
    pub ev_drain_protection:       Option<bool>,
    pub ready_to_drive:            Option<bool>,
    pub ready_to_drive_end_time:   Option<String>,
    pub ready_to_drive_start_time: Option<String>,
    pub ready_to_drive_soc:        Option<u8>,
    pub ready_to_drive_days:       Option<[bool; 7]>,
    pub off_peak_charging:         Option<bool>,
    pub off_peak_start:            Option<String>,
    pub off_peak_end:              Option<String>,
    pub smart_charge:              Option<bool>,
    pub smart_export:              Option<bool>,
    pub smart_export_limit_w:      Option<u32>,
    pub smart_export_soc_min:      Option<u8>,
    pub smart_export_excess_solar: Option<bool>,
    pub charge_soc_limit:          Option<u8>,
    pub charge_amps:               Option<u8>,
    pub charge_eco:                Option<bool>,
}
