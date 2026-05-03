use super::config::MeterConfig;
use crate::data_io::mqtt::CHADEMO_DATA;
use crate::error::IndraError;
use std::{net::SocketAddr, sync::Arc};
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::sync::RwLock;
use tokio::time::timeout;
use tokio::time::Instant;
use tokio::{
    net::TcpStream,
    sync::Mutex,
    time::{sleep, Duration},
};
// MQTT Meter additions
use serde_json::Value;
use rumqttc::{AsyncClient, Event, Incoming, MqttOptions, QoS};

use crate::global_state::OperationMode;
use crate::statics::ChademoTx;


lazy_static::lazy_static! {
    pub static ref METER: Arc<RwLock<Option<f32>>> = Arc::new(RwLock::new(Some(0f32)));
}

pub async fn meter(meter_config: MeterConfig, mode_tx: ChademoTx) -> Result<(), IndraError> {
    log::info!("Starting thread: meter   | {}", tokio::task::id());


    if meter_config.meter_type.to_lowercase() != "mqtt" {
        ////////////////////////////////////////////////
        //      modbus meter code
        ////////////////////////////////////////////////
        log::info!("Modbus meter enabled | Modbus");

        // let config = &APP_CONFIG.clone();s
        let address = meter_config.address.clone();
        let socket_addr: SocketAddr = address
            .parse::<SocketAddr>()
            .map_err(|e| IndraError::SocketError(e))?;
        log::info!(
            "Connecting to RTU meter:  | IP:{:?} port:{}",
            socket_addr.ip(),
            socket_addr.port()
        );
        loop {
            let mut stream = TcpStream::connect(socket_addr)
                .await
                .map_err(|e| IndraError::SocketConnectError(e))?;
            let (mut rx, mut tx) = stream.split();

            // Raw modbus params for SDM230 @ 1hz
            let device_id = 1;
            let function_code = 0x04; // Read Holding Registers
            let starting_address = 0x0c;
            let quantity = 2;

            let request =
                energy_modbus_rtu_request(device_id, function_code, starting_address, quantity);
            log::info!("SDM230 modbus PDU: | {request:02x?}");
            let mut val = 0.1f32;

            'inner: loop {
                let mut buf = [0u8; 24];
                let instant = Instant::now();
                if let Err(e) = tx.write(&request).await {
                    log::error!("TCP write error | {e:?}");
                    break 'inner;
                }

                match timeout(Duration::from_millis(400), rx.read(&mut buf)).await {
                    Ok(Ok(_)) => {
                        // Strange blank meter readings
                        if buf[3..=6] != [0, 0, 0, 0] {
                            val =
                                f32::from_be_bytes(buf[3..=6].try_into().unwrap_or(val.to_be_bytes()));
                        }
                    }
                    Err(e) => {
                        log::error!("Meter TCP timeout | {e:?}");
                        break 'inner;
                    }
                    _ => {
                        log::error!("Meter TCP read error");
                        break 'inner;
                    }
                };

                log::info!("Meter value  | {} ", val);
                *METER.write().await = Some(val);
                {
                    let mut data = CHADEMO_DATA.write().await;
                    data.from_meter(val,false);
                }
                
                if instant.elapsed() < Duration::from_millis(500) {
                    sleep(Duration::from_millis(500) - instant.elapsed()).await
                }
            }
            *METER.clone().write().await = None;
            drop(stream)
        }

    }
    else {
        ////////////////////////////////////////////////
        //          mqtt meter code
        ////////////////////////////////////////////////
        log::info!("MQTT Meter enabled | MQTT");

        // Clone the values we need BEFORE they are moved into mqttoptions
        let client_id = meter_config.mqtt_meter_client_id.clone();
        let host = meter_config.mqtt_meter_host.clone();
        let port = meter_config.mqtt_meter_port;
        let mut mqttoptions = MqttOptions::new(client_id, host, port);
        mqttoptions.set_keep_alive(Duration::from_secs(5));
        let username = meter_config.mqtt_meter_username.clone();
        let password = meter_config.mqtt_meter_password.clone();
        mqttoptions.set_credentials(username, password);
        mqttoptions.set_transport(rumqttc::Transport::Tcp);
        mqttoptions.set_clean_session(true);
        let (client, mut eventloop) = AsyncClient::new(mqttoptions, 10);


        log::info!("MQTT Meter: Spawn Staleness check");
        let mode_tx_for_staleness = mode_tx.clone();
        tokio::spawn(start_meter_staleness_checker(meter_config.clone(), mode_tx_for_staleness));

        // Run the eventloop inline — this keeps `client` alive for the lifetime of this task.
        // Subscriptions are done inside ConnAck so they are re-issued on every reconnect.
        loop {
            match eventloop.poll().await {
                Ok(Event::Incoming(rumqttc::Packet::ConnAck(_))) => {
                    // Fires on first connect and on every reconnect. With clean_session(true)
                    // the broker discards subscriptions on disconnect, so we must resubscribe here.
                    log::info!("MQTT Meter: connected/reconnected — subscribing to topics");
                    if let Err(e) = client
                        .subscribe(&meter_config.mqtt_meter_total_power_topic, QoS::AtMostOnce)
                        .await
                    {
                        log::error!("MQTT Meter: failed to subscribe to total power topic | {e:?}");
                    } else {
                        log::info!("MQTT Meter: subscribed | {}", meter_config.mqtt_meter_total_power_topic);
                    }
                    if meter_config.mqtt_meter_phase_power_topic != meter_config.mqtt_meter_total_power_topic {
                        if let Err(e) = client
                            .subscribe(&meter_config.mqtt_meter_phase_power_topic, QoS::AtMostOnce)
                            .await
                        {
                            log::error!("MQTT Meter: failed to subscribe to phase power topic | {e:?}");
                        } else {
                            log::info!("MQTT Meter: subscribed | {}", meter_config.mqtt_meter_phase_power_topic);
                        }
                    }
                }
                Ok(event) => {
                    handle_mqtt_meter_event(event, &meter_config).await;
                }
                Err(e) => {
                    // Connection dropped — rumqttc will reconnect automatically.
                    // Log it so disconnects are visible, then keep polling.
                    log::error!("MQTT Meter: connection error | {e:?}");
                }
            }
        }
    }

}

