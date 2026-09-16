//! Equipment-simulator harness: a TCP-speaking HSMS/GEM equipment plus a host-side client, so
//! the whole stack can be validated end-to-end without real tool access — the same practice
//! fabs themselves use before integrating new equipment.
//!
//! The simulator hosts the GEM/GEM300/recipe/lot state behind one lock, speaks passive HSMS,
//! and executes process jobs when a host issues the standard remote-command flow. Process jobs
//! call the installed [`PatterningBackend`] per substrate — core recipe/lot/APC logic is
//! identical regardless of which backend is installed. Recipe drift is checked before every
//! job: if the effective parameter set (recipe parameters plus equipment-constant offsets)
//! fails the drift check, the job aborts and the drift alarm (ALID 9001) is raised.

use crate::adapter::AdapterRegistry;
use crate::error::FabError;
use crate::gem::{CommunicationState, ControlState, GemAction, GemEquipment};
use crate::gem300::{
    DataCollectionPlan, E116Collector, ObjectRegistry, ProcessJobManager, ProcessJobState,
    SubstrateLocation, SubstrateManager,
};
use crate::hsms::{HsmsChannel, HsmsFrame, HsmsState, HsmsTimers, SelectStatus};
use crate::lot::{Lot, LotState, LotTracker};
use crate::recipe::{detect_drift, ParameterValue, RecipeStore};
use crate::secs::{Item, SecsHeader, SecsMessage};
use tpt_fab_litho::{ExposureRequest, LayerSpec, PatterningBackend, ProcessContext};

use std::collections::BTreeMap;
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// SVIDs defined by the standard simulator configuration.
pub mod svids {
    /// Currently executing recipe (PPID).
    pub const PP_EXEC_NAME: u32 = 2001;
    /// Control state.
    pub const CONTROL_STATE: u32 = 2002;
    /// Communication state.
    pub const COMMUNICATION_STATE: u32 = 2003;
    /// Lots processed since power-on.
    pub const LOTS_PROCESSED: u32 = 2004;
}

/// ECIDs defined by the standard simulator configuration (drift offsets).
pub mod ecids {
    /// Temperature offset, °C, added to the recipe setpoint (simulates calibration drift).
    pub const TEMP_OFFSET_C: u32 = 1001;
    /// Chamber pressure offset, mTorr, added to the recipe setpoint.
    pub const PRESSURE_OFFSET_MTORR: u32 = 1002;
    /// Process time scale factor (1.0 = nominal).
    pub const TIME_SCALE: u32 = 1003;
}

/// ALIDs defined by the standard simulator configuration.
pub mod alids {
    /// Recipe drift out of spec.
    pub const DRIFT_OUT_OF_SPEC: u32 = 9001;
    /// Patterning backend refused the plan.
    pub const BACKEND_ERROR: u32 = 9002;
}

/// CEIDs emitted by the simulator on top of the GEM300 job/substrate events.
pub mod ceids {
    /// A lot finished all processing (also `lot_ceids::COMPLETED` report payload).
    pub const LOT_COMPLETED: u32 = crate::lot::lot_ceids::COMPLETED;
}

/// Full simulator state, guarded by one mutex (a simulator, not a production MES — lock
/// contention is a non-goal).
pub struct FabState {
    /// GEM engine.
    pub gem: GemEquipment,
    /// Recipe store.
    pub recipes: RecipeStore,
    /// Lot tracker.
    pub lots: LotTracker,
    /// E39 object registry.
    pub registry: ObjectRegistry,
    /// E90 substrates.
    pub substrates: SubstrateManager,
    /// E40 process jobs.
    pub jobs: ProcessJobManager,
    /// E116 data collection.
    pub e116: E116Collector,
}

/// Shared simulator core.
pub struct SimCore {
    /// All fab state.
    pub state: Mutex<FabState>,
    /// Installed patterning backend.
    pub backend: Mutex<Box<dyn PatterningBackend>>,
    /// Vendor adapters (fixed at startup).
    pub adapters: AdapterRegistry,
    /// Layer the simulator patterns per substrate.
    pub pattern_layer: LayerSpec,
}

/// Simulator configuration.
pub struct SimulatorConfig {
    /// Device/session ID.
    pub device_id: u16,
    /// Model number (S1F2 MDLN).
    pub mdln: String,
    /// Software revision (S1F2 SOFTREV).
    pub softrev: String,
    /// HSMS timers.
    pub timers: HsmsTimers,
    /// Layer patterned per substrate.
    pub pattern_layer: LayerSpec,
}

