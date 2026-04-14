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
// MQTT meter additions
use serde_json::Value;
use rumqttc::{AsyncClient, Event, Incoming, MqttOptions, QoS};

lazy_static::lazy_static! {
    pub static ref METER: Arc<RwLock<Option<f32>>> = Arc::new(RwLock::new(Some(0f32)));
}

// MQTT meter additions
lazy_static::lazy_static! {
    pub static ref LAST_METER_UPDATE: Arc<Mutex<Instant>> = 
        Arc::new(Mutex::new(Instant::now()));
}

pub async fn meter(config: MeterConfig) -> Result<(), IndraError> {
    log::info!("Starting thread: meter   | {}", tokio::task::id());


    // MQTT meter additions - Check which meter source to use
    match config.source.as_str() {
        "mqtt" => {
            log::info!("Using MQTT meter source: | MQTT");
            // meter subscribe is now handled in mqtt.rs
            tokio::spawn(start_meter_staleness_checker(config));
            return Ok(());
        }
        "modbus" | _ => {
            log::info!("Using (default) meter source: | Modbus");
            // Existing Modbus code continues below...
        }
    }


    // let config = &APP_CONFIG.clone();s
    let address = config.address.clone();
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
            {
                *METER.clone().write().await = Some(val);
            }
            {
                CHADEMO_DATA.clone().write().await.from_meter(val);
            }
            if instant.elapsed() < Duration::from_millis(500) {
                sleep(Duration::from_millis(500) - instant.elapsed()).await
            }
        }
        *METER.clone().write().await = None;
        drop(stream)
    }
}

// MQTT additions - Called from mqtt.rs when a new meter value arrives via MQTT
pub async fn update_from_mqtt(payload: String) {
    let payload_trim = payload.trim();

    if let Ok(json) = serde_json::from_str::<Value>(payload_trim) {
        //extract power from "status = { "power": 1005, "powerl1": 218, "powerl2": 350, "powerl3": 437, "timestamp": 1776202878"
        let val = match json.get("power").and_then(|v| v.as_f64()) {
            Some(v) => v as f32,
            None => {
                log::warn!("meter: invalid JSON from MQTT: {}", payload_trim);
                return;
            }
        };
    
        *METER.write().await = Some(val);
        CHADEMO_DATA.write().await.from_meter(val);
        *LAST_METER_UPDATE.lock().await = Instant::now();   
        log::debug!("meter: updated value from MQTT | {:.2} W", val);
    } else {
        log::warn!("meter: failed to parse meter value from MQTT:  | {}", payload_trim);
    }
}

// Background task to check if meter data is stale
pub async fn start_meter_staleness_checker(meter_config: MeterConfig) {
    loop {
        sleep(Duration::from_secs(10)).await;

        let age = LAST_METER_UPDATE.lock().await.elapsed().as_secs();
        log::debug!("MQTT meter: data age  | {} seconds", age);
        if age > meter_config.mqtt_meter_timeout_seconds {
            log::error!("MQTT meter: data is stale - treating as offline  | {} seconds", age);
            *METER.write().await = None;
            CHADEMO_DATA.write().await.from_meter(0.0);
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