async fn handle_mqtt_meter_event(mqtt_event: rumqttc::Event, meter_config: &MeterConfig) {
    use rumqttc::Event::*;

    match mqtt_event {
        Incoming(mqtt_in) => {
            if let rumqttc::Packet::Publish(msg) = mqtt_in {
                //if let Ok(payload) = String::from_utf8(msg.payload.to_vec()) {
                let payload_str = String::from_utf8_lossy(&msg.payload);
                let clean_payload = payload_str.trim().replace(['\n', '\r'], "");
                let readable_payload = clean_payload .replace(",", ", ").replace(":", ": ");
                log::debug!("MQTT Meter: Message received | Topic: '{}' | Payload: '{}'",  msg.topic, readable_payload);

                // If it's our total meter topic, pass raw payload to meter.rs
                if msg.topic == meter_config.mqtt_meter_total_power_topic {
                    log::debug!("MQTT Meter: message from topic: meter total (check field) | {} field: {}", msg.topic, meter_config.mqtt_meter_total_power_field);
                    update_from_mqtt(clean_payload.to_string(),msg.topic.clone(), meter_config.mqtt_meter_total_power_field.clone(), meter_config.mqtt_meter_total_power_scale.clone(), meter_config   ).await;
                }

                // If it's our phase meter topic, pass raw payload to meter.rs
                if msg.topic == meter_config.mqtt_meter_phase_power_topic {
                    log::debug!("MQTT Meter: message from topic: meter phase (check field) | {} field: {}", msg.topic, meter_config.mqtt_meter_phase_power_field);
                    update_from_mqtt(clean_payload.to_string(),msg.topic.clone(), meter_config.mqtt_meter_phase_power_field.clone(), meter_config.mqtt_meter_phase_power_scale.clone(), meter_config  ).await;
                }
                
            }
        }
        Outgoing(_) => {
            // ignore MQTT acks / publishes from client
        }
       
    }
}