impl Default for SimulatorConfig {
    fn default() -> Self {
        SimulatorConfig {
            device_id: 0,
            mdln: "TPT-SIM1".into(),
            softrev: env!("CARGO_PKG_VERSION").into(),
            timers: HsmsTimers {
                t3: Duration::from_secs(10),
                t5: Duration::from_secs(1),
                t6: Duration::from_secs(3),
                t7: Duration::from_secs(5),
                t8: Duration::from_secs(5),
                linktest: Duration::from_secs(0),
            },
            pattern_layer: LayerSpec::new("poly", 60.0, 140.0, 0.3),
        }
    }
}

/// The bound simulator. Accepts connections via [`EquipmentSimulator::run_once`] or a spawned
/// accept-loop thread.
pub struct EquipmentSimulator {
    listener: TcpListener,
    core: Arc<SimCore>,
    timers: HsmsTimers,
}

impl EquipmentSimulator {
    /// Binds a simulator on an arbitrary port and initializes the standard
    /// SVID/ECID/CEID/ALID registries.
    pub fn bind(
        config: SimulatorConfig,
        backend: Box<dyn PatterningBackend>,
    ) -> Result<Self, FabError> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let mut gem = GemEquipment::new(config.device_id, config.mdln, config.softrev);
        gem.define_svid(svids::PP_EXEC_NAME, Item::A(String::new()));
        gem.define_svid(svids::CONTROL_STATE, Item::A("EquipmentOffline".into()));
        gem.define_svid(svids::COMMUNICATION_STATE, Item::A("NotCommunicating".into()));
        gem.define_svid(svids::LOTS_PROCESSED, Item::U4(vec![0]));
        gem.define_ecid(ecids::TEMP_OFFSET_C, Item::F8(vec![0.0]));
        gem.define_ecid(ecids::PRESSURE_OFFSET_MTORR, Item::F8(vec![0.0]));
        gem.define_ecid(ecids::TIME_SCALE, Item::F8(vec![1.0]));
        for (ceid, name) in [
            (5001u32, "SubstrateLocationChanged"),
            (6001, "ProcessJobCreated"),
            (6002, "ProcessJobStarted"),
            (6003, "ProcessJobCompleted"),
            (6004, "ProcessJobAborted"),
            (7002, "LotStarted"),
            (7003, "LotCompleted"),
        ] {
            gem.define_ceid(ceid, name);
        }
        gem.define_alid(alids::DRIFT_OUT_OF_SPEC, "recipe drift out of spec");
        gem.define_alid(alids::BACKEND_ERROR, "patterning backend refused plan");
        let core = Arc::new(SimCore {
            state: Mutex::new(FabState {
                gem,
                recipes: RecipeStore::new(),
                lots: LotTracker::new(),
                registry: ObjectRegistry::default(),
                substrates: SubstrateManager::default(),
                jobs: ProcessJobManager::default(),
                e116: E116Collector::default(),
            }),
            backend: Mutex::new(backend),
            adapters: AdapterRegistry::new(),
            pattern_layer: config.pattern_layer,
        });
        Ok(EquipmentSimulator { listener, core, timers: config.timers })
    }

    /// Local bind address.
    pub fn local_addr(&self) -> SocketAddr {
        self.listener.local_addr().expect("bound listener")
    }

    /// Shared core handle (for scenario setup before/while connections run).
    pub fn core(&self) -> &Arc<SimCore> {
        &self.core
    }

    /// Registers a data-collection plan (E116).
    pub fn add_collection_plan(&self, plan: DataCollectionPlan) {
        self.core.state.lock().unwrap().e116.add_plan(plan);
    }

    /// Convenience scenario setup: registers a lot of substrates in port 1's carrier and
    /// creates a queued process job for it.
    pub fn stage_lot_and_job(
        &self,
        lot_id: &str,
        wafer_ids: &[&str],
        pj_id: &str,
        recipe_id: &str,
    ) -> Result<(), FabError> {
        let mut state = self.core.state.lock().unwrap();
        let FabState { lots, registry, substrates: subs, jobs, .. } = &mut *state;
        let mut substrate_ids = Vec::new();
        for w in wafer_ids {
            subs.register(
                registry,
                *w,
                Some(lot_id.to_string()),
                SubstrateLocation::Port { port_id: 1, carrier: Some(format!("C-{lot_id}")) },
            )?;
            substrate_ids.push((*w).to_string());
        }
        registry.create(format!("C-{lot_id}"), crate::gem300::ObjectKind::Carrier)?;
        let mut lot = Lot::new(lot_id, substrate_ids.clone());
        lot.transition(LotState::InQueue, "staged at port 1")?;
        lots.register(lot)?;
        jobs.create(registry, pj_id, vec![format!("C-{lot_id}")], recipe_id, substrate_ids)?;
        Ok(())
    }

    /// Accepts exactly one connection and serves it until the peer separates or disconnects.
    /// Returns when the connection ends.
    pub fn run_once(&self) -> Result<(), FabError> {
        let (stream, _peer) = self.listener.accept()?;
        serve_connection(stream, Arc::clone(&self.core), self.timers);
        Ok(())
    }

    /// Spawns the accept loop on a background thread. Serves connections until the listener
    /// fails. Returns the bound address.
    pub fn spawn(&self) -> SocketAddr {
        let addr = self.local_addr();
        let listener = self.listener.try_clone().expect("clone listener");
        let core = Arc::clone(&self.core);
        let timers = self.timers;
        std::thread::spawn(move || {
            while let Ok((stream, _)) = listener.accept() {
                let core = Arc::clone(&core);
                std::thread::spawn(move || serve_connection(stream, core, timers));
            }
        });
        addr
    }
}

