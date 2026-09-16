//! GEM300 suite support: E39 (object services), E40 (process jobs), E90 (substrate
//! tracking), E116 (equipment performance tracking).
//!
//! The object model is deliberately simplified: E39's object-service semantics (create, get,
//! destroy, lifecycle events) are provided over in-memory registries shared by the other three
//! standards. What matters for validation is that job lifecycle transitions, substrate
//! locations, and data-collection triggers are all modeled and observable.

use crate::error::FabError;
use crate::gem::EventOccurrence;
use std::collections::BTreeMap;
use std::time::SystemTime;

/// Stable identifier for a tracked object (PJID, CarrierID, SubstrateID, ...).
pub type ObjectId = String;

/// Kinds of E39-managed objects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectKind {
    /// E90 substrate.
    Substrate,
    /// E90/E40 carrier.
    Carrier,
    /// E40 process job.
    ProcessJob,
    /// E40 control job.
    ControlJob,
}

/// A registered E39 object.
#[derive(Debug, Clone)]
pub struct TrackedObject {
    /// Object ID.
    pub id: ObjectId,
    /// Object kind.
    pub kind: ObjectKind,
    /// Creation timestamp.
    pub created: SystemTime,
}

/// E39 object registry: create/get/destroy semantics shared by E40 and E90.
#[derive(Debug, Default)]
pub struct ObjectRegistry {
    objects: BTreeMap<ObjectId, TrackedObject>,
}

impl ObjectRegistry {
    /// Registers a new object; errors if the ID already exists.
    pub fn create(&mut self, id: impl Into<String>, kind: ObjectKind) -> Result<(), FabError> {
        let id = id.into();
        if self.objects.contains_key(&id) {
            return Err(FabError::Gem300(format!("object {id} already exists")));
        }
        self.objects.insert(id.clone(), TrackedObject { id, kind, created: SystemTime::now() });
        Ok(())
    }

    /// Removes an object; errors if unknown.
    pub fn destroy(&mut self, id: &str) -> Result<(), FabError> {
        self.objects
            .remove(id)
            .map(|_| ())
            .ok_or_else(|| FabError::Gem300(format!("unknown object {id}")))
    }

    /// Fetches object metadata.
    pub fn get(&self, id: &str) -> Result<&TrackedObject, FabError> {
        self.objects.get(id).ok_or_else(|| FabError::Gem300(format!("unknown object {id}")))
    }

    /// All objects of a kind, sorted by ID.
    pub fn list(&self, kind: ObjectKind) -> Vec<&TrackedObject> {
        self.objects.values().filter(|o| o.kind == kind).collect()
    }

    /// Number of registered objects.
    pub fn len(&self) -> usize {
        self.objects.len()
    }

    /// Whether the registry is empty.
    pub fn is_empty(&self) -> bool {
        self.objects.is_empty()
    }
}

/// E90 substrate locations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubstrateLocation {
    /// In a load port carrier.
    Port { port_id: u32, carrier: Option<ObjectId> },
    /// In an intermediate buffer.
    Buffer { slot: u32 },
    /// In a process chamber.
    Chamber { chamber_id: String },
    /// Out of the tool (after unload).
    Fixed,
}

/// An E90-tracked substrate.
#[derive(Debug, Clone)]
pub struct Substrate {
    /// Substrate ID.
    pub id: ObjectId,
    /// Lot this substrate belongs to.
    pub lot: Option<ObjectId>,
    /// Current location.
    pub location: SubstrateLocation,
}

/// E90 substrate manager.
#[derive(Debug, Default)]
pub struct SubstrateManager {
    substrates: BTreeMap<ObjectId, Substrate>,
}

impl SubstrateManager {
    /// Registers a substrate.
    pub fn register(
        &mut self,
        registry: &mut ObjectRegistry,
        id: impl Into<String>,
        lot: Option<ObjectId>,
        location: SubstrateLocation,
    ) -> Result<(), FabError> {
        let id = id.into();
        registry.create(id.clone(), ObjectKind::Substrate)?;
        self.substrates.insert(id.clone(), Substrate { id, lot, location });
        Ok(())
    }

    /// Moves a substrate, returning the [`EventOccurrence`] (`SubstrateLocationChanged`)
    /// carrying VID-style values for the move.
    pub fn move_substrate(
        &mut self,
        id: &str,
        to: SubstrateLocation,
    ) -> Result<EventOccurrence, FabError> {
        let s = self
            .substrates
            .get_mut(id)
            .ok_or_else(|| FabError::Gem300(format!("unknown substrate {id}")))?;
        let from = std::mem::replace(&mut s.location, to.clone());
        Ok(EventOccurrence {
            ceid: 5001, // SubstrateLocationChanged (E90 example CEID)
            reports: vec![
                ("SubstrateID".into(), crate::secs::Item::A(id.to_string())),
                ("FromLocation".into(), crate::secs::Item::A(format_location(&from))),
                ("ToLocation".into(), crate::secs::Item::A(format_location(&to))),
            ],
        })
    }

