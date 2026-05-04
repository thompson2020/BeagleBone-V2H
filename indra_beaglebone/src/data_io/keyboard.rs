use crate::{
    global_state::OperationMode,
    log_error,
    pre_charger::PreCommand,
    statics::{ChademoTx, PreTx},
};
use std::io::Read;

pub async fn scan_kb(pre_tx: PreTx, mode_tx: ChademoTx) {
    loop {
        // Input: c for manual charge, d for V2H, s to stop, q to quit (+CR)
        let mut input = [0u8; 2];

        let _ = std::io::stdin().lock();
        match std::io::stdin().read(&mut input) {
            Ok(_) => {
                println!("Input received: {:?}", input[0]);
            }
            Err(e) => eprintln!("Error reading input: {}", e),
        };
        match input[0] {
            115 => {
                // "s" stop
                log_error!("kb", pre_tx.send(PreCommand::Disable).await);
                log_error!("kb", mode_tx.send(OperationMode::Idle).await);
            }
            100 => {
                // "d" V2H
                log_error!("kb", mode_tx.send(OperationMode::V2h).await);
            }
            99 => {
                // "c" manual charge
                log_error!("kb", mode_tx.send(OperationMode::Charge).await);
            }
            113 => {
                // "q" quit
                log_error!("kb", pre_tx.send(PreCommand::Disable).await);
                log_error!("kb", mode_tx.send(OperationMode::Quit).await);
                println!("q key captured. Exiting...");
            }
            _ => continue,
        }
    }
}
