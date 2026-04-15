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

    pub mqtt_meter: bool,
    pub mqtt_meter_total_power_topic: String,
    pub mqtt_meter_total_power_field: String,
    pub mqtt_meter_phase_power_topic: String,
    pub mqtt_meter_phase_power_field: String,
}


#[derive(Debug, Deserialize, Clone)]
pub struct MeterConfig {
    pub modbus_meter: bool,
    pub address: String,
	
    pub mqtt_meter_timeout_seconds: u64,         // ← our 120 second timeout for mqtt meter readings, added to config.toml
}

#[derive(Debug, Deserialize, Clone)]
pub struct AppConfig {
    pub mqtt: MqttConfig,
    pub meter: MeterConfig,
}
