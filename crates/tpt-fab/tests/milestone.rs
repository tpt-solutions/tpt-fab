//! Milestone tests.
//!
//! Phase 1 milestone (spec Section 5): *a simulated equipment session completes a full recipe
//! cycle against `tpt-fab`'s own simulator, no real hardware involved.*
//!
//! Phase 2 milestone: *swapping the backend behind `tpt-fab`'s core requires no changes to
//! recipe/lot/APC logic* — the exact same host-side script runs unchanged against every
//! backend and produces the identical lifecycle outcome.

use std::time::Duration;

use tpt_fab::lot::LotState;
use tpt_fab::secs::{Item, SecsMessage};
use tpt_fab::sim::{ceids, svids, EquipmentSimulator, HostClient, SimulatorConfig};
use tpt_fab::PatterningBackend;
use tpt_fab_litho::duv::{DuvConfig, DuvMultiPatternBackend};
use tpt_fab_litho::ebeam::{EbeamBackend, EbeamConfig};
use tpt_fab_litho::euv::{EuvBackend, EuvConfig};
use tpt_fab_litho::nil::{NilBackend, NilConfig};

const T: Duration = Duration::from_secs(5);

/// Parses the simulator's S6F11 shape `L[DATAID, CEID, L[L["SubjectID", id], L["Note", n]]]`.
fn parse_event(msg: &SecsMessage) -> (u32, String) {
    let mut ceid = 0u32;
    let mut subject = String::new();
    if let Some(Item::L(parts)) = msg.item.as_ref() {
        if parts.len() >= 2 {
            ceid = parts[1].as_u4().unwrap_or(0);
        }
        if let Some(Item::L(reports)) = parts.get(2) {
            for rep in reports {
                if let Item::L(kv) = rep {
                    if kv.len() == 2 && kv[0].as_ascii() == Some("SubjectID") {
                        if let Some(s) = kv[1].as_ascii() {
                            subject = s.to_string();
                        }
                    }
                }
            }
        }
    }
    (ceid, subject)
}

fn backends() -> Vec<Box<dyn PatterningBackend>> {
    vec![
        Box::new(DuvMultiPatternBackend::new(DuvConfig::default()).unwrap()),
        Box::new(EuvBackend::new(EuvConfig::default()).unwrap()),
        Box::new(NilBackend::new(NilConfig::default()).unwrap()),
        Box::new(EbeamBackend::new(EbeamConfig::default()).unwrap()),
    ]
}

/// The full recipe cycle, driven identically regardless of the installed backend.
/// Returns the CEID sequence the host observed.
fn run_full_recipe_cycle(backend: Box<dyn PatterningBackend>) -> Vec<(u32, String)> {
    let sim = EquipmentSimulator::bind(SimulatorConfig::default(), backend).unwrap();
    sim.stage_lot_and_job("L1", &["W1", "W2", "W3"], "PJ1", "etch-std").unwrap();
    let addr = sim.spawn();

    let mut host = HostClient::connect(addr, 0, SimulatorConfig::default().timers).unwrap();
    assert_eq!(host.state, tpt_fab::hsms::HsmsState::Selected);

    // Establish communications and bring the tool online-remote.
    assert_eq!(host.establish_communications().unwrap(), 0);
    assert_eq!(host.go_online().unwrap(), 0);

    // Upload the recipe (S7F3), then read it back (S7F5) to prove the transfer round-trips.
    assert_eq!(host.upload_recipe("etch-std", b"recipe-body-v1").unwrap(), 0);
    let (ppid, body) = host.download_recipe("etch-std").unwrap();
    assert_eq!(ppid, "etch-std");
    assert_eq!(body, b"recipe-body-v1");

    // START the process job (S2F41 remote command).
    assert_eq!(host.remote_command("START", vec![("PJID", "PJ1")]).unwrap(), 0);

    // Collect event reports until the lot completes.
    let mut observed: Vec<(u32, String)> = Vec::new();
    loop {
        let ev = host
            .next_event(T)
            .unwrap_or_else(|e| panic!("no event within timeout ({e}); got {observed:?}"));
        let parsed = parse_event(&ev);
        observed.push(parsed.clone());
        if parsed.0 == ceids::LOT_COMPLETED {
            break;
        }
    }
    observed
}

#[test]
fn milestone_simulated_session_completes_full_recipe_cycle() {
    let observed =
        run_full_recipe_cycle(Box::new(DuvMultiPatternBackend::new(DuvConfig::default()).unwrap()));
    // The lifecycle: job created -> started -> per-substrate moves -> job completed -> lot done.
    let ceids_seen: Vec<u32> = observed.iter().map(|(c, _)| *c).collect();
    assert!(ceids_seen.contains(&6001), "job-created event missing: {ceids_seen:?}");
    assert!(ceids_seen.contains(&6002), "job-started event missing: {ceids_seen:?}");
    assert!(ceids_seen.contains(&6003), "job-completed event missing: {ceids_seen:?}");
    assert_eq!(*ceids_seen.last().unwrap(), ceids::LOT_COMPLETED);
    // Substrate moves: 3 wafers in and 3 out (6 SubstrateLocationChanged events).
    assert_eq!(ceids_seen.iter().filter(|c| **c == 5001).count(), 6);
    // Subject on the completion event is the lot.
    assert_eq!(observed.last().unwrap().1, "L1");
}