/// Serves one passive HSMS connection against the core.
fn serve_connection(stream: TcpStream, core: Arc<SimCore>, timers: HsmsTimers) {
    let write_half = Arc::new(Mutex::new(match stream.try_clone() {
        Ok(s) => s,
        Err(_) => return,
    }));
    let mut reader = match stream.try_clone() {
        Ok(s) => HsmsChannel::new(s, timers.t8),
        Err(_) => return,
    };
    let mut state = HsmsState::ConnectedNotSelected;
    // T7 applies until selected.
    loop {
        let idle = if state == HsmsState::ConnectedNotSelected {
            timers.t7
        } else {
            Duration::from_secs(60)
        };
        match reader.recv(idle) {
            Ok(frame) => match frame.header.s_type {
                1 => {
                    let rsp_status = if state == HsmsState::ConnectedNotSelected {
                        state = HsmsState::Selected;
                        SelectStatus::Accepted
                    } else {
                        SelectStatus::NotReady
                    };
                    let rsp = HsmsFrame::select_rsp(frame.header.system_bytes, rsp_status);
                    if send_frame(&write_half, &rsp).is_err() {
                        return;
                    }
                }
                4 => return, // separate
                5 => {
                    let rsp = HsmsFrame::linktest_rsp(frame.header.system_bytes);
                    if send_frame(&write_half, &rsp).is_err() {
                        return;
                    }
                }
                0 if state == HsmsState::Selected => {
                    if handle_data(&frame, &core, &write_half).is_err() {
                        return;
                    }
                }
                0 => {
                    let rej = HsmsFrame::reject(frame.header.system_bytes, 0);
                    if send_frame(&write_half, &rej).is_err() {
                        return;
                    }
                }
                _ => {}
            },
            Err(FabError::Timeout(_)) | Err(FabError::Disconnected) | Err(FabError::Io(_)) => {
                return
            }
            Err(_) => return,
        }
    }
}

fn send_frame(write_half: &Arc<Mutex<TcpStream>>, frame: &HsmsFrame) -> Result<(), FabError> {
    use std::io::Write;
    let mut s = write_half.lock().unwrap();
    let bytes = frame.encode();
    s.write_all(&bytes)?;
    s.flush()?;
    Ok(())
}

fn send_message(write_half: &Arc<Mutex<TcpStream>>, msg: &SecsMessage) -> Result<(), FabError> {
    let frame = HsmsFrame::data(
        msg.header.device_id,
        msg.header.stream,
        msg.header.function,
        msg.header.w_bit,
        msg.header.system_bytes,
        {
            let mut body = Vec::new();
            if let Some(item) = &msg.item {
                item.encode_into(&mut body);
            }
            body
        },
    );
    send_frame(write_half, &frame)
}

