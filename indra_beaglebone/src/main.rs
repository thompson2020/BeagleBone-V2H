// #![allow(dead_code)]
#![allow(unused_imports)]
use chademo::{
    ev_connect,
    state::{self},
};
use data_io::{config::APP_CONFIG, db::Database, meter, mqtt, panel};
use global_state::OperationMode;
use tokio::{
    signal::unix::{signal, SignalKind},
    sync::OnceCell,
};
use std::time::Duration; 

mod api;
mod chademo;
mod data_io;
mod error;
mod global_state;
mod logger;
mod macros;
mod pre_charger;
mod scheduler;

const MAX_SOC: u8 = 100;
const MIN_SOC: u8 = 10;
const MAX_AMPS: u8 = 16;
const METER_BIAS: f32 = 0.0;

static POOL: OnceCell<Database> = OnceCell::const_new();

/*
 * Outstanding TODO
 *
 * Firmware (ev_connect.rs)
 *     Insulation tests skipped — see ev_connect.rs "insulation tests skipped !!!"
 *     ADC SPI driver needed for differential voltage across contactors / welding detection
 *     Review CHAdeMO shutdown procedure — OBD2 fault codes may be thrown by EV on session end
 *
 * Web UI (+page.svelte)
 *     Event table actions — Edit, Delete, Add Event, Update
 */

async fn setup_can_interfaces() {
    log::info!("CAN: bringing up interfaces");
    for iface in &["can0", "can1"] {
        // Take down first — ignore error, interface may already be down
        let _ = tokio::process::Command::new("ip")
            .args(["link", "set", iface, "down"])
            .status()
            .await;

        let configured = tokio::process::Command::new("ip")
            .args(["link", "set", iface, "type", "can", "bitrate", "500000"])
            .status()
            .await
            .map(|s| s.success())
            .unwrap_or(false);

        let up = tokio::process::Command::new("ip")
            .args(["link", "set", iface, "up"])
            .status()
            .await
            .map(|s| s.success())
            .unwrap_or(false);

        if configured && up {
            log::info!("CAN: {} up at 500 kbps", iface);
        } else {
            log::error!("CAN: failed to bring up {} (configured={} up={})", iface, configured, up);
        }
    }
    // Allow kernel time to initialise the interfaces before any socket is opened
    tokio::time::sleep(Duration::from_millis(500)).await;
}

