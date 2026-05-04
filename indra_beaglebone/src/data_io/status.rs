use crate::{global_state::OperationMode, pre_charger::PreState};
use super::operator_settings::OperatorSettings;
use serde::Serialize;

#[derive(Serialize, Debug)]
pub struct ChargerSnapshot {
    // From CHADEMO (car / CHAdeMO protocol state)
    pub soc: f32,
    pub state: OperationMode,
    pub requested_amps: f32,

    // From PREDATA (pre-charger / DC-DC converter)
    pub pre_state:           PreState,
    pub dc_volts_setpoint:   f32,
    pub dc_amps_setpoint:    f32,
    pub dc_output_volts:     f32,
    pub dc_output_amps:      f32,
    pub dc_w:                f32,
    pub dc_bus_volts:        f32,
    pub ac_amps:             f32,
    pub pre_temp:            f32,
    pub pre_fan:             u8,
    pub pre_enabled:         bool,
    pub pre_status_ok:       bool,
    pub pre_status:          [u8; 2],

    // From METER (grid power meter)
    pub meter_kw: f32,
    pub phase_w: Option<f32>,

    // From METER (charger sub-meter: SDM230 via mbmd)
    pub charger_v: Option<f32>,
    pub charger_a: Option<f32>,
    pub charger_w: Option<f32>,
    pub efficiency: Option<f32>,

    // From SUPERVISORY (Home Assistant commands + internal computation)
    pub smart_charge_request: bool,
    pub smart_charge_active: bool,

    pub ev_drain_protection_request: bool,
    pub ev_drain_protection_active: bool,
    
    pub smart_export_request: bool,
    pub smart_export_active: bool,

    pub smart_export_excess_solar_request: bool,
    pub smart_export_excess_solar_active: bool,

    pub ready_to_drive_request: bool,
    pub ready_to_drive_active: bool,

    pub off_peak_charging_request: bool,
    pub off_peak_charging_active: bool,

    // From OPERATOR_SETTINGS (web UI / future MQTT)
    pub settings: OperatorSettings,
}

pub async fn snapshot() -> ChargerSnapshot {
    let chademo   = crate::chademo::state::CHADEMO.lock().await;
    let pre       = crate::pre_charger::PREDATA.lock().await;
    let meter     = super::meter::METER.read().await;
    let sup       = super::supervisor::SUPERVISORY.read().await;
    let settings  = super::operator_settings::OPERATOR_SETTINGS.read().await.clone();

    ChargerSnapshot {
        soc:             *chademo.soc() as f32,
        state:           *chademo.state(),
        requested_amps:  chademo.requested_charging_amps(),

        pre_state:           *pre.get_state(),
        dc_volts_setpoint:   pre.get_dc_setpoint_volts(),
        dc_amps_setpoint:    pre.get_dc_setpoint_amps(),
        dc_output_volts:     pre.get_dc_output_volts(),
        dc_output_amps:      pre.get_dc_output_amps(),
        dc_w:                pre.dc_power(),
        dc_bus_volts:        pre.get_dc_bus_volts(),
        ac_amps:             pre.get_ac_amps(),
        pre_temp:            pre.get_temp(),
        pre_fan:             pre.get_fan_percentage(),
        pre_enabled:         pre.enabled(),
        pre_status_ok:       pre.status_ok(),
        pre_status:          *pre.get_status(),

        meter_kw:        meter.total_w.unwrap_or(0.0),
        phase_w:         meter.phase_w,

        charger_v:       meter.charger_v,
        charger_a:       meter.charger_a,
        charger_w:       meter.charger_w,
        efficiency:      meter.efficiency,

        smart_charge_request:        sup.smart_charge_request,
        smart_charge_active:         sup.smart_charge_active,

        ev_drain_protection_request: sup.ev_drain_protection_request,
        ev_drain_protection_active:  sup.ev_drain_protection_active,

        smart_export_request:                    sup.smart_export_request,
        smart_export_active:                     sup.smart_export_active,

        smart_export_excess_solar_request:       sup.smart_export_excess_solar_request,
        smart_export_excess_solar_active:        sup.smart_export_excess_solar_active,

        ready_to_drive_request:      sup.ready_to_drive_request,
        ready_to_drive_active:       sup.ready_to_drive_active,
        off_peak_charging_request:   sup.off_peak_charging_request,
        off_peak_charging_active:    sup.off_peak_charging_active,

        settings,
    }
}