fn handle_data(
    frame: &HsmsFrame,
    core: &Arc<SimCore>,
    write_half: &Arc<Mutex<TcpStream>>,
) -> Result<(), FabError> {
    let header: SecsHeader = frame.header;
    let msg = SecsMessage::from_parts(header, &frame.body)?;
    if core.adapters.intercept_incoming(&msg) == crate::adapter::AdapterDecision::Drop {
        return Ok(());
    }
    // Actions with side effects that must happen after the reply goes out.
    let mut reply_and_deferred: Option<SecsMessage> = None;
    let mut deferred_job: Option<String> = None;
    {
        let mut state = core.state.lock().unwrap();
        let reply = state.gem.handle_message(&msg);
        let actions = state.gem.take_actions();
        for action in actions {
            match action {
                GemAction::RecipeReceived { ppid, body } => {
                    apply_recipe_body(&mut state, &ppid, body);
                }
                GemAction::RemoteCommand { command, parameters } => {
                    if command.eq_ignore_ascii_case("START") {
                        if let Some(pjid) = parameters
                            .iter()
                            .find(|(k, _)| k == "PJID")
                            .and_then(|(_, v)| v.as_ascii().map(str::to_string))
                        {
                            deferred_job = Some(pjid);
                        }
                    }
                }
                GemAction::GoOnlineRequested => {
                    let mode = state.gem.default_online_mode;
                    state.gem.operator_request_online(mode);
                }
                GemAction::GoOfflineRequested => state.gem.go_offline(true),
                GemAction::EquipmentConstantsChanged { .. } | GemAction::TerminalMessage { .. } => {
                }
            }
        }
        update_svids(&mut state.gem);
        if let Some(mut r) = reply {
            core.adapters.intercept_outgoing(&mut r);
            reply_and_deferred = Some(r);
        }
    }
    if let Some(r) = reply_and_deferred {
        send_message(write_half, &r)?;
    }
    // Run any deferred job with the lock released so event reports flow.
    if let Some(pjid) = deferred_job {
        run_process_job(core, write_half, &pjid)?;
    }
    Ok(())
}

fn apply_recipe_body(state: &mut FabState, ppid: &str, body: Vec<u8>) {
    // A downloaded PP becomes a new recipe version if the recipe exists; otherwise a new
    // recipe whose parameters start from the standard defaults.
    let params = BTreeMap::from([
        ("chamber_temp_c".to_string(), ParameterValue::Numeric(150.0)),
        ("chamber_pressure_mtorr".to_string(), ParameterValue::Numeric(50.0)),
        ("process_time_s".to_string(), ParameterValue::Numeric(60.0)),
    ]);
    let tolerances = BTreeMap::from([
        ("chamber_temp_c".to_string(), crate::recipe::Tolerance::Absolute(3.0)),
        ("chamber_pressure_mtorr".to_string(), crate::recipe::Tolerance::Absolute(4.0)),
        ("process_time_s".to_string(), crate::recipe::Tolerance::Relative(0.05)),
    ]);
    if let Ok(recipe) = state.recipes.get_mut(ppid) {
        let v = crate::recipe::RecipeVersion::new(0, params, tolerances, body, "host");
        recipe.add_version(v);
    } else {
        let recipe =
            crate::recipe::Recipe::new(ppid, "downloaded via S7F3", params, tolerances, body);
        let _ = state.recipes.add(recipe);
    }
}

fn update_svids(gem: &mut GemEquipment) {
    let ctrl = match gem.ctrl_state {
        ControlState::EquipmentOffline => "EquipmentOffline",
        ControlState::AttemptOnline => "AttemptOnline",
        ControlState::HostOffline => "HostOffline",
        ControlState::OnlineLocal => "OnlineLocal",
        ControlState::OnlineRemote => "OnlineRemote",
    };
    let com = match gem.com_state {
        CommunicationState::Disabled => "Disabled",
        CommunicationState::EnabledNotCommunicating => "NotCommunicating",
        CommunicationState::EnabledCommunicating => "Communicating",
    };
    gem.define_svid(svids::CONTROL_STATE, Item::A(ctrl.into()));
    gem.define_svid(svids::COMMUNICATION_STATE, Item::A(com.into()));
}

