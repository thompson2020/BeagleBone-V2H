use crate::chademo::state::Chademo; //s, ChargerState};
use crate::error::IndraError;
use crate::global_state::OperationMode;
use crate::log_error;
use crate::pre_charger::PreCharger;

use lazy_static::lazy_static;
use serde::Serialize;
use std::ops::ControlFlow;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;
use tokio::sync::{Mutex, RwLock};
use tokio::time::sleep;


use super::config::MqttConfig;
use crate::data_io::config::MeterConfig;

use crate::meter::update_total_power;
use crate::meter::mark_total_power_as_stale;
use crate::meter::update_phase_power;
use crate::meter::mark_phase_power_as_stale;

use crate::statics::ChademoTx;
use crate::global_state::ChargeParameters;


lazy_static! {
    pub static ref CHADEMO_DATA: Arc<RwLock<MqttChademo>> =
        Arc::new(RwLock::new(MqttChademo::default()));
}

#[derive(Clone, Copy, Serialize, Debug)]
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
    
    pub phase_w: Option<f32>,
    pub smart_charge: bool,
    pub ev_drain_protection: bool,

    #[serde(skip_serializing)]
    pub last_total_power_update: Option<Instant>,
    #[serde(skip_serializing)]
    pub last_phase_power_update: Option<Instant>,
    #[serde(skip_serializing)]
    pub smart_charge_update: Option<Instant>,
    #[serde(skip_serializing)]
    pub ev_drain_protection_update: Option<Instant>,
    
}
impl Default for MqttChademo {
    fn default() -> Self {
        Self {
            dc_kw: 0.0,
            soc: 0.0,
            volts: 0.0,
            temp: 0.0,
            amps: 0.0,
            state: OperationMode::Idle,
            requested_amps: 0.0,
            fan: 0,
            
            meter_kw: 0.0,
            phase_w: None,
            smart_charge: false,
            ev_drain_protection: false,

            last_total_power_update: None,          //update times if they came from mqtt
            last_phase_power_update: None,
            smart_charge_update: None,
            ev_drain_protection_update: None,
        }
    }
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
    pub fn from_meter(&mut self, w: impl Into<f32>, stale: bool) -> &mut Self {                  // from meter.rs
        self.meter_kw = w.into();
        if !stale {
            self.last_total_power_update = Some(Instant::now());
        }
        self
    }
    pub fn from_meter_phase(&mut self, val: Option<f32>, stale: bool) -> &mut Self {         // from meter.rs
        self.phase_w = val.into();
        if !stale {
            self.last_phase_power_update = Some(Instant::now());
        }
        self
    }
    pub fn update_smart_charge(&mut self, enabled: bool) -> &mut Self {
        self.smart_charge = enabled;
        self
    }

    pub fn update_ev_drain_protection(&mut self, enabled: bool) -> &mut Self {
        self.ev_drain_protection = enabled;
        self
    }
}


