//! Runs a complete equipment session against the built-in simulator — the same cycle the
//! milestone test asserts, but printed for humans.
//!
//! This is the end-to-end validation path that needs no real tool access: HSMS select,
//! GEM establish-communications, online, recipe upload over S7, a remote START, and the
//! resulting E40/E90/E116 event stream.
//!
//! Run with: `cargo run -p tpt-fab --example full_session`

use std::time::Duration;

use tpt_fab::secs::Item;
use tpt_fab::sim::{ceids, EquipmentSimulator, HostClient, SimulatorConfig};
use tpt_fab_litho::duv::{DuvConfig, DuvMultiPatternBackend};

fn main() -> Result<(), tpt_fab::FabError> {
    let sim = EquipmentSimulator::bind(
        SimulatorConfig::default(),
        Box::new(DuvMultiPatternBackend::new(DuvConfig::default()).unwrap()),
    )
    .unwrap();
    sim.stage_lot_and_job("LOT-1", &["W1", "W2", "W3"], "PJ-1", "etch-std").unwrap();
    let addr = sim.spawn();
    println!("simulator listening on {addr}");

    let mut host = HostClient::connect(addr, 0, SimulatorConfig::default().timers)?;
    println!("[hsms] selected");

    assert_eq!(host.establish_communications()?, 0);
    println!("[gem ] communications established (S1F13/S1F14)");

    assert_eq!(host.go_online()?, 0);
    println!("[gem ] equipment online-remote (S1F17/S1F18)");

    assert_eq!(host.upload_recipe("etch-std", b"recipe-body-v1")?, 0);
    let (ppid, body) = host.download_recipe("etch-std")?;
    println!("[s7  ] recipe '{ppid}' uploaded and read back ({} bytes)", body.len());

    assert_eq!(host.remote_command("START", vec![("PJID", "PJ-1")])?, 0);
    println!("[s2  ] START accepted (S2F41/S2F42 HCACK=0)");

    println!("[s6  ] event stream:");
    loop {
        let ev = host.next_event(Duration::from_secs(5))?;
        let mut ceid = 0u32;
        let mut subject = String::new();
        if let Some(Item::L(parts)) = ev.item.as_ref() {
            if parts.len() >= 2 {
                ceid = parts[1].as_u4().unwrap_or(0);
            }
            if let Some(Item::L(reports)) = parts.get(2) {
                for rep in reports {
                    if let Item::L(kv) = rep {
                        if kv.len() == 2 && kv[0].as_ascii() == Some("SubjectID") {
                            subject = kv[1].as_ascii().unwrap_or("").to_string();
                        }
                    }
                }
            }
        }
        let name = match ceid {
            5001 => "SubstrateLocationChanged",
            6001 => "ProcessJobCreated",
            6002 => "ProcessJobStarted",
            6003 => "ProcessJobCompleted",
            7002 => "LotStarted",
            7003 => "LotCompleted",
            _ => "?",
        };
        println!(
            "       S{}F{}  ceid {ceid:<5} ({name:<26}) subject {subject}",
            ev.header.stream, ev.header.function
        );
        if ceid == ceids::LOT_COMPLETED {
            break;
        }
    }

    let state = sim.core().state.lock().unwrap();
    println!(
        "[end ] lot state: {:?}, wafers processed: {}",
        state.lots.get("LOT-1").unwrap().state,
        state.substrates.substrates_in_lot("LOT-1").len()
    );
    host.separate()?;
    Ok(())
}