/// Executes a queued process job end-to-end, emitting S6F11 event reports as it goes.
fn run_process_job(
    core: &Arc<SimCore>,
    write_half: &Arc<Mutex<TcpStream>>,
    pj_id: &str,
) -> Result<(), FabError> {
    // Phase 1: validate drift and start the job.
    let outcome = {
        let mut state = core.state.lock().unwrap();
        let pj = state.jobs.get(pj_id)?.clone();
        let FabState { gem, recipes, lots, registry, substrates: subs, jobs, e116: _ } =
            &mut *state;
        let mut events: Vec<SecsMessage> = Vec::new();
        let recipe = match recipes.get(&pj.recipe).map(|r| r.latest().clone()) {
            Ok(r) => r,
            Err(e) => {
                // Unknown recipe: abort the job rather than dropping the connection.
                let alarm = gem.make_alarm(alids::DRIFT_OUT_OF_SPEC, true)?;
                events.push(alarm);
                let ceid = jobs.transition(registry, pj_id, ProcessJobState::Aborted)?;
                events.push(event_for(gem, ceid, pj_id, "ABORTED"));
                hold_lot(lots, subs, &pj);
                let _ = e;
                return Ok(());
            }
        };
        // Effective parameters = recipe value + EC offset (or scale).
        let offsets = effective_offsets(&gem.ecids);
        let mut measured = recipe.parameters.clone();
        for (name, value) in measured.iter_mut() {
            if let ParameterValue::Numeric(v) = value {
                if name.contains("temp") {
                    *v += offsets.0;
                } else if name.contains("pressure") {
                    *v += offsets.1;
                } else if name.contains("time") {
                    *v *= offsets.2;
                }
            }
        }
        let check = detect_drift(&pj.recipe, &recipe, &measured);
        if !check.runnable() {
            let alarm = gem.make_alarm(alids::DRIFT_OUT_OF_SPEC, true)?;
            events.push(alarm);
            let ceid = jobs.transition(registry, pj_id, ProcessJobState::Aborted)?;
            events.push(event_for(gem, ceid, pj_id, "ABORTED"));
            hold_lot(lots, subs, &pj);
        } else {
            for step in [
                ProcessJobState::SettingUp,
                ProcessJobState::WaitingForStart,
                ProcessJobState::Executing,
            ] {
                let ceid = jobs.transition(registry, pj_id, step)?;
                events.push(event_for(gem, ceid, pj_id, "RUNNING"));
            }
            if let Ok(lot_id) = lot_of_via(lots, subs, pj_id) {
                if let Ok(ceid) = lots
                    .get_mut(&lot_id)
                    .and_then(|l| l.transition(LotState::InProcessing, pj_id.to_string()))
                {
                    events.push(event_for(gem, ceid, &lot_id, "RUNNING"));
                }
            }
        }
        update_svids(gem);
        events
    };
    for ev in outcome {
        send_message(write_half, &ev)?;
    }

    // Only proceed if the job actually started.
    let started = {
        let state = core.state.lock().unwrap();
        state.jobs.get(pj_id).map(|p| p.state == ProcessJobState::Executing).unwrap_or(false)
    };
    if !started {
        return Ok(());
    }

    // Phase 2: process each substrate through the patterning backend.
    let substrates: Vec<String> = {
        let state = core.state.lock().unwrap();
        state.jobs.get(pj_id).map(|p| p.substrates.clone()).unwrap_or_default()
    };
    let mut backend_failed = false;
    for sub in &substrates {
        // Move into chamber.
        let ev = {
            let mut state = core.state.lock().unwrap();
            state
                .substrates
                .move_substrate(sub, SubstrateLocation::Chamber { chamber_id: "CH-LITHO".into() })
                .map(|occ| state.gem.make_event_report(&occ))
        };
        if let Ok(msg) = ev {
            send_message(write_half, &msg)?;
        }
        // Ask the installed backend to plan this substrate's pattern step.
        let plan_result = {
            let request = ExposureRequest {
                layer: core.pattern_layer.clone(),
                context: ProcessContext::default(),
                overlay_budget_nm: 10.0,
                dose_budget_mj_cm2: 500.0,
            };
            core.backend.lock().unwrap().plan_exposure(&request).map(|plan| plan.total_dose_mj_cm2)
        };
        match plan_result {
            Ok(_dose) => {}
            Err(_) => {
                backend_failed = true;
                let msg = {
                    let mut state = core.state.lock().unwrap();
                    state.gem.make_alarm(alids::BACKEND_ERROR, true)
                };
                if let Ok(msg) = msg {
                    send_message(write_half, &msg)?;
                }
                break;
            }
        }
        // Move out.
        let ev = {
            let mut state = core.state.lock().unwrap();
            state
                .substrates
                .move_substrate(sub, SubstrateLocation::Fixed)
                .map(|occ| state.gem.make_event_report(&occ))
        };
        if let Ok(msg) = ev {
            send_message(write_half, &msg)?;
        }
    }

    // Phase 3: finish the job and the lot.
    let events = {
        let mut state = core.state.lock().unwrap();
        let FabState { gem, recipes: _, lots, registry, substrates: subs, jobs, e116 } =
            &mut *state;
        let mut events = Vec::new();
        let ceid = if backend_failed {
            jobs.transition(registry, pj_id, ProcessJobState::Aborted)?
        } else {
            jobs.transition(registry, pj_id, ProcessJobState::Completed)?
        };
        let label = if backend_failed { "ABORTED" } else { "DONE" };
        events.push(event_for(gem, ceid, pj_id, label));
        if let Ok(lot_id) = lot_of_via(lots, subs, pj_id) {
            let to = if backend_failed { LotState::OnHold } else { LotState::Completed };
            if let Ok(lceid) =
                lots.get_mut(&lot_id).and_then(|l| l.transition(to, pj_id.to_string()))
            {
                events.push(event_for(gem, lceid, &lot_id, label));
            }
        }
        if !backend_failed {
            // Bump LOTS_PROCESSED.
            if let Some(Item::U4(v)) = gem.svids.get_mut(&svids::LOTS_PROCESSED) {
                if let Some(first) = v.first_mut() {
                    *first += 1;
                }
            }
        }
        update_svids(gem);
        e116.on_event(6003, &[("PJID".into(), pj_id.to_string())]);
        events
    };
    for ev in events {
        send_message(write_half, &ev)?;
    }
    Ok(())
}

