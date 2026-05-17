use super::config::MeterConfig;
use crate::{error::IndraError, global_state::OperationMode, statics::ChademoTx};
use rumqttc::{AsyncClient, Event, Incoming, MqttOptions, QoS};
use serde_json::Value;
use std::{net::SocketAddr, sync::Arc};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    sync::RwLock,
    time::{sleep, timeout, Duration, Instant},
};


#[derive(Clone, Copy, Default, Debug, serde::Serialize)]
pub struct MeterState {
    pub total_w: Option<f32>,
    pub phase_w: Option<f32>,
    #[serde(skip)]
    pub last_total_update: Option<Instant>,
    #[serde(skip)]
    pub last_phase_update: Option<Instant>,

    // Charger-dedicated sub-meter (SDM230 via mbmd)
    pub charger_v: Option<f32>,
    pub charger_a: Option<f32>,
    pub charger_w: Option<f32>,
    pub efficiency: Option<f32>,  // charger_w / dc_w * 100
}

lazy_static::lazy_static! {
    pub static ref METER: Arc<RwLock<MeterState>> = Arc::new(RwLock::new(MeterState::default()));
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

                log::debug!("Meter value  | {} ", val);
                {
                    let mut meter = METER.write().await;
                    meter.total_w = Some(val);
                    meter.last_total_update = Some(Instant::now());
                }
                if instant.elapsed() < Duration::from_millis(500) {
                    sleep(Duration::from_millis(500) - instant.elapsed()).await
                }
            }
            METER.write().await.total_w = None;
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

        // Run the eventloop inline -- this keeps `client` alive for the lifetime of this task.
        // Subscriptions are done inside ConnAck so they are re-issued on every reconnect.
        loop {
            match eventloop.poll().await {
                Ok(Event::Incoming(rumqttc::Packet::ConnAck(_))) => {
                    // Fires on first connect and on every reconnect. With clean_session(true)
                    // the broker discards subscriptions on disconnect, so we must resubscribe here.
                    log::info!("MQTT Meter: connected/reconnected -- subscribing to topics");
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
                    for topic in [
                        &meter_config.mqtt_meter_charger_volts_topic,
                        &meter_config.mqtt_meter_charger_current_topic,
                        &meter_config.mqtt_meter_charger_power_topic,
                    ] {
                        if !topic.is_empty() {
                            if let Err(e) = client.subscribe(topic.as_str(), QoS::AtMostOnce).await {
                                log::error!("MQTT Meter: failed to subscribe to charger topic {topic} | {e:?}");
                            } else {
                                log::info!("MQTT Meter: subscribed | {topic}");
                            }
                        }
                    }
                }
                Ok(Event::Incoming(Incoming::Publish(msg))) => {
                    handle_publish(msg, &meter_config).await;
                }
                Ok(_) => {}
                Err(e) => {
                    // Connection dropped -- rumqttc will reconnect automatically.
                    // Log it so disconnects are visible, then keep polling.
                    log::error!("MQTT Meter: connection error | {e:?}");
                }
            }
        }
    }

}

/// Extract a watt value from either a plain number ("1234.5") or a JSON payload
/// with a dotted field path ("sensor.power.total").  Returns None and logs a
/// warning if the payload cannot be parsed or the path is missing.
fn extract_value(payload: &str, field: &str) -> Option<f32> {
    if let Ok(v) = payload.parse::<f32>() {
        return Some(v);
    }
    if let Ok(json) = serde_json::from_str::<Value>(payload) {
        let mut cur = &json;
        for key in field.split('.') {
            cur = match cur.get(key) {
                Some(v) => v,
                None => {
                    log::warn!("MQTT Meter: JSON path '{}' not found | payload: {}", field, payload);
                    return None;
                }
            };
        }
        if let Some(v) = cur.as_f64() {
            return Some(v as f32);
        }
        log::warn!("MQTT Meter: JSON field '{}' is not a number | payload: {}", field, payload);
        return None;
    }
    log::warn!("MQTT Meter: payload is not a number or JSON | {}", payload);
    None
}