#[test]
fn milestone_backend_swap_changes_nothing_in_core() {
    // Run the identical host-side script against every backend. The recipe upload, remote
    // command, job lifecycle, and lot completion must be byte-for-byte the same — that is the
    // Phase 2 milestone: core recipe/lot/APC logic is unchanged by the backend swap.
    let mut reference: Option<Vec<(u32, String)>> = None;
    for backend in backends() {
        let observed = run_full_recipe_cycle(backend);
        match &reference {
            None => reference = Some(observed),
            Some(expected) => assert_eq!(&observed, expected, "backend swap changed core behavior"),
        }
    }
    assert!(reference.is_some());
}

#[test]
fn milestone_end_state_is_complete() {
    let sim = EquipmentSimulator::bind(
        SimulatorConfig::default(),
        Box::new(DuvMultiPatternBackend::new(DuvConfig::default()).unwrap()),
    )
    .unwrap();
    sim.stage_lot_and_job("L2", &["W4", "W5"], "PJ2", "etch-std").unwrap();
    // Collect a trace sample whenever a job completes (E116 data-collection plan).
    sim.add_collection_plan(tpt_fab::gem300::DataCollectionPlan {
        plan_id: 1,
        trigger_ceid: 6003,
        vids: vec!["PJID".into()],
    });
    let addr = sim.spawn();

    let mut host = HostClient::connect(addr, 0, SimulatorConfig::default().timers).unwrap();
    host.establish_communications().unwrap();
    host.go_online().unwrap();
    host.upload_recipe("etch-std", b"body").unwrap();
    host.remote_command("START", vec![("PJID", "PJ2")]).unwrap();
    loop {
        let ev = host.next_event(T).unwrap();
        if parse_event(&ev).0 == ceids::LOT_COMPLETED {
            break;
        }
    }

    let state = sim.core().state.lock().unwrap();
    // Lot completed with history.
    let lot = state.lots.get("L2").unwrap();
    assert_eq!(lot.state, LotState::Completed);
    // Substrates ended processed (Fixed).
    for w in ["W4", "W5"] {
        assert_eq!(
            state.substrates.get(w).unwrap().location,
            tpt_fab::gem300::SubstrateLocation::Fixed
        );
    }
    // SVID says one lot processed.
    assert_eq!(
        state.gem.svids.get(&svids::LOTS_PROCESSED),
        Some(&tpt_fab::secs::Item::U4(vec![1]))
    );
    // E116 collected a sample on the job-completed trigger.
    assert!(!state.e116.samples().is_empty());
    // Recipe landed with a checksummed version chain.
    let recipe = state.recipes.get("etch-std").unwrap();
    assert_eq!(recipe.version_numbers(), vec![1]);
}

#[test]
fn drift_out_of_spec_aborts_job_and_raises_alarm() {
    let sim = EquipmentSimulator::bind(
        SimulatorConfig::default(),
        Box::new(DuvMultiPatternBackend::new(DuvConfig::default()).unwrap()),
    )
    .unwrap();
    sim.stage_lot_and_job("L3", &["W6"], "PJ3", "etch-std").unwrap();
    let addr = sim.spawn();

    let mut host = HostClient::connect(addr, 0, SimulatorConfig::default().timers).unwrap();
    host.establish_communications().unwrap();
    host.go_online().unwrap();
    host.upload_recipe("etch-std", b"body").unwrap();

    // Induce drift: +25 °C offset is far outside the recipe's ±3 °C tolerance.
    let acks = host
        .set_equipment_constants(vec![(
            tpt_fab::sim::ecids::TEMP_OFFSET_C,
            tpt_fab::secs::Item::F8(vec![25.0]),
        )])
        .unwrap();
    assert_eq!(acks, vec![0]);

    host.remote_command("START", vec![("PJID", "PJ3")]).unwrap();

    // Expect the drift alarm and an abort; the lot goes on hold, never completes.
    let mut saw_alarm = false;
    let mut saw_abort = false;
    for _ in 0..8 {
        let ev = host.next_event(T).unwrap();
        match (ev.header.stream, ev.header.function) {
            (5, 1) => saw_alarm = true,
            (6, 11) => {
                let parts = ev.item.as_ref().unwrap().as_list().unwrap().to_vec();
                let ceid = parts[1].as_u4().unwrap();
                if ceid == 6004 {
                    saw_abort = true;
                    break;
                }
            }
            _ => {}
        }
    }
    assert!(saw_alarm, "drift alarm not raised");
    assert!(saw_abort, "job not aborted on drift");
    let state = sim.core().state.lock().unwrap();
    assert_eq!(state.lots.get("L3").unwrap().state, LotState::OnHold);
    assert_ne!(state.jobs.get("PJ3").unwrap().state, tpt_fab::gem300::ProcessJobState::Completed);
}