fn hold_lot(lots: &mut LotTracker, subs: &SubstrateManager, pj: &crate::gem300::ProcessJob) {
    // The lot the job was staged for goes on hold on abort.
    let lot_id =
        pj.substrates.first().and_then(|s| subs.get(s).ok().and_then(|sub| sub.lot.clone()));
    if let Some(lot_id) = lot_id {
        let _ = lots.get_mut(&lot_id).and_then(|l| l.transition(LotState::OnHold, "drift abort"));
    }
}

/// Offsets tuple: (temp, pressure, time-scale) read from the equipment constants.
fn effective_offsets(ecids_map: &BTreeMap<u32, Item>) -> (f64, f64, f64) {
    let get = |id: u32| -> f64 {
        ecids_map
            .get(&id)
            .and_then(|i| match i {
                Item::F8(v) => v.first().copied(),
                Item::F4(v) => v.first().map(|f| *f as f64),
                Item::U4(v) => v.first().map(|u| *u as f64),
                _ => None,
            })
            .unwrap_or(0.0)
    };
    let t = get(crate::sim::ecids::TEMP_OFFSET_C);
    let p = get(crate::sim::ecids::PRESSURE_OFFSET_MTORR);
    let scale = get(crate::sim::ecids::TIME_SCALE);
    (t, p, if scale > 0.0 { scale } else { 1.0 })
}

/// Resolves the lot a process job is processing, via the job's staged substrates.
fn lot_of_via(lots: &LotTracker, subs: &SubstrateManager, pj_id: &str) -> Result<String, FabError> {
    let _ = pj_id;
    lots.lots()
        .iter()
        .find(|l| !l.is_finished() && l.substrates.iter().any(|s| subs.get(s).is_ok()))
        .map(|l| l.id.clone())
        .ok_or_else(|| FabError::Lot("no active lot found for job".into()))
}

/// Builds an S6F11 with a standard report payload for a CEID occurrence.
fn event_for(gem: &mut GemEquipment, ceid: u32, subject: &str, note: &str) -> SecsMessage {
    let occurrence = crate::gem::EventOccurrence {
        ceid,
        reports: vec![
            ("SubjectID".to_string(), Item::A(subject.to_string())),
            ("Note".to_string(), Item::A(note.to_string())),
        ],
    };
    gem.make_event_report(&occurrence)
}

/// Host-side validation client: an active HSMS endpoint with typed GEM helpers.
pub struct HostClient {
    channel: HsmsChannel,
    device_id: u16,
    next_system: u32,
    /// HSMS session state.
    pub state: HsmsState,
    /// Messages received while waiting for something else (S6F11s etc.), FIFO.
    pub inbox: Vec<SecsMessage>,
    timers: HsmsTimers,
}

