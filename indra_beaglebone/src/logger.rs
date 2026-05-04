use log::{LevelFilter, Log, Metadata, Record, SetLoggerError};
use serde::Serialize;
use std::collections::VecDeque;
use std::sync::{Mutex, OnceLock};
use tokio::sync::broadcast;

const BUFFER_CAP: usize = 500;

#[derive(Clone, Debug, Serialize)]
pub struct LogEntry {
    pub ts: String,      // HH:MM:SS.mmm
    pub level: String,
    pub target: String,
    pub message: String,
}

static BUFFER: Mutex<VecDeque<LogEntry>> = Mutex::new(VecDeque::new());
static BROADCAST: OnceLock<broadcast::Sender<LogEntry>> = OnceLock::new();

pub fn subscribe() -> Option<broadcast::Receiver<LogEntry>> {
    BROADCAST.get().map(|tx| tx.subscribe())
}

pub fn init(level: LevelFilter) -> Result<(), SetLoggerError> {
    let (tx, _) = broadcast::channel(512);
    let _ = BROADCAST.set(tx);
    log::set_boxed_logger(Box::new(IndraLogger { level }))?;
    log::set_max_level(level);
    Ok(())
}

struct IndraLogger {
    level: LevelFilter,
}

impl Log for IndraLogger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level() <= self.level
    }

    fn log(&self, record: &Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let now = chrono::Local::now();
        println!(
            "{} [{:<5}] [{}] {}",
            now.format("%Y-%m-%d %H:%M:%S"),
            record.level(),
            record.target(),
            record.args()
        );
        let entry = LogEntry {
            ts: now.format("%H:%M:%S%.3f").to_string(),
            level: record.level().to_string(),
            target: record.target().to_string(),
            message: record.args().to_string(),
        };
        if let Some(tx) = BROADCAST.get() {
            let _ = tx.send(entry.clone());
        }
        if let Ok(mut buf) = BUFFER.lock() {
            if buf.len() >= BUFFER_CAP {
                buf.pop_front();
            }
            buf.push_back(entry);
        }
    }

    fn flush(&self) {}
}
