use crate::global_state::OperationMode;
use super::operator_settings::OperatorSettings;
use serde::Serialize;

#[derive(Serialize, Debug)]
pub struct ChargerSnapshot {
    // From CHADEMO (car / CHAdeMO protocol state)
    pub soc: f32,
    pub state: OperationMode,
    pub requested_amps: f32,

    // From PREDATA (pre-charger / DC-DC converter)
    pub dc_kw: f32,
    pub volts: f32,
    pub temp: f32,
    pub amps: f32,
    pub fan: u8,

    // From METER (grid power meter)
    pub meter_kw: f32,
    pub phase_w: Option<f32>,

    // From SUPERVISORY (Home Assistant commands)
    pub smart_charge: bool,
    pub ev_drain_protection: bool,

    // From OPERATOR_SETTINGS (web UI / future MQTT)
    pub settings: OperatorSettings,
}

pub async fn snapshot() -> ChargerSnapshot {
    let chademo   = crate::chademo::state::CHADEMO.lock().await;
    let pre       = crate::pre_charger::PREDATA.lock().await;
    let meter     = super::meter::METER.read().await;
    let sup       = super::mqtt::SUPERVISORY.read().await;
    let settings  = super::operator_settings::OPERATOR_SETTINGS.read().await.clone();

    ChargerSnapshot {
        soc:             *chademo.soc() as f32,
        state:           *chademo.state(),
        requested_amps:  chademo.requested_charging_amps(),

        dc_kw:           pre.ac_power(),
        volts:           pre.get_dc_output_volts(),
        temp:            pre.get_temp(),
        amps:            pre.get_dc_output_amps(),
        fan:             pre.get_fan_percentage(),

        meter_kw:        meter.total_w.unwrap_or(0.0),
        phase_w:         meter.phase_w,

        smart_charge:        sup.smart_charge,
        ev_drain_protection: sup.ev_drain_protection,

        settings,
    }
}