#[tokio::main]
async fn main() -> Result<(), &'static str> {
    // ==================== STARTUP BANNER ====================
    let build_time = std::fs::metadata(env!("CARGO_MANIFEST_DIR"))
        .and_then(|m| m.modified())
        .map(|t| chrono::DateTime::<chrono::Local>::from(t))
        .unwrap_or_else(|_| chrono::Local::now());

    println!("\n{}", "=".repeat(70));
    println!("🚀 INDRA BEAGLEBONE V2H CHARGER");
    println!("   Version     : {}", env!("CARGO_PKG_VERSION"));
    println!("   Built       : {}", build_time.format("%Y-%m-%d %H:%M:%S"));
    println!("   Started at  : {}", chrono::Local::now().format("%Y-%m-%d %H:%M:%S %Z"));
    println!("{}", "=".repeat(70));

    #[cfg(feature = "tracing")]
    console_subscriber::ConsoleLayer::builder()
        .retention(Duration::from_secs(60))
        .server_addr(([0, 0, 0, 0], 5556))
        .init();

    #[cfg(feature = "logging-verbose")]
    logger::init(log::LevelFilter::Trace).expect("Logger init failed");
    #[cfg(all(feature = "logging-debug", not(feature = "logging-verbose")))]
    logger::init(log::LevelFilter::Debug).expect("Logger init failed");
    #[cfg(not(any(feature = "logging-verbose", feature = "logging-debug")))]
    logger::init(log::LevelFilter::Info).expect("Logger init failed");

    log::info!("=== Indra BeagleBone service starting ===");
    log::info!("Built: {}", build_time.format("%Y-%m-%d %H:%M:%S"));
    // =======================================================

    setup_can_interfaces().await;







    POOL.get_or_try_init(|| async { Database::new().await })
        .await
        .expect("SQLx error");

    let loaded_settings = data_io::operator_settings::load().await;
    *data_io::operator_settings::OPERATOR_SETTINGS.write().await = loaded_settings;

    let (led_tx, led_rx) = statics::led_channel();
    let (mode_tx, mode_rx) = statics::chademo_channel();
    let (events_tx, events_rx) = statics::events_channel();

    let app_config = &APP_CONFIG.clone();

    let _pca9552_reset = state::pin_init_out_high(state::RESETPCAPIN).unwrap();
    let _master = state::pin_init_out_high(state::MASTERCONTACTOR).unwrap();

    // Start LED handler early so the E-Stop check below can show status on the panel
    tokio::spawn(panel::panel_event_listener(led_rx, mode_tx.clone()));
    tokio::time::sleep(Duration::from_millis(300)).await; // allow PCA9552 init to complete

    // If E-Stop is still held from a previous trip, park here until it is released.
    // With Restart=always the service restarts after an E-Stop exit; this block
    // prevents an infinite restart loop by waiting for the pin to go high first.
    {
        use sysfs_gpio::{Direction, Pin};
        let estop = Pin::new(chademo::state::ESTOPPIN);
        let _ = estop.export();
        let _ = estop.set_direction(Direction::In);
        if estop.get_value().unwrap_or(1) == 0 {
            log::warn!("[ESTOP] E-Stop held at startup — waiting for release (red blink on panel)");
            let _ = led_tx.send(panel::LedCommand::Logo(panel::State::Initialising)).await;
            loop {
                tokio::time::sleep(Duration::from_millis(200)).await;
                if estop.get_value().unwrap_or(0) == 1 {
                    log::info!("[ESTOP] E-Stop released — resuming startup");
                    break;
                }
            }
        }
        let _ = estop.unexport();
    }

    tokio::spawn(meter::meter(app_config.meter.clone(), mode_tx.clone())); // rtu over tcp SDM230 modbus meter
    tokio::spawn(panel::monitor_estop(led_tx.clone(), mode_tx.clone()));
    tokio::spawn(scheduler::init(events_rx, mode_tx.clone()));
    tokio::spawn(api::run(events_tx, mode_tx.clone()));
    tokio::spawn(data_io::db::init(10_000));
    tokio::spawn(mqtt::mqtt_task(app_config.mqtt.clone(), mode_tx.clone()));
    tokio::spawn(data_io::supervisor::supervisor_task());
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;

    let mut ctrl_c =
        signal(SignalKind::interrupt()).expect("Failed to create Ctrl-C signal handler");
    let eb = mode_tx.clone();
    tokio::spawn(async move {
        loop {
            ctrl_c.recv().await;
            log::warn!("CTRL-C caught - sending Quit instruction");
            let _ = eb.send(OperationMode::Quit).await;
            ctrl_c.recv().await;
            log::warn!("CTRL-C caught again - forcing exit");
            std::process::exit(1)
        }
    });

    let mut sigterm =
        signal(SignalKind::terminate()).expect("Failed to create SIGTERM signal handler");
    let eb = mode_tx.clone();
    tokio::spawn(async move {
        sigterm.recv().await;
        log::warn!("SIGTERM received - sending Quit for clean shutdown");
        let _ = eb.send(OperationMode::Quit).await;
    });

    // Final loop
    ev_connect::ev100ms(led_tx, mode_rx)
        .await
        .map_err(|_| &*"ev100ms thread died")
}

pub mod statics {
    use std::sync::Arc;

    use tokio::sync::{mpsc, Mutex};
    use tokio_socketcan::CANFrame;

    use crate::{
        data_io::{db::ChademoDbRow, panel::LedCommand},
        global_state::OperationMode,
        pre_charger::PreCommand,
        scheduler::Events,
    };

    pub type Channel<T> = (mpsc::Sender<T>, mpsc::Receiver<T>);
    pub type PreRx = mpsc::Receiver<PreCommand>;
    pub type PreTx = mpsc::Sender<PreCommand>;
    pub type PreChannel = Channel<PreCommand>;
    pub type ChademoRx = mpsc::Receiver<OperationMode>;
    pub type ChademoTx = mpsc::Sender<OperationMode>;
    pub type ChademoChannel = Channel<OperationMode>;
    pub type LedChannel = Channel<LedCommand>;
    pub type LedRx = mpsc::Receiver<LedCommand>;
    pub type LedTx = mpsc::Sender<LedCommand>;
    pub type EventsRx = mpsc::Receiver<Events>;
    pub type EventsTx = mpsc::Sender<Events>;
    pub type EventsChannel = Channel<Events>;

    pub type PreRxMutex = Arc<Mutex<PreRx>>;

    pub fn chademo_channel() -> ChademoChannel {
        mpsc::channel::<OperationMode>(100)
    }
    pub fn pre_channel() -> PreChannel {
        mpsc::channel::<PreCommand>(100)
    }
    pub fn led_channel() -> LedChannel {
        mpsc::channel::<LedCommand>(100)
    }
    pub fn events_channel() -> EventsChannel {
        mpsc::channel::<Events>(100)
    }

    // pub fn mpsc_channel<T>(buf: usize) -> Channel<T> {
    //     mpsc::channel::<T>(buf)
    // }

    pub fn mutex<T>(i: T) -> Arc<Mutex<T>> {
        Arc::new(Mutex::new(i))
    }
}
