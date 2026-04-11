use crate::chademo::state::Chademo; //s, ChargerState};
use crate::error::IndraError;
use crate::global_state::OperationMode;
use crate::log_error;
use crate::pre_charger::PreCharger;
use crate::meter::update_from_mqtt;

use lazy_static::lazy_static;
use serde::Serialize;
use std::ops::ControlFlow;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, RwLock};
use tokio::time::sleep;

use super::config::MqttConfig;
use crate::data_io::config::MeterConfig;


lazy_static! {
    pub static ref CHADEMO_DATA: Arc<RwLock<MqttChademo>> =
        Arc::new(RwLock::new(MqttChademo::default()));
}

#[derive(Clone, Copy, Serialize, Default, Debug)]
pub struct MqttChademo {
    pub dc_kw: f32,
    pub soc: f32,
    pub volts: f32,
    pub temp: f32,
    pub amps: f32,
    pub state: OperationMode,
    pub requested_amps: f32,
    pub fan: u8,
    pub meter_kw: f32,
}

impl MqttChademo {
    pub fn from_pre(&mut self, pre: PreCharger) -> &mut Self {
        self.dc_kw = pre.ac_power();
        self.temp = pre.get_temp();
        self.volts = pre.get_dc_output_volts();
        self.amps = pre.get_dc_output_amps();
        self.fan = pre.get_fan_percentage();
        self
    }
    pub fn from_chademo(&mut self, chademo: &Chademo) -> &mut Self {
        self.soc = *chademo.soc() as f32;
        self.state = *chademo.state();
        self.requested_amps = chademo.requested_charging_amps();
        self
    }
    pub fn from_meter(&mut self, kw: impl Into<f32>) -> &mut Self {
        self.meter_kw = kw.into();
        self
    }
}

// MQTT Meter - read the mqttConfig as well as the meter config so we can subscribe and get meter readings from mqtt if necessary
pub async fn mqtt_task(mqtt_config: MqttConfig, meter_config: MeterConfig) -> Result<(), IndraError> {
    use rumqttc::{AsyncClient, MqttOptions, QoS};

    log::info!("Starting thread: mqtt_task   | {}", tokio::task::id());
    if !mqtt_config.enabled {
        log::warn!("MQTT not enabled in config");
        return Ok(());
    }
    
	// Clone the values we need BEFORE they are moved into mqttoptions
    let client_id = mqtt_config.client_id.clone();
    let host = mqtt_config.host.clone();
    let port = mqtt_config.port;
    let mut mqttoptions = MqttOptions::new(client_id, host, port);
    mqttoptions.set_keep_alive(Duration::from_secs(5));
    let username = mqtt_config.username.clone();
    let password = mqtt_config.password.clone();
    mqttoptions.set_credentials(username, password);
    mqttoptions.set_transport(rumqttc::Transport::Tcp);
    mqttoptions.set_clean_session(true);
    let (client, mut eventloop) = AsyncClient::new(mqttoptions, 2);
    // Clone configs for the spawned task so we can still use them later in this function
    let mqtt_config_clone = mqtt_config.clone();
    let meter_config_clone = meter_config.clone();
	
	tokio::spawn(async move {
        loop {
            if let Ok(mqtt_event) = eventloop.poll().await {
                if let ControlFlow::Break(_) = handle_mqtt_event(mqtt_event, &mqtt_config_clone, &meter_config_clone).await {
                    continue;
                }
            };
        }
    });

    client
        .subscribe(&mqtt_config.sub, QoS::AtLeastOnce)
        .await
        .map_err(|e| IndraError::MqttSub(e))?;
    
	//MQTT meter Subscribe to meter topic only if source is set to "mqtt" in config.toml
    if meter_config.source == "mqtt" {
        let meter_topic = meter_config.mqtt_topic_power.clone();   // use value from config.toml
        log::debug!("MQTT meter: subscribing to topic from config:  | {}", meter_topic);

        client
            .subscribe(&meter_topic, QoS::AtMostOnce)
            .await
            .map_err(|e| IndraError::MqttSub(e))?;

        log::debug!("MQTT meter: successfully subscribed to:  | {}", meter_topic);
    } else {
        log::debug!("MQTT meter: source is not 'mqtt', skipping meter subscription");
    }


    let interval = mqtt_config.interval;
    loop {
        sleep(Duration::from_secs(interval.into())).await;

        // send basic data as json string
        let msg = match serde_json::to_string(&*CHADEMO_DATA.read().await) {
            Ok(d) => d,
            Err(e) => {
                log::error!("CHAdeMO Ser | {e}");
                continue;
            }
        };
        let topic = mqtt_config.topic.clone();
        let msg_with_space = msg.replace(":", ": "); // Add space as my log viewer formats better 
        log::debug!("MQTT Publishing Chademo Data:  | {} = {msg_with_space}", &topic);

        // spawn to avoid latency spikes
        let client_send = client.clone();
        tokio::task::spawn(async move {
            log_error!(
                "MQTT Publishing Chademo Data:",
                client_send
                    .publish(topic, QoS::AtLeastOnce, true, msg)
                    .await
                    .map_err(|e| IndraError::MqttSend(e))
            );
        });
    }
}

async fn handle_mqtt_event(mqtt_event: rumqttc::Event, mqtt_config: &MqttConfig, meter_config: &MeterConfig) -> ControlFlow<()> {
    use rumqttc::Event::*;

    match mqtt_event {
        Incoming(mqtt_in) => {
            if let rumqttc::Packet::Publish(msg) = mqtt_in {
                if let Ok(payload) = String::from_utf8(msg.payload.to_vec()) {
                    log::debug!("MQTT Message received | Topic: '{}' | Payload: '{}'", 
                               msg.topic, payload);

                    // Check if this is a command from the web GUI
                    if msg.topic == mqtt_config.sub {
                        log::warn!("MQTT : command received via MQTT on - not implemented yet ' | {}'", msg.topic);
						// use rumqttc::Packet::*;
						// log::debug!("Incoming {:?}", mqtt_in);
						// if let Publish(msg) = mqtt_in {
						//     *CARDATA.lock().await = match serde_json::from_slice::<Data>(&msg.payload) {
						//         Ok(json) => json.inner,
						//         Err(e) => {
						//             log::error!("{e:?}");
						//             return ControlFlow::Break(());
						//         }
						//     };
						// }
                    }

                    // If it's our meter topic, pass raw payload to meter.rs
                    if msg.topic == meter_config.mqtt_topic_power {
                        log::debug!("MQTT message from meter topic - Processing");
                        crate::meter::update_from_mqtt(payload).await;
                    }
                }
            }
        }

        Outgoing(_mqtt_out) => {
            // log::debug!("Outgoing {:?}", mqtt_out);
        }
    }

    ControlFlow::Continue(())
}