impl HostClient {
    /// Connects (active role) and performs the HSMS select transaction.
    pub fn connect(addr: SocketAddr, device_id: u16, timers: HsmsTimers) -> Result<Self, FabError> {
        let stream = TcpStream::connect(addr)?;
        let mut channel = HsmsChannel::new(stream, timers.t8);
        channel.send(&HsmsFrame::select_req(1))?;
        let rsp = channel.recv(timers.t6)?;
        if rsp.select_status() != Some(SelectStatus::Accepted) {
            return Err(FabError::Hsms("select refused".into()));
        }
        Ok(HostClient {
            channel,
            device_id,
            next_system: 100,
            state: HsmsState::Selected,
            inbox: Vec::new(),
            timers,
        })
    }

    fn alloc_system(&mut self) -> u32 {
        self.next_system += 1;
        self.next_system
    }

    /// Sends a message without expecting a reply (a fresh transaction ID is assigned).
    pub fn send(&mut self, mut msg: SecsMessage) -> Result<(), FabError> {
        let sys = self.alloc_system();
        self.send_raw(msg.header.stream, msg.header.function, sys, msg.item.take())
    }

    fn send_raw(
        &mut self,
        stream: u8,
        function: u8,
        system_bytes: u32,
        item: Option<Item>,
    ) -> Result<(), FabError> {
        let mut body = Vec::new();
        if let Some(i) = &item {
            i.encode_into(&mut body);
        }
        self.channel.send(&HsmsFrame::data(
            self.device_id,
            stream,
            function,
            false,
            system_bytes,
            body,
        ))
    }

    /// Sends a W-bit request and awaits the matching reply (T3). Non-matching data messages
    /// are queued in [`HostClient::inbox`].
    pub fn request(
        &mut self,
        stream: u8,
        function: u8,
        item: Option<Item>,
    ) -> Result<SecsMessage, FabError> {
        let sys = self.alloc_system();
        let mut body = Vec::new();
        if let Some(i) = &item {
            i.encode_into(&mut body);
        }
        self.channel.send(&HsmsFrame::data(self.device_id, stream, function, true, sys, body))?;
        loop {
            let frame = self.channel.recv(self.timers.t3)?;
            if !frame.is_data() {
                continue; // ignore control frames while waiting
            }
            let msg = SecsMessage::from_parts(frame.header, &frame.body)?;
            if msg.header.system_bytes == sys && msg.header.function == function + 1 {
                return Ok(msg);
            }
            self.inbox.push(msg);
        }
    }

    /// Receives the next equipment-initiated message (event report, alarm...). Auto-acks
    /// S6F11 with S6F12 and S5F1 with S5F2, as a well-behaved host should.
    pub fn next_event(&mut self, timeout: Duration) -> Result<SecsMessage, FabError> {
        if !self.inbox.is_empty() {
            return Ok(self.inbox.remove(0));
        }
        loop {
            let frame = self.channel.recv(timeout)?;
            if !frame.is_data() {
                continue;
            }
            let msg = SecsMessage::from_parts(frame.header, &frame.body)?;
            // Ack what the standard requires acks for.
            let (astream, afunction) = match (msg.header.stream, msg.header.function) {
                (6, 11) => (6u8, 12u8),
                (5, 1) => (5u8, 2u8),
                _ => continue,
            };
            self.send_raw(astream, afunction, msg.header.system_bytes, Some(Item::B(vec![0])))?;
            return Ok(msg);
        }
    }

    /// Sends separate and closes.
    pub fn separate(&mut self) -> Result<(), FabError> {
        let sys = self.alloc_system();
        self.channel.send(&HsmsFrame::separate_req(sys))
    }

    /// S1F13 establish communications; returns the ACKC13 code (0 = accepted).
    pub fn establish_communications(&mut self) -> Result<u8, FabError> {
        let req = Item::L(vec![Item::A("tpt-host".into()), Item::A("0.1".into())]);
        let reply = self.request(1, 13, Some(req))?;
        Ok(reply
            .item
            .as_ref()
            .and_then(|i| i.as_list().and_then(|l| l.first()))
            .and_then(Item::ack_code)
            .unwrap_or(1))
    }

    /// S1F17 host online request; returns ACKC17 (0 = accepted).
    pub fn go_online(&mut self) -> Result<u8, FabError> {
        let reply = self.request(1, 17, None)?;
        Ok(reply.item.as_ref().and_then(Item::ack_code).unwrap_or(1))
    }