    /// Fetches substrate state.
    pub fn get(&self, id: &str) -> Result<&Substrate, FabError> {
        self.substrates.get(id).ok_or_else(|| FabError::Gem300(format!("unknown substrate {id}")))
    }

    /// All substrates in a lot, sorted by ID.
    pub fn substrates_in_lot(&self, lot: &str) -> Vec<&Substrate> {
        self.substrates.values().filter(|s| s.lot.as_deref() == Some(lot)).collect()
    }
}

fn format_location(loc: &SubstrateLocation) -> String {
    match loc {
        SubstrateLocation::Port { port_id, .. } => format!("Port{port_id}"),
        SubstrateLocation::Buffer { slot } => format!("Buffer{slot}"),
        SubstrateLocation::Chamber { chamber_id } => format!("Chamber({chamber_id})"),
        SubstrateLocation::Fixed => "Fixed".into(),
    }
}

/// E40 process job states, following the PJ state model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessJobState {
    /// Job created, waiting for carriers/material.
    Queued,
    /// Material is being set up (carriers arriving).
    SettingUp,
    /// Waiting for a start command.
    WaitingForStart,
    /// Processing.
    Executing,
    /// Paused.
    ProcessPaused,
    /// Completed successfully.
    Completed,
    /// Aborted.
    Aborted,
}

impl ProcessJobState {
    /// Whether the job can transition to `Executing`.
    pub fn can_start(self) -> bool {
        matches!(self, ProcessJobState::WaitingForStart)
    }

    /// Whether the job is in a terminal state.
    pub fn is_terminal(self) -> bool {
        matches!(self, ProcessJobState::Completed | ProcessJobState::Aborted)
    }
}

/// CEIDs emitted on PJ lifecycle transitions (E40-style examples).
pub mod process_job_ceids {
    /// ProcessJobCreated.
    pub const CREATED: u32 = 6001;
    /// ProcessJobStarted.
    pub const STARTED: u32 = 6002;
    /// ProcessJobCompleted.
    pub const COMPLETED: u32 = 6003;
    /// ProcessJobAborted.
    pub const ABORTED: u32 = 6004;
    /// ProcessJobPaused.
    pub const PAUSED: u32 = 6005;
    /// ProcessJobResumed.
    pub const RESUMED: u32 = 6006;
}

/// An E40 process job.
#[derive(Debug, Clone)]
pub struct ProcessJob {
    /// PJID.
    pub id: ObjectId,
    /// Carriers feeding this job.
    pub carriers: Vec<ObjectId>,
    /// Recipe association (PPID).
    pub recipe: String,
    /// Current state.
    pub state: ProcessJobState,
    /// Substrates claimed by this job.
    pub substrates: Vec<ObjectId>,
}

/// E40 process job manager over the shared object registry.
#[derive(Debug, Default)]
pub struct ProcessJobManager {
    jobs: BTreeMap<ObjectId, ProcessJob>,
}

impl ProcessJobManager {
    /// Creates a job (state `Queued`) and registers it as an E39 object.
    pub fn create(
        &mut self,
        registry: &mut ObjectRegistry,
        id: impl Into<String>,
        carriers: Vec<ObjectId>,
        recipe: impl Into<String>,
        substrates: Vec<ObjectId>,
    ) -> Result<ObjectId, FabError> {
        let id = id.into();
        registry.create(id.clone(), ObjectKind::ProcessJob)?;
        self.jobs.insert(
            id.clone(),
            ProcessJob {
                id: id.clone(),
                carriers,
                recipe: recipe.into(),
                state: ProcessJobState::Queued,
                substrates,
            },
        );
        Ok(id)
    }

    /// State transition with E40-guard checks; returns the corresponding CEID.
    pub fn transition(
        &mut self,
        registry: &ObjectRegistry,
        id: &str,
        to: ProcessJobState,
    ) -> Result<u32, FabError> {
        registry.get(id)?;
        let job = self
            .jobs
            .get_mut(id)
            .ok_or_else(|| FabError::Gem300(format!("unknown process job {id}")))?;
        let from = job.state;
        let ceid = match (from, to) {
            (ProcessJobState::Queued, ProcessJobState::SettingUp) => process_job_ceids::CREATED,
            (ProcessJobState::SettingUp, ProcessJobState::WaitingForStart) => {
                process_job_ceids::CREATED
            }
            (ProcessJobState::WaitingForStart, ProcessJobState::Executing) if from.can_start() => {
                process_job_ceids::STARTED
            }
            (ProcessJobState::Executing, ProcessJobState::ProcessPaused) => {
                process_job_ceids::PAUSED
            }
            (ProcessJobState::ProcessPaused, ProcessJobState::Executing) => {
                process_job_ceids::RESUMED
            }
            (ProcessJobState::Executing, ProcessJobState::Completed) => {
                process_job_ceids::COMPLETED
            }
            (_, ProcessJobState::Aborted) if !from.is_terminal() => process_job_ceids::ABORTED,
            _ => {
                return Err(FabError::Gem300(format!(
                    "illegal PJ transition {from:?} -> {to:?} for {id}"
                )))
            }
        };
        job.state = to;
        Ok(ceid)
    }