pub async fn mqtt_task(mqtt_config: MqttConfig, mode_tx: ChademoTx) -> Result<(), IndraError> {
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
    let mode_tx_for_eventloop = mode_tx.clone();  
    let mqtt_config_clone = mqtt_config.clone();

	
	tokio::spawn(async move {
        loop {
            if let Ok(mqtt_event) = eventloop.poll().await {
                if let ControlFlow::Break(_) = handle_mqtt_event(mqtt_event, &mqtt_config_clone, mode_tx_for_eventloop.clone()).await {
                    continue;
                }
            };
        }
    });

    //Subscribe to command topic for web GUI commands
    log::info!("MQTT meter: subscribing to: command topic | {}", mqtt_config.sub);
    client
        .subscribe(&mqtt_config.sub, QoS::AtLeastOnce)
        .await
        .map_err(|e| IndraError::MqttSub(e))?;
    
	//MQTT meter Subscribe to meter topic only if mqtt_meter = enabled config.toml
    if mqtt_config.mqtt_meter  {

        //subscribe to total power topic
        log::info!("MQTT meter: subscribing to:  total power topic | {}", mqtt_config.mqtt_meter_total_power_topic);
        client.subscribe(&mqtt_config.mqtt_meter_total_power_topic, QoS::AtMostOnce)
            .await
            .map_err(|e| IndraError::MqttSub(e))?;
    
        // Subscribe to phase power topic (only if it's different from total power topic)
        if mqtt_config.mqtt_meter_phase_power_topic != mqtt_config.mqtt_meter_total_power_topic {
            log::info!("MQTT meter: subscribing to: phase power | {}", mqtt_config.mqtt_meter_phase_power_topic);
            client.subscribe(&mqtt_config.mqtt_meter_phase_power_topic, QoS::AtMostOnce)
                .await
                .map_err(|e| IndraError::MqttSub(e))?;
            }
        
        //subscribe to smart charge topic
        log::info!("MQTT meter: subscribing to: smart charge topic | {}", mqtt_config.mqtt_smart_charge_topic);
        client.subscribe(&mqtt_config.mqtt_smart_charge_topic, QoS::AtMostOnce)
            .await
            .map_err(|e| IndraError::MqttSub(e))?;

        //subscribe to ev_drain_protection topic
        log::info!("MQTT meter: subscribing to: ev_drain_protection topic | {}", mqtt_config.mqtt_ev_drain_protection_topic);
        client.subscribe(&mqtt_config.mqtt_ev_drain_protection_topic, QoS::AtMostOnce)
            .await
            .map_err(|e| IndraError::MqttSub(e))?;

        log::info!("MQTT meter: Spawn Staleness check");
        let mode_tx_for_staleness = mode_tx.clone();
        tokio::spawn(start_meter_staleness_checker(mqtt_config.clone(), mode_tx_for_staleness));
    } else {
        log::debug!("MQTT meter: mqtt_meter = false, skipping mqtt and meter subscription");
    }






    let interval = mqtt_config.interval;
    let publish_client = client.clone();           
    let publish_config = mqtt_config.clone(); 

    loop {
        sleep(Duration::from_secs(interval.into())).await;

        // send basic data as json string
        let snapshot = {
            let guard = CHADEMO_DATA.read().await;
            *guard
            };
        let msg = match serde_json::to_string(&snapshot) {
            Ok(d) => d,
            Err(e) => {
                log::error!("CHAdeMO Ser | {e}");
                continue;
            }
        };
        let topic = publish_config.topic.clone();
        let msg_with_space = msg.replace(":", ": "); 

        log::debug!("MQTT Publishing Chademo Data:  | {} = {msg_with_space}", &topic);

        // spawn to avoid latency spikes
        let client_send = publish_client.clone();
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

async fn handle_mqtt_event(mqtt_event: rumqttc::Event, mqtt_config: &MqttConfig, mode_tx: ChademoTx) -> ControlFlow<()> {
    use rumqttc::Event::*;

    match mqtt_event {
        Incoming(mqtt_in) => {
            if let rumqttc::Packet::Publish(msg) = mqtt_in {
                //if let Ok(payload) = String::from_utf8(msg.payload.to_vec()) {
                let payload_str = String::from_utf8_lossy(&msg.payload);
                let clean_payload = payload_str.trim().replace(['\n', '\r'], "");
                log::debug!("MQTT Message received | Topic: '{}' | Payload: '{}'",  msg.topic, clean_payload);

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

                // If it's our total meter topic, pass raw payload to meter.rs
                if msg.topic == mqtt_config.mqtt_meter_total_power_topic {
                    log::debug!("MQTT message from topic: meter total (check field) | {} field: {}", msg.topic, mqtt_config.mqtt_meter_total_power_field);
                    update_from_mqtt(clean_payload.to_string(),msg.topic.clone(), mqtt_config.mqtt_meter_total_power_field.clone(), mqtt_config.mqtt_meter_total_power_scale.clone(), mqtt_config, &mode_tx,  ).await;
                }

                // If it's our phase meter topic, pass raw payload to meter.rs
                if msg.topic == mqtt_config.mqtt_meter_phase_power_topic {
                    log::debug!("MQTT message from topic: meter phase (check field) | {} field: {}", msg.topic, mqtt_config.mqtt_meter_phase_power_field);
                    update_from_mqtt(clean_payload.to_string(),msg.topic.clone(), mqtt_config.mqtt_meter_phase_power_field.clone(), mqtt_config.mqtt_meter_phase_power_scale.clone(), mqtt_config, &mode_tx  ).await;
                }

                // If it's our smart_charge_topic, pass raw payload to meter.rs
                if msg.topic == mqtt_config.mqtt_smart_charge_topic {
                    log::debug!("MQTT message from topic: smart charge | {}", msg.topic);
                    update_from_mqtt(clean_payload.to_string(),msg.topic.clone(), mqtt_config.mqtt_smart_charge_field.clone(), 1.0, mqtt_config, &mode_tx  ).await;
                }

                // If it's our ev_drain_protection_topic, pass raw payload to meter.rs
                if msg.topic == mqtt_config.mqtt_ev_drain_protection_topic {
                    log::debug!("MQTT message from topic:  ev drain protection | {}", msg.topic);
                    update_from_mqtt(clean_payload.to_string(),msg.topic.clone(), mqtt_config.mqtt_ev_drain_protection_field.clone(), 1.0, mqtt_config, &mode_tx  ).await;
                }
                
            }
        }
        
        Outgoing(_mqtt_out) => {
            // log::debug!("Outgoing {:?}", mqtt_out);
        }
    }

    ControlFlow::Continue(())
}



pub async fn update_from_mqtt(payload: String, topic: String, mqtt_field: String, scale: f32, mqtt_config: &MqttConfig, mode_tx: &ChademoTx) {
    let payload_trim = payload.trim();

    // Now update the correct value
    log::debug!("MQTT: Extracting topic & field: | {} , {}", topic, mqtt_field );

    // Case 1: Plain number
    let val: f32 = if let Ok(val) = payload_trim.parse::<f32>() {
        log::debug!("MQTT: value extracted(plain number): | {:.2} W", val);
        val
    } 
    // Case 2: JSON object
    else if let Ok(json) = serde_json::from_str::<serde_json::Value>(payload_trim) {
        match json.get(&mqtt_field).and_then(|v| v.as_f64()) {
            Some(v) => {
                let val = v as f32;
                log::debug!("MQTT: value extracted(JSON field '{}') | {:.2}", mqtt_field, val);
                val
            }
            None => {
                log::warn!("MQTT: JSON missing field '{}' | payload: {}", mqtt_field, payload_trim);
                return;
            }
        }
    } 
    else {
        log::warn!("MQTT: failed to parse as number or JSON | payload: {}", payload_trim);
        return;
    };

    let scaled_val = val * scale;
    if scale != 1.0 {
        log::debug!("MQTT: scaled value by {} | {:.2}", scale, scaled_val);
    }

    
    
    
    match (topic.as_str(), mqtt_field.as_str()) {

        // ===================== TOTAL POWER =====================
        (t, f)
            if t == mqtt_config.mqtt_meter_total_power_topic
            && f == mqtt_config.mqtt_meter_total_power_field =>
        {
            log::debug!("MQTT: calling update_total_power | {}", scaled_val);
            update_total_power(scaled_val).await;
            return;
        }

        // ===================== PHASE POWER =====================
        (t, f)
            if t == mqtt_config.mqtt_meter_phase_power_topic
            && f == mqtt_config.mqtt_meter_phase_power_field =>
        {
            log::debug!("MQTT: calling update_phase_power | {}", scaled_val);
            update_phase_power(scaled_val).await;
            return;
        }

        // ===================== SMART CHARGE =====================
        (t, f)
            if t == mqtt_config.mqtt_smart_charge_topic
            && f == mqtt_config.mqtt_smart_charge_field =>
        {
            log::debug!("MQTT: updating smart charge | {}", scaled_val);

            let enabled = scaled_val > 0.0;

            {
                let mut data = CHADEMO_DATA.write().await;
                data.update_smart_charge(enabled);
            }
            
            log::debug!("MQTT: calling handle_smart_charge_change | {}", scaled_val);
            let mode_tx = mode_tx.clone();
                tokio::spawn(async move {
                    handle_smart_charge_change(enabled, &mode_tx).await;
                });
            return;
        }

        // ===================== EV PROTECTION =====================
        (t, f)
            if t == mqtt_config.mqtt_ev_drain_protection_topic
            && f == mqtt_config.mqtt_ev_drain_protection_field =>
        {
            log::debug!("MQTT: updating ev drain protection | {}", scaled_val);

            let mut data = CHADEMO_DATA.write().await;
            data.update_ev_drain_protection(scaled_val > 0.0);

            // TODO - Decide if we need to take any immediate action based on this change (e.g. if EV drain protection is enabled while in V2H, maybe force Idle mode for safety)
            return;
        }

        // ===================== UNKNOWN =====================
        _ => {
            log::warn!("MQTT: unknown topic/field | {} / {}", topic, mqtt_field);
        }
    }



}


// Handles smart charge activation from Octopus IOG slots
// Uses soft start/stop to avoid sudden 0 <-> 6.4kW reversal
pub async fn handle_smart_charge_change(enabled: bool, mode_tx: &ChademoTx) {
  
      // Use a static to remember the last known state
    static LAST_SMART_CHARGE: std::sync::atomic::AtomicBool = 
        std::sync::atomic::AtomicBool::new(false);

    let previous = LAST_SMART_CHARGE.load(std::sync::atomic::Ordering::Relaxed);

    // If the value hasn't changed → do nothing and return early
    if previous == enabled {
        log::debug!("Smart Charge: Received but value not changed - No Action taken | {}", enabled);
        return;
    }
    
    // Update the stored state
    LAST_SMART_CHARGE.store(enabled, std::sync::atomic::Ordering::Relaxed);

  
    if enabled {
        // Soft start: 1A for 60 seconds
        log::info!("Smart Charge activated - soft start at 1A for 60s");
        let mut params = ChargeParameters::default();
        params.set_amps(1);
        params.set_soc_limit(90);
        let mode = OperationMode::Charge(params);
        log::debug!("Smart Charge mode created | {:?}", mode);

        let _ = mode_tx.send(mode).await;
        
        tokio::time::sleep(tokio::time::Duration::from_secs(60)).await;

        // Ramp up to full 16A
        log::info!("Smart Charge soft start complete - ramping to 16A");
        let mut params = ChargeParameters::default();
        params.set_amps(1);
        params.set_soc_limit(90);
        let mode = OperationMode::Charge(params);
        log::debug!("Smart Charge mode created | {:?}", mode);

        let _ = mode_tx.send(mode).await;
        
        // TODO: Consider smoother ramp (e.g. 1A → 4A → 8A → 16A over time)

    } else {
        // Soft stop: reduce to 1A for 60 seconds before returning to V2H
  
        log::info!("Smart Charge ended - soft stop at 1A for 60s");
        let mut params = ChargeParameters::default();
        params.set_amps(1);
        params.set_soc_limit(90);
        let mode = OperationMode::Charge(params);
        log::debug!("Smart Charge mode created | {:?}", mode);

        let _ = mode_tx.send(mode).await;
        
        tokio::time::sleep(tokio::time::Duration::from_secs(60)).await;

        log::info!("Smart Charge soft stop complete - returning to V2H");
        let _ = mode_tx.send(OperationMode::V2h).await;
    
    }
}






















// Background task to check if any MQTT meter data is stale
pub async fn start_meter_staleness_checker(mqtt_config: MqttConfig, mode_tx: ChademoTx) {
    loop {
        tokio::time::sleep(tokio::time::Duration::from_secs(10)).await;

        let timeout_seconds = mqtt_config.mqtt_meter_timeout_seconds;

        log::debug!("MQTT meter: running staleness check (timeout = {}s)", timeout_seconds);

        // Check total power staleness
        /*{
            let data = CHADEMO_DATA.read().await;
            if let Some(last_update) = data.last_total_power_update {
                let age = last_update.elapsed().as_secs();
                if age > timeout_seconds {
                    log::warn!("MQTT meter: total power data is stale (age = {}s)", age);
                    crate::meter::mark_total_power_as_stale().await;
                } else {
                    log::debug!("MQTT meter: total power data is fresh (age = {}s)", age);
                }
            }
        }*/

        let snapshot = {
            let data = CHADEMO_DATA.read().await;
            *data
        };


       // Check total power staleness
        {
            log::debug!("Staleness check: Starting total power check");

            //let data = CHADEMO_DATA.read().await;
            //log::debug!("Staleness check: Acquired read lock on CHADEMO_DATA");

            if let Some(last_update) = snapshot.last_total_power_update {
                log::debug!("Staleness check: last_total_power_update found");

                let age = last_update.elapsed().as_secs();
                log::debug!("Staleness check: total power age = {} seconds", age);

                if age > timeout_seconds {
                    log::warn!("Staleness check:  total power data is STALE (age = {}s > {}s timeout)", 
                            age, timeout_seconds);

                    // Safety: Only force Idle if currently in V2H
                    if snapshot.state == OperationMode::V2h {
                        log::warn!("Staleness check: Meter stale + currently in V2H → forcing Idle for safety");
                        let _ = mode_tx.send(OperationMode::Idle).await;
                    } else {
                        log::info!("Staleness check: Meter stale, but current mode is {:?} → no action taken (only V2H affected)", 
                                   snapshot.state);
                    }
                    log::debug!("Staleness check: mark_total_power_as_stale()");
                    crate::meter::mark_total_power_as_stale().await;
                } else {
                    log::debug!("Staleness check: total power data is FRESH (age = {}s ≤ {}s timeout)", 
                                age, timeout_seconds);
                }
            } else {
                log::warn!("Staleness check: No last_total_power_update timestamp found (never received data?)");
            }

            log::debug!("Staleness check: Finished total power check");
        }




      // Check phase power staleness
        {
            //let data = CHADEMO_DATA.read().await;
            if let Some(last_update) = snapshot.last_phase_power_update {
                let age = last_update.elapsed().as_secs();
                if age > timeout_seconds {
                    log::warn!("MQTT meter: phase power data is stale (age = {}s)", age);
                    crate::meter::mark_phase_power_as_stale().await;
                } else {
                    log::debug!("MQTT meter: phase power data is fresh (age = {}s)", age);
                }   
            }
        }

        // Check smart_charge staleness
        {
            //let data = CHADEMO_DATA.read().await;
            if let Some(last_update) = snapshot.smart_charge_update {
                let age = last_update.elapsed().as_secs();
                if age > timeout_seconds {
                    log::warn!("MQTT meter: smart_charge data is stale (age = {}s) - setting to false", age);
                    {
                        let mut data = CHADEMO_DATA.write().await;
                        data.update_smart_charge(false);
                    }

                }
            }
        }

        // Check ev_drain_protection staleness
        {
            //let data = CHADEMO_DATA.read().await;
            if let Some(last_update) = snapshot.ev_drain_protection_update {
                let age = last_update.elapsed().as_secs();
                if age > timeout_seconds {
                    log::warn!("MQTT meter: ev_drain_protection data is stale (age = {}s) - setting to false", age);
                    {
                        let mut data = CHADEMO_DATA.write().await;
                        data.update_ev_drain_protection(false);
                    }

                }
            }
        }


    }
}