pub async fn update_from_mqtt(payload: String, topic: String, mqtt_field: String, scale: f32, meter_config: &MeterConfig) {
    let payload_trim = payload.trim();

    // Now update the correct value
    log::debug!("MQTT Meter: Extracting topic & field: | {} , {}", topic, mqtt_field );

    // Case 1: Plain number
    let val: f32 = if let Ok(val) = payload_trim.parse::<f32>() {
        log::debug!("MQTT Meter: value extracted(plain number): | {:.2} W", val);
        val
    } 
    // Case 2: JSON object
    else if let Ok(json) = serde_json::from_str::<serde_json::Value>(payload_trim) {
        let mut current = &json;
        for key in mqtt_field.split('.') {
            current = match current.get(key) {
                Some(v) => v,
                None => {
                    log::warn!("MQTT Meter: JSON missing path '{}' | payload: {}", mqtt_field, payload_trim);
                    return;
                }
            };
        }

        match current.as_f64() {
            Some(v) => {
                let val = v as f32;
                log::debug!("MQTT Meter: value extracted(JSON field '{}') | {:.2}", mqtt_field, val);
                val
            }
            None => {
                log::warn!("MQTT Meter: JSON missing field '{}' | payload: {}", mqtt_field, payload_trim);
                return;
            }
        }
    } 
    else {
        log::warn!("MQTT Meter: failed to parse as number or JSON | payload: {}", payload_trim);
        return;
    };

    let scaled_val = val * scale;
    if scale != 1.0 {
        log::debug!("MQTT Meter: scaled value by {} | {:.2}", scale, scaled_val);
    }

    
    
    
    match (topic.as_str(), mqtt_field.as_str()) {

        // ===================== TOTAL POWER =====================
        (t, f)
            if t == meter_config.mqtt_meter_total_power_topic
            && f == meter_config.mqtt_meter_total_power_field =>
        {
            log::debug!("MQTT Meter: calling update_total_power | {}", scaled_val);
            update_total_power(scaled_val).await;
            return;
        }

        // ===================== PHASE POWER =====================
        (t, f)
            if t == meter_config.mqtt_meter_phase_power_topic
            && f == meter_config.mqtt_meter_phase_power_field =>
        {
            log::debug!("MQTT Meter: calling update_phase_power | {}", scaled_val);
            update_phase_power(scaled_val).await;
            return;
        }



        // ===================== UNKNOWN =====================
        _ => {
            log::warn!("MQTT Meter: unknown topic/field | {} / {}", topic, mqtt_field);
        }
    }



}








// MQTT update events
pub async fn update_total_power(val: f32) {
    {
        let mut meter = METER.write().await;
        *meter = Some(val);
    }
  {
        let mut data = CHADEMO_DATA.write().await;
        data.from_meter(val, false);
    }
    log::debug!("MQTT Meter: updated: total power | {:.2} W", val);
}
pub async fn mark_total_power_as_stale() {
    *METER.write().await = None;
    {
        let mut data = CHADEMO_DATA.write().await;
        data.from_meter(0.0, true);
    }    
    log::error!("MQTT Meter: updated: total power is STALE → treating as offline");
}
pub async fn update_phase_power(val: f32) {
    {
        let mut data = CHADEMO_DATA.write().await;
        data.from_meter_phase(Some(val), false);
    }
    log::debug!("MQTT Meter: updated: phase power | {:.2} W", val);
}
pub async fn mark_phase_power_as_stale() {
    {
        let mut data = CHADEMO_DATA.write().await;
        data.from_meter_phase(None, true);
    }
    log::error!("MQTT Meter: updated: phase power to stale (treating as offline) | Stale ");
}









