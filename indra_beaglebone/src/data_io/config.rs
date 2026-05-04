use serde::Deserialize;
use std::sync::Arc;
use std::{fs, panic};

lazy_static::lazy_static! {
    pub static ref APP_CONFIG: Arc<AppConfig> = {
        let config_file = "config.toml";
        let toml_str = fs::read_to_string(config_file)
            .expect(&format!("Failed to read configuration file:  | {}", config_file));
        let config = match toml::from_str(&toml_str) {
            Ok(t) => t,
            Err(e) => panic!("TOML parse fail {e:?}"),
        };
        Arc::new(config)
    };
}

//


#[derive(Debug, Deserialize, Clone)]
pub struct MqttConfig {
    pub enabled: bool,
    pub host: String,
    pub port: u16,
    pub client_id: String,
    pub username: String,
    pub password: String,
    pub interval: u32,
    pub topic: String,
    pub sub: String,

 

    pub mqtt_smart_charge_topic: String,
    pub mqtt_smart_charge_field: String,

    pub mqtt_ev_drain_protection_topic: String,
    pub mqtt_ev_drain_protection_field: String,

    pub mqtt_smart_export_topic: String,
    pub mqtt_smart_export_field: String,

    pub mqtt_smart_export_excess_solar_topic: String,
    pub mqtt_smart_export_excess_solar_field: String,


    //remove
    //pub mqtt_meter: bool,
    //pub mqtt_meter_total_power_topic: String,
    //pub mqtt_meter_total_power_field: String,
    //pub mqtt_meter_total_power_scale: f32,

    //pub mqtt_meter_phase_power_topic: String,
    //pub mqtt_meter_phase_power_field: String,
    //pub mqtt_meter_phase_power_scale: f32,
    pub mqtt_timeout_seconds: u64,         // ← our 120 second timeout for mqtt meter readings, added to config.toml


}


#[derive(Debug, Deserialize, Clone)]
pub struct MeterConfig {
    pub meter_type: String,                 // "modbus" or "mqtt"
    // Modbus-specific config:
    pub address: String,
	
    //mqtt specific:
    pub mqtt_meter_host: String,
    pub mqtt_meter_port: u16,
    pub mqtt_meter_client_id: String,
    pub mqtt_meter_username: String,
    pub mqtt_meter_password: String,

    pub mqtt_meter_total_power_topic: String,
    pub mqtt_meter_total_power_field: String,
    pub mqtt_meter_total_power_scale: f32,

    pub mqtt_meter_phase_power_topic: String,
    pub mqtt_meter_phase_power_field: String,
    pub mqtt_meter_phase_power_scale: f32,
    pub mqtt_meter_timeout_seconds: u64,

    // Charger-dedicated sub-meter (e.g. SDM230 via mbmd). Leave empty to disable.
    pub mqtt_meter_charger_volts_topic: String,
    pub mqtt_meter_charger_current_topic: String,
    pub mqtt_meter_charger_power_topic: String,
    pub mqtt_meter_charger_power_scale: f32,    // 1.0 for Watts, 1000.0 if mbmd publishes kW
}

#[derive(Debug, Deserialize, Clone)]
pub struct AppConfig {
    pub mqtt: MqttConfig,
    pub meter: MeterConfig,
}