/// Handle a single MQTT Publish message.  Checks the topic against the
/// configured total and phase topics -- both `if` blocks can fire on the same
/// message when total and phase share a topic but use different fields.
async fn handle_publish(msg: rumqttc::mqttbytes::v4::Publish, cfg: &MeterConfig) {
    let payload = String::from_utf8_lossy(&msg.payload);
    let payload = payload.trim().replace(['\n', '\r'], "");
    log::debug!("MQTT Meter: publish | topic: '{}' payload: '{}'", msg.topic, payload);

    if msg.topic == cfg.mqtt_meter_total_power_topic {
        if let Some(v) = extract_value(&payload, &cfg.mqtt_meter_total_power_field) {
            let scaled = v * cfg.mqtt_meter_total_power_scale;
            let mut m = METER.write().await;
            m.total_w = Some(scaled);
            m.last_total_update = Some(Instant::now());
        }
    }

    if msg.topic == cfg.mqtt_meter_phase_power_topic {
        if let Some(v) = extract_value(&payload, &cfg.mqtt_meter_phase_power_field) {
            let scaled = v * cfg.mqtt_meter_phase_power_scale;
            let mut m = METER.write().await;
            m.phase_w = Some(scaled);
            m.last_phase_update = Some(Instant::now());
        }
    }

    if !cfg.mqtt_meter_charger_volts_topic.is_empty() && msg.topic == cfg.mqtt_meter_charger_volts_topic {
        if let Ok(v) = payload.parse::<f32>() {
            METER.write().await.charger_v = Some(v);
        }
    }

    if !cfg.mqtt_meter_charger_current_topic.is_empty() && msg.topic == cfg.mqtt_meter_charger_current_topic {
        if let Ok(v) = payload.parse::<f32>() {
            METER.write().await.charger_a = Some(v);
        }
    }

    if !cfg.mqtt_meter_charger_power_topic.is_empty() && msg.topic == cfg.mqtt_meter_charger_power_topic {
        if let Ok(v) = payload.parse::<f32>() {
            let scaled = v * cfg.mqtt_meter_charger_power_scale;
            let dc_w = crate::pre_charger::PREDATA.lock().await.dc_power();
            let efficiency = if dc_w.abs() > 10.0 && scaled.abs() > 10.0 {
                if dc_w > 0.0 {
                    Some(dc_w.abs() / scaled.abs() * 100.0)  // charging: dc out / ac in
                } else {
                    Some(scaled.abs() / dc_w.abs() * 100.0)  // discharging: ac out / dc in
                }
            } else {
                None
            };
            let mut m = METER.write().await;
            m.charger_w = Some(scaled);
            m.efficiency = efficiency;
            log::debug!("MQTT Meter: charger {:.2}W  efficiency {:?}%", scaled, efficiency);
        }
    }
}





async fn mark_total_power_as_stale() {
    METER.write().await.total_w = None;
    log::error!("MQTT Meter: updated: total power is STALE -> treating as offline");
}
async fn mark_phase_power_as_stale() {
    METER.write().await.phase_w = None;
    log::error!("MQTT Meter: phase power STALE -- treating as offline");
}









/// Returns true if `last` is older than `timeout_seconds` (or was never set).
/// Logs at debug/warn level so the caller just acts on the bool.
fn is_stale(last: Option<Instant>, timeout_seconds: u64, label: &str) -> bool {
    match last {
        Some(t) => {
            let age = t.elapsed().as_secs();
            if age > timeout_seconds {
                log::warn!("Meter staleness: {} STALE ({}s > {}s)", label, age, timeout_seconds);
                true
            } else {
                false
            }
        }
        None => false, // no data yet -- nothing to mark stale
    }
}

// Background task to check if any MQTT Meter data is stale
async fn start_meter_staleness_checker(meter_config: MeterConfig, mode_tx: ChademoTx) {
    loop {
        tokio::time::sleep(tokio::time::Duration::from_secs(10)).await;

        let timeout = meter_config.mqtt_meter_timeout_seconds;
        let snapshot = *METER.read().await;

        if is_stale(snapshot.last_total_update, timeout, "total power") {
            let current_mode = *crate::chademo::state::CHADEMO.lock().await.state();
            if current_mode == OperationMode::V2h {
                log::warn!("Meter staleness: total power stale + V2H active -> forcing Idle");
                let _ = mode_tx.send(OperationMode::Idle).await;
            }
            mark_total_power_as_stale().await;
        }

        if is_stale(snapshot.last_phase_update, timeout, "phase power") {
            mark_phase_power_as_stale().await;
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