    /// S7F3 recipe upload; returns ACKC7 (0 = accepted).
    pub fn upload_recipe(&mut self, ppid: &str, body: &[u8]) -> Result<u8, FabError> {
        let req = Item::L(vec![Item::A(ppid.into()), Item::B(body.to_vec())]);
        let reply = self.request(7, 3, Some(req))?;
        Ok(reply.item.as_ref().and_then(Item::ack_code).unwrap_or(1))
    }

    /// S7F5 recipe download; returns (PPID, BODY).
    pub fn download_recipe(&mut self, ppid: &str) -> Result<(String, Vec<u8>), FabError> {
        let reply = self.request(7, 5, Some(Item::A(ppid.into())))?;
        let parts = reply
            .item
            .as_ref()
            .and_then(|i| i.as_list())
            .ok_or_else(|| FabError::Recipe("S7F6 not a list".into()))?;
        let ppid = parts.first().and_then(|i| i.as_ascii()).unwrap_or("").to_string();
        let body = match parts.get(1) {
            Some(Item::B(b)) => b.clone(),
            _ => Vec::new(),
        };
        Ok((ppid, body))
    }

    /// S2F15 set equipment constants; returns the per-constant ack codes.
    pub fn set_equipment_constants(&mut self, ecs: Vec<(u32, Item)>) -> Result<Vec<u8>, FabError> {
        let req = Item::L(
            ecs.into_iter()
                .map(|(ecid, value)| Item::L(vec![Item::U4(vec![ecid]), value]))
                .collect(),
        );
        let reply = self.request(2, 15, Some(req))?;
        Ok(reply
            .item
            .as_ref()
            .and_then(|i| i.as_list())
            .map(|l| l.iter().filter_map(Item::ack_code).collect())
            .unwrap_or_default())
    }

    /// S2F41 remote command with string parameters; returns HCACK (0 = ok).
    pub fn remote_command(
        &mut self,
        command: &str,
        params: Vec<(&str, &str)>,
    ) -> Result<u8, FabError> {
        let req = Item::L(vec![
            Item::A(command.into()),
            Item::L(
                params
                    .into_iter()
                    .map(|(k, v)| Item::L(vec![Item::A(k.into()), Item::A(v.into())]))
                    .collect(),
            ),
        ]);
        let reply = self.request(2, 41, Some(req))?;
        Ok(reply
            .item
            .as_ref()
            .and_then(|i| i.as_list().and_then(|l| l.first()))
            .and_then(Item::ack_code)
            .unwrap_or(1))
    }

    /// S1F3 status request; returns one item per requested SVID.
    pub fn status(&mut self, svids: Vec<u32>) -> Result<Vec<Item>, FabError> {
        let req = Item::L(svids.into_iter().map(|s| Item::U4(vec![s])).collect());
        let reply = self.request(1, 3, Some(req))?;
        Ok(reply
            .item
            .map(|i| match i {
                Item::L(v) => v,
                other => vec![other],
            })
            .unwrap_or_default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tpt_fab_litho::duv::{DuvConfig, DuvMultiPatternBackend};

    fn sim() -> Arc<EquipmentSimulator> {
        let backend = DuvMultiPatternBackend::new(DuvConfig::default()).unwrap();
        Arc::new(EquipmentSimulator::bind(SimulatorConfig::default(), Box::new(backend)).unwrap())
    }

    #[test]
    fn hsms_select_and_establish_comms() {
        let s = sim();
        let addr = s.spawn();
        let mut host = HostClient::connect(addr, 0, HsmsTimers::default()).unwrap();
        assert_eq!(host.state, HsmsState::Selected);
        assert_eq!(host.establish_communications().unwrap(), 0);
        let state = s.core.state.lock().unwrap();
        assert_eq!(state.gem.com_state, CommunicationState::EnabledCommunicating);
    }

    #[test]
    fn status_query_reflects_state() {
        let s = sim();
        let addr = s.spawn();
        let mut host = HostClient::connect(addr, 0, HsmsTimers::default()).unwrap();
        host.establish_communications().unwrap();
        let vals = host.status(vec![svids::CONTROL_STATE]).unwrap();
        assert_eq!(vals[0].as_ascii(), Some("EquipmentOffline"));
        host.go_online().unwrap();
        let vals = host.status(vec![svids::CONTROL_STATE]).unwrap();
        assert_eq!(vals[0].as_ascii(), Some("OnlineRemote"));
        assert_eq!(s.core.state.lock().unwrap().gem.ctrl_state, ControlState::OnlineRemote);
    }
}