    /// Fetches job state.
    pub fn get(&self, id: &str) -> Result<&ProcessJob, FabError> {
        self.jobs.get(id).ok_or_else(|| FabError::Gem300(format!("unknown process job {id}")))
    }

    /// Fetches job state mutably.
    pub fn get_mut(&mut self, id: &str) -> Result<&mut ProcessJob, FabError> {
        self.jobs.get_mut(id).ok_or_else(|| FabError::Gem300(format!("unknown process job {id}")))
    }
}

/// E116 data-collection plan.
#[derive(Debug, Clone)]
pub struct DataCollectionPlan {
    /// Plan ID.
    pub plan_id: u32,
    /// Triggering CEID.
    pub trigger_ceid: u32,
    /// VIDs collected on each trigger.
    pub vids: Vec<String>,
}

/// A collected trace sample.
#[derive(Debug, Clone)]
pub struct TraceSample {
    /// Plan that produced this sample.
    pub plan_id: u32,
    /// Triggering CEID.
    pub ceid: u32,
    /// Collected values.
    pub values: Vec<(String, String)>,
    /// Sample timestamp.
    pub at: SystemTime,
}

/// E116 equipment performance tracking: plans + collected samples.
#[derive(Debug, Default)]
pub struct E116Collector {
    plans: Vec<DataCollectionPlan>,
    samples: Vec<TraceSample>,
}

impl E116Collector {
    /// Registers a data-collection plan.
    pub fn add_plan(&mut self, plan: DataCollectionPlan) {
        self.plans.push(plan);
    }

    /// Called by the simulator on every event: collects samples for matching plans.
    pub fn on_event(&mut self, ceid: u32, values: &[(String, String)]) {
        for plan in &self.plans {
            if plan.trigger_ceid == ceid {
                self.samples.push(TraceSample {
                    plan_id: plan.plan_id,
                    ceid,
                    values: values.to_vec(),
                    at: SystemTime::now(),
                });
            }
        }
    }

    /// All collected samples.
    pub fn samples(&self) -> &[TraceSample] {
        &self.samples
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_create_get_destroy() {
        let mut reg = ObjectRegistry::default();
        reg.create("C1", ObjectKind::Carrier).unwrap();
        assert_eq!(reg.get("C1").unwrap().kind, ObjectKind::Carrier);
        assert!(reg.create("C1", ObjectKind::Carrier).is_err());
        reg.destroy("C1").unwrap();
        assert!(reg.destroy("C1").is_err());
    }

    #[test]
    fn substrate_move_emits_location_changed() {
        let mut reg = ObjectRegistry::default();
        let mut mgr = SubstrateManager::default();
        mgr.register(
            &mut reg,
            "W01",
            Some("L1".into()),
            SubstrateLocation::Port { port_id: 1, carrier: Some("C1".into()) },
        )
        .unwrap();
        let ev = mgr
            .move_substrate("W01", SubstrateLocation::Chamber { chamber_id: "CH1".into() })
            .unwrap();
        assert_eq!(ev.ceid, 5001);
        assert_eq!(
            mgr.get("W01").unwrap().location,
            SubstrateLocation::Chamber { chamber_id: "CH1".into() }
        );
        assert_eq!(mgr.substrates_in_lot("L1").len(), 1);
    }

    #[test]
    fn process_job_lifecycle_transitions() {
        let mut reg = ObjectRegistry::default();
        let mut mgr = ProcessJobManager::default();
        mgr.create(&mut reg, "PJ1", vec!["C1".into()], "etch-std", vec!["W01".into()]).unwrap();
        assert_eq!(mgr.get("PJ1").unwrap().state, ProcessJobState::Queued);
        mgr.transition(&reg, "PJ1", ProcessJobState::SettingUp).unwrap();
        mgr.transition(&reg, "PJ1", ProcessJobState::WaitingForStart).unwrap();
        let ceid = mgr.transition(&reg, "PJ1", ProcessJobState::Executing).unwrap();
        assert_eq!(ceid, process_job_ceids::STARTED);
        mgr.transition(&reg, "PJ1", ProcessJobState::Completed).unwrap();
        assert!(mgr.get("PJ1").unwrap().state.is_terminal());
        // Illegal transition from terminal state.
        assert!(mgr.transition(&reg, "PJ1", ProcessJobState::Executing).is_err());
    }

    #[test]
    fn e116_collects_matching_plans() {
        let mut c = E116Collector::default();
        c.add_plan(DataCollectionPlan {
            plan_id: 1,
            trigger_ceid: 6002,
            vids: vec!["PJID".into()],
        });
        c.on_event(6003, &[("PJID".into(), "PJ1".into())]);
        assert!(c.samples().is_empty());
        c.on_event(6002, &[("PJID".into(), "PJ1".into())]);
        assert_eq!(c.samples().len(), 1);
        assert_eq!(c.samples()[0].values[0].1, "PJ1");
    }
}