// Background task to check if any MQTT Meter data is stale
pub async fn start_meter_staleness_checker(meter_config: MeterConfig, mode_tx: ChademoTx) {
    loop {
        tokio::time::sleep(tokio::time::Duration::from_secs(10)).await;

        let timeout_seconds = meter_config.mqtt_meter_timeout_seconds;

        log::debug!("Meter Staleness check: running staleness check (timeout = {}s)", timeout_seconds);

        let snapshot = {
            let data = CHADEMO_DATA.read().await;
            *data
        };


       // Check total power staleness
        {
            log::debug!("Meter Staleness check: Starting total power check");

            //let data = CHADEMO_DATA.read().await;
            //log::debug!("Meter Staleness check: Acquired read lock on CHADEMO_DATA");

            if let Some(last_update) = snapshot.last_total_power_update {
                log::debug!("Meter Staleness check: last_total_power_update found");

                let age = last_update.elapsed().as_secs();
                log::debug!("Meter Staleness check: total power age = {} seconds", age);

                if age > timeout_seconds {
                    log::warn!("Meter Staleness check:  total power data is STALE (age = {}s > {}s timeout)", 
                            age, timeout_seconds);

                    // Safety: Only force Idle if currently in V2H
                    if snapshot.state == OperationMode::V2h {
                        log::warn!("Meter Staleness check: Meter stale + currently in V2H → forcing Idle for safety");
                        let _ = mode_tx.send(OperationMode::Idle).await;
                    } else {
                        log::info!("Meter Staleness check: Meter stale, but current mode is {:?} → no action taken (only V2H affected)", 
                                   snapshot.state);
                    }
                    log::debug!("Meter Staleness check: mark_total_power_as_stale()");
                    crate::meter::mark_total_power_as_stale().await;
                } else {
                    log::debug!("Meter Staleness check: total power data is FRESH (age = {}s ≤ {}s timeout)", age, timeout_seconds);
                }
            } else {
                log::warn!("Meter Staleness check: No last_total_power_update timestamp found (never received data?)");
            }

            log::debug!("Meter Staleness check: Finished total power check");
        }




      // Check phase power staleness
        {
            //let data = CHADEMO_DATA.read().await;
            log::debug!("Meter Staleness check: Starting phase power staleness check");            
            if let Some(last_update) = snapshot.last_phase_power_update {
                let age = last_update.elapsed().as_secs();
                if age > timeout_seconds {
                    log::warn!("Meter Staleness check: phase power data is stale (age = {}s)", age);
                    crate::meter::mark_phase_power_as_stale().await;
                } else {
                    log::debug!("Meter Staleness check: phase power data is FRESH (age = {}s ≤ {}s timeout)", age, timeout_seconds);
                }   
            } else {
                log::warn!("Meter Staleness check: No last_phase_power_update timestamp found (never received data?)");
            }   
        }

    }
}







fn energy_modbus_rtu_request(
    device_id: u8,
    function_code: u8,
    starting_address: u16,
    quantity: u16,
) -> [u8; 8] {
    let mut request = [0u8; 8];
    request[0] = device_id;
    request[1] = function_code;
    [request[2], request[3]] = starting_address.to_be_bytes();
    [request[4], request[5]] = quantity.to_be_bytes();
    let crc = calculate_crc(&request[0..6]);
    [request[6], request[7]] = crc.to_le_bytes();
    request
}

#[inline]
fn calculate_crc(data: &[u8]) -> u16 {
    let mut crc = 0xFFFFu16;
    for byte in data {
        crc ^= u16::from(*byte);
        for _ in 0..8 {
            if (crc & 1) != 0 {
                crc >>= 1;
                crc ^= 0xA001;
            } else {
                crc >>= 1;
            }
        }
    }
    crc
}
