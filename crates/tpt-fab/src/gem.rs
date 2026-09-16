//! GEM state models and message behavior (SEMI E30).
//!
//! Implements the communication state model, the control state model, variable registries
//! (SVID/ECID/CEID/ALID), alarms, terminal services, recipe transfer messages, and remote
//! commands — enough of E30 for the equipment simulator to run full recipe cycles against a
//! host. Side effects the caller must perform (starting process jobs, persisting recipes,
//! going physically online) are surfaced as [`GemAction`]s rather than done inline, keeping
//! this layer testable without hardware.

use crate::error::FabError;
use crate::secs::{Item, SecsMessage};
use std::collections::BTreeMap;

/// GEM communication state model (E30).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommunicationState {
    /// Communications disabled locally.
    Disabled,
    /// Enabled, but no successful S1 handshake yet.
    EnabledNotCommunicating,
    /// S1F13/S1F14 handshake completed.
    EnabledCommunicating,
}

/// GEM control state model (E30), flattened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlState {
    /// Equipment offline, operator has not requested online.
    EquipmentOffline,
    /// Operator requested online; waiting for host.
    AttemptOnline,
    /// Offline at host's request (via S1F15).
    HostOffline,
    /// Online, local control.
    OnlineLocal,
    /// Online, remote (host) control — required for S2F41 remote commands.
    OnlineRemote,
}

impl ControlState {
    /// Whether the equipment is in any offline substate.
    pub fn is_offline(self) -> bool {
        matches!(self, ControlState::EquipmentOffline | ControlState::HostOffline)
    }
}

/// Mode entered when coming online.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnlineMode {
    /// Come up in OnlineLocal.
    Local,
    /// Come up in OnlineRemote.
    Remote,
}

/// An event occurrence to report.
#[derive(Debug, Clone)]
pub struct EventOccurrence {
    /// CEID of the event.
    pub ceid: u32,
    /// Named report values (VID → value) attached to this occurrence.
    pub reports: Vec<(String, Item)>,
}

/// A GEM action the equipment layer must perform in response to a host message.
#[derive(Debug, Clone)]
pub enum GemAction {
    /// Host sent a remote command via S2F41.
    RemoteCommand { command: String, parameters: Vec<(String, Item)> },
    /// Host downloaded a process program (recipe body) via S7F3.
    RecipeReceived { ppid: String, body: Vec<u8> },
    /// Host set equipment constants via S2F15.
    EquipmentConstantsChanged { changed: Vec<(u32, Item)> },
    /// Host asked the equipment to go online (S1F17).
    GoOnlineRequested,
    /// Host asked the equipment to go offline (S1F15).
    GoOfflineRequested,
    /// Terminal message to display (S10F3).
    TerminalMessage { terminal_id: u16, text: String },
}

/// One alarm definition/state entry.
#[derive(Debug, Clone, PartialEq)]
pub struct Alarm {
    /// ALID.
    pub alid: u32,
    /// Alarm text.
    pub text: String,
    /// Whether currently set.
    pub set: bool,
}

/// Equipment-side GEM engine.
pub struct GemEquipment {
    /// Device/session ID used in outgoing headers.
    pub device_id: u16,
    /// Model number reported in S1F2.
    pub mdln: String,
    /// Software revision reported in S1F2.
    pub softrev: String,
    /// Mode entered when the equipment comes online.
    pub default_online_mode: OnlineMode,
    /// Communication state (E30).
    pub com_state: CommunicationState,
    /// Control state (E30).
    pub ctrl_state: ControlState,
    /// Status variables (SVID → current value).
    pub svids: BTreeMap<u32, Item>,
    /// Equipment constants (ECID → current value).
    pub ecids: BTreeMap<u32, Item>,
    /// Named events (CEID → name).
    pub events: BTreeMap<u32, String>,
    /// Alarms (ALID → state).
    pub alarms: BTreeMap<u32, Alarm>,
    /// Process programs stored on the equipment (PPID → body).
    pub process_programs: BTreeMap<String, Vec<u8>>,
    /// Terminal log (most recent last).
    pub terminal_log: Vec<(u16, String)>,
    /// Set date/time string if the host set one.
    pub date_time: Option<String>,
    pending_actions: Vec<GemAction>,
    next_data_id: u32,
}

impl GemEquipment {
    /// Creates an equipment engine with empty registries.
    pub fn new(device_id: u16, mdln: impl Into<String>, softrev: impl Into<String>) -> Self {
        GemEquipment {
            device_id,
            mdln: mdln.into(),
            softrev: softrev.into(),
            default_online_mode: OnlineMode::Remote,
            com_state: CommunicationState::EnabledNotCommunicating,
            ctrl_state: ControlState::EquipmentOffline,
            svids: BTreeMap::new(),
            ecids: BTreeMap::new(),
            events: BTreeMap::new(),
            alarms: BTreeMap::new(),
            process_programs: BTreeMap::new(),
            terminal_log: Vec::new(),
            date_time: None,
            pending_actions: Vec::new(),
            next_data_id: 1,
        }
    }

    /// Defines a status variable.
    pub fn define_svid(&mut self, svid: u32, value: Item) {
        self.svids.insert(svid, value);
    }

    /// Defines an equipment constant.
    pub fn define_ecid(&mut self, ecid: u32, value: Item) {
        self.ecids.insert(ecid, value);
    }

    /// Defines a named event.
    pub fn define_ceid(&mut self, ceid: u32, name: &str) {
        self.events.insert(ceid, name.to_string());
    }

    /// Defines an alarm (initially clear).
    pub fn define_alid(&mut self, alid: u32, text: &str) {
        self.alarms.insert(alid, Alarm { alid, text: text.to_string(), set: false });
    }

    /// Operator requests online in the given mode (L-loc/L-rem button equivalent).
    pub fn operator_request_online(&mut self, mode: OnlineMode) {
        if self.ctrl_state.is_offline() || self.ctrl_state == ControlState::AttemptOnline {
            self.ctrl_state = match mode {
                OnlineMode::Local => ControlState::OnlineLocal,
                OnlineMode::Remote => ControlState::OnlineRemote,
            };
        }
    }

    /// Operator or host takes the equipment offline.
    pub fn go_offline(&mut self, by_host: bool) {
        self.ctrl_state =
            if by_host { ControlState::HostOffline } else { ControlState::EquipmentOffline };
        self.com_state = CommunicationState::EnabledNotCommunicating;
    }

    /// Builds an S1F1 (Are You There) request.
    pub fn make_are_you_there(&mut self) -> SecsMessage {
        let sys = self.next_system();
        SecsMessage::data(self.device_id, 1, 1, true, sys, None)
    }

    /// Builds an S1F13 establish-communications request and, if accepted locally, moves the
    /// communication state to communicating (the caller sends it and awaits S1F14).
    pub fn make_establish_request(&mut self) -> SecsMessage {
        let sys = self.next_system();
        SecsMessage::data(
            self.device_id,
            1,
            13,
            true,
            sys,
            Some(Item::L(vec![Item::A(self.mdln.clone()), Item::A(self.softrev.clone())])),
        )
    }

    /// Builds an S5F1 alarm report and records the alarm state.
    pub fn make_alarm(&mut self, alid: u32, set: bool) -> Result<SecsMessage, FabError> {
        let alarm = self
            .alarms
            .get_mut(&alid)
            .ok_or_else(|| FabError::Gem(format!("unknown ALID {alid}")))?;
        alarm.set = set;
        let alcd = if set { 0x01 } else { 0x00 };
        let text = alarm.text.clone();
        let sys = self.next_system();
        Ok(SecsMessage::data(
            self.device_id,
            5,
            1,
            true,
            sys,
            Some(Item::L(vec![Item::B(vec![alcd]), Item::U4(vec![alid]), Item::A(text)])),
        ))
    }

    /// Builds an S6F11 event report for an occurrence and returns it.
    pub fn make_event_report(&mut self, occurrence: &EventOccurrence) -> SecsMessage {
        let data_id = self.next_data_id;
        self.next_data_id += 1;
        let reports: Vec<Item> = occurrence
            .reports
            .iter()
            .map(|(vid, value)| Item::L(vec![Item::A(vid.clone()), value.clone()]))
            .collect();
        let sys = self.next_system();
        SecsMessage::data(
            self.device_id,
            6,
            11,
            true,
            sys,
            Some(Item::L(vec![
                Item::U4(vec![data_id]),
                Item::U4(vec![occurrence.ceid]),
                Item::L(reports),
            ])),
        )
    }

    /// Handles a host→equipment message and produces the reply, updating state and queueing
    /// [`GemAction`]s.
    pub fn handle_message(&mut self, msg: &SecsMessage) -> Option<SecsMessage> {
        let h = msg.header;
        if h.s_type != 0 {
            return None; // control frames never reach the GEM layer
        }
        let reply_sys = h.system_bytes;
        let reply = match (h.stream, h.function) {
            (1, 1) => Some(self.reply_on_line_data(reply_sys)),
            (1, 3) => Some(self.reply_status_values(reply_sys, msg.item.as_ref())),
            (1, 13) => Some(self.reply_establish_communications(reply_sys)),
            (1, 15) => {
                self.pending_actions.push(GemAction::GoOfflineRequested);
                Some(SecsMessage::data(
                    self.device_id,
                    1,
                    16,
                    false,
                    reply_sys,
                    Some(Item::B(vec![0])),
                ))
            }
            (1, 17) => {
                self.pending_actions.push(GemAction::GoOnlineRequested);
                Some(SecsMessage::data(
                    self.device_id,
                    1,
                    18,
                    false,
                    reply_sys,
                    Some(Item::B(vec![0])),
                ))
            }
            (2, 13) => Some(self.reply_equipment_constants(reply_sys, msg.item.as_ref())),
            (2, 15) => Some(self.reply_new_equipment_constants(reply_sys, msg.item.as_ref())),
            (2, 31) => {
                if let Some(Item::A(s)) = msg.item.as_ref() {
                    self.date_time = Some(s.clone());
                }
                Some(SecsMessage::data(
                    self.device_id,
                    2,
                    32,
                    false,
                    reply_sys,
                    Some(Item::B(vec![0])),
                ))
            }
            (2, 41) => Some(self.reply_remote_command(reply_sys, msg.item.as_ref())),
            (7, 1) => Some(self.reply_load_pp_request(reply_sys, msg.item.as_ref())),
            (7, 3) => Some(self.reply_process_program(reply_sys, msg.item.as_ref())),
            (7, 5) => Some(self.reply_pp_request(reply_sys, msg.item.as_ref())),
            (10, 3) => Some(self.reply_terminal(reply_sys, msg.item.as_ref())),
            _ => None,
        };
        reply
    }

    /// Drains pending actions.
    pub fn take_actions(&mut self) -> Vec<GemAction> {
        std::mem::take(&mut self.pending_actions)
    }

    fn next_system(&mut self) -> u32 {
        self.next_data_id = self.next_data_id.wrapping_add(1);
        self.next_data_id
    }

    fn reply_on_line_data(&mut self, sys: u32) -> SecsMessage {
        SecsMessage::data(
            self.device_id,
            1,
            2,
            false,
            sys,
            Some(Item::L(vec![Item::A(self.mdln.clone()), Item::A(self.softrev.clone())])),
        )
    }

    fn reply_establish_communications(&mut self, sys: u32) -> SecsMessage {
        // Accept and enter communicating.
        self.com_state = CommunicationState::EnabledCommunicating;
        SecsMessage::data(
            self.device_id,
            1,
            14,
            false,
            sys,
            Some(Item::L(vec![
                Item::B(vec![0]), // 0 = accepted
                Item::L(vec![Item::A(self.mdln.clone()), Item::A(self.softrev.clone())]),
            ])),
        )
    }

    fn reply_status_values(&mut self, sys: u32, req: Option<&Item>) -> SecsMessage {
        let values = match req {
            Some(Item::L(vids)) if !vids.is_empty() => vids
                .iter()
                .map(|vid| match vid.as_u4() {
                    Some(id) => self.svids.get(&id).cloned().unwrap_or(Item::Boolean(vec![])),
                    None => Item::Boolean(vec![]),
                })
                .collect::<Vec<_>>(),
            // Empty or absent request = all SVIDs.
            _ => self.svids.values().cloned().collect(),
        };
        SecsMessage::data(self.device_id, 1, 4, false, sys, Some(Item::L(values)))
    }

    fn reply_equipment_constants(&mut self, sys: u32, req: Option<&Item>) -> SecsMessage {
        let values = match req {
            Some(Item::L(ids)) if !ids.is_empty() => ids
                .iter()
                .map(|id| match id.as_u4() {
                    Some(ec) => self.ecids.get(&ec).cloned().unwrap_or(Item::Boolean(vec![])),
                    None => Item::Boolean(vec![]),
                })
                .collect(),
            _ => self.ecids.values().cloned().collect(),
        };
        SecsMessage::data(self.device_id, 2, 14, false, sys, Some(Item::L(values)))
    }

    fn reply_new_equipment_constants(&mut self, sys: u32, req: Option<&Item>) -> SecsMessage {
        let mut changed = Vec::new();
        let acks: Vec<Item> = match req {
            Some(Item::L(pairs)) => pairs
                .iter()
                .map(|pair| {
                    if let Item::L(kv) = pair {
                        if kv.len() == 2 {
                            if let Some(ecid) = kv[0].as_u4() {
                                let value = kv[1].clone();
                                self.ecids.insert(ecid, value.clone());
                                changed.push((ecid, value));
                                return Item::U1(vec![0]); // accepted
                            }
                        }
                    }
                    Item::U1(vec![1]) // invalid format
                })
                .collect(),
            _ => vec![Item::U1(vec![1])],
        };
        if !changed.is_empty() {
            self.pending_actions.push(GemAction::EquipmentConstantsChanged { changed });
        }
        SecsMessage::data(self.device_id, 2, 16, false, sys, Some(Item::L(acks)))
    }

    fn reply_remote_command(&mut self, sys: u32, req: Option<&Item>) -> SecsMessage {
        // S2F41: L[RCMD A, L[CPNAME, val]...]
        let parsed = match req {
            Some(Item::L(parts)) if !parts.is_empty() => {
                let command = parts[0].as_ascii().unwrap_or("").to_string();
                let params = match parts.get(1) {
                    Some(Item::L(cps)) => cps
                        .iter()
                        .filter_map(|cp| match cp {
                            Item::L(kv) if kv.len() == 2 => {
                                kv[0].as_ascii().map(|n| (n.to_string(), kv[1].clone()))
                            }
                            _ => None,
                        })
                        .collect(),
                    _ => Vec::new(),
                };
                Some((command, params))
            }
            _ => None,
        };
        let hcack = match &parsed {
            Some((command, _)) if !command.is_empty() => 0, // acknowledged, will do
            _ => 1,                                         // invalid command
        };
        if let Some((command, parameters)) = parsed {
            self.pending_actions.push(GemAction::RemoteCommand { command, parameters });
        }
        SecsMessage::data(
            self.device_id,
            2,
            42,
            false,
            sys,
            Some(Item::L(vec![Item::B(vec![hcack]), Item::L(vec![])])),
        )
    }

    fn reply_load_pp_request(&mut self, sys: u32, req: Option<&Item>) -> SecsMessage {
        // S7F1: L[PPID, LENGTH] -> S7F2 PPGNT (0 = grant).
        let known = match req {
            Some(Item::L(parts)) => parts
                .first()
                .and_then(|p| p.as_ascii())
                .map(|ppid| self.process_programs.contains_key(ppid))
                .unwrap_or(false),
            _ => false,
        };
        // Grant when new (0) — 5 = "PPID already in use" style refusal otherwise.
        let code = if known { 5u8 } else { 0 };
        SecsMessage::data(self.device_id, 7, 2, false, sys, Some(Item::B(vec![code])))
    }

    fn reply_process_program(&mut self, sys: u32, req: Option<&Item>) -> SecsMessage {
        // S7F3: L[PPID, BODY] -> S7F4 ack.
        let ack = match req {
            Some(Item::L(parts)) if parts.len() == 2 => {
                let ppid = parts[0].as_ascii().unwrap_or("").to_string();
                if let Item::B(body) = &parts[1] {
                    let body = body.clone();
                    self.pending_actions
                        .push(GemAction::RecipeReceived { ppid: ppid.clone(), body: body.clone() });
                    self.process_programs.insert(ppid, body);
                    0
                } else if let Item::A(body) = &parts[1] {
                    let body = body.clone().into_bytes();
                    self.pending_actions
                        .push(GemAction::RecipeReceived { ppid: ppid.clone(), body: body.clone() });
                    self.process_programs.insert(ppid, body);
                    0
                } else {
                    3 // bad format
                }
            }
            _ => 3,
        };
        SecsMessage::data(self.device_id, 7, 4, false, sys, Some(Item::B(vec![ack])))
    }

    fn reply_pp_request(&mut self, sys: u32, req: Option<&Item>) -> SecsMessage {
        // S7F5: PPID -> S7F6 L[PPID, BODY] (empty A on miss).
        let ppid = req.and_then(|i| i.as_ascii()).unwrap_or("").to_string();
        match self.process_programs.get(&ppid) {
            Some(body) => SecsMessage::data(
                self.device_id,
                7,
                6,
                false,
                sys,
                Some(Item::L(vec![Item::A(ppid), Item::B(body.clone())])),
            ),
            None => SecsMessage::data(
                self.device_id,
                7,
                6,
                false,
                sys,
                Some(Item::L(vec![Item::A(ppid), Item::A(String::new())])),
            ),
        }
    }

    fn reply_terminal(&mut self, sys: u32, req: Option<&Item>) -> SecsMessage {
        // S10F3: L[TID, TEXT].
        let (tid, text) = match req {
            Some(Item::L(parts)) if parts.len() == 2 => {
                let tid = parts[0].as_u1().map(u16::from).unwrap_or(0);
                let text = parts[1].as_ascii().unwrap_or("").to_string();
                self.terminal_log.push((tid, text.clone()));
                (tid, text)
            }
            _ => (0, String::new()),
        };
        if !text.is_empty() {
            self.pending_actions.push(GemAction::TerminalMessage { terminal_id: tid, text });
        }
        SecsMessage::data(
            self.device_id,
            10,
            4,
            false,
            sys,
            Some(Item::B(vec![0])), // accepted
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(stream: u8, function: u8, item: Option<Item>) -> SecsMessage {
        SecsMessage::data(0, stream, function, false, 1, item)
    }

    #[test]
    fn establish_communications_moves_com_state() {
        let mut gem = GemEquipment::new(10, "TPT-SIM", "0.1");
        assert_eq!(gem.com_state, CommunicationState::EnabledNotCommunicating);
        let reply = gem.handle_message(&msg(1, 13, Some(Item::L(vec![])))).unwrap();
        assert_eq!(reply.name(), "S1F14");
        assert_eq!(gem.com_state, CommunicationState::EnabledCommunicating);
        assert_eq!(reply.item.as_ref().unwrap().as_list().unwrap()[0].ack_code(), Some(0));
    }

    #[test]
    fn svid_roundtrip_via_s1f3() {
        let mut gem = GemEquipment::new(10, "TPT-SIM", "0.1");
        gem.define_svid(101, Item::A("r1".into()));
        let req = msg(1, 3, Some(Item::L(vec![Item::U4(vec![101])])));
        let reply = gem.handle_message(&req).unwrap();
        let vals = reply.item.unwrap();
        assert_eq!(vals.as_list().unwrap()[0].as_ascii(), Some("r1"));
    }

    #[test]
    fn remote_command_queues_action_and_acks() {
        let mut gem = GemEquipment::new(10, "TPT-SIM", "0.1");
        gem.operator_request_online(OnlineMode::Remote);
        assert_eq!(gem.ctrl_state, ControlState::OnlineRemote);
        let cmd = msg(
            2,
            41,
            Some(Item::L(vec![
                Item::A("START".into()),
                Item::L(vec![Item::L(vec![Item::A("PPID".into()), Item::A("etch-std".into())])]),
            ])),
        );
        let reply = gem.handle_message(&cmd).unwrap();
        assert_eq!(reply.item.unwrap().as_list().unwrap()[0].ack_code(), Some(0));
        match gem.take_actions().as_slice() {
            [GemAction::RemoteCommand { command, parameters }] => {
                assert_eq!(command, "START");
                assert_eq!(parameters[0].0, "PPID");
            }
            other => panic!("unexpected actions: {other:?}"),
        }
    }

    #[test]
    fn recipe_upload_and_download_roundtrip() {
        let mut gem = GemEquipment::new(10, "TPT-SIM", "0.1");
        let up = msg(7, 3, Some(Item::L(vec![Item::A("r1".into()), Item::B(vec![1, 2, 3])])));
        assert_eq!(gem.handle_message(&up).unwrap().item.unwrap().ack_code(), Some(0));
        let down = msg(7, 5, Some(Item::A("r1".into())));
        let reply = gem.handle_message(&down).unwrap();
        let parts = reply.item.unwrap().as_list().unwrap().to_vec();
        assert_eq!(parts[0].as_ascii(), Some("r1"));
        assert_eq!(parts[1], Item::B(vec![1, 2, 3]));
    }

    #[test]
    fn alarm_report_records_state() {
        let mut gem = GemEquipment::new(10, "TPT-SIM", "0.1");
        gem.define_alid(5, "chamber overtemp");
        let m = gem.make_alarm(5, true).unwrap();
        assert_eq!(m.name(), "S5F1W");
        assert!(gem.alarms.get(&5).unwrap().set);
        let m2 = gem.make_alarm(5, false).unwrap();
        let body = m2.item.unwrap();
        assert_eq!(body.as_list().unwrap()[0], Item::B(vec![0]));
    }

    #[test]
    fn offline_online_transitions() {
        let mut gem = GemEquipment::new(10, "TPT-SIM", "0.1");
        // Host takes equipment offline.
        gem.handle_message(&msg(1, 15, None));
        assert!(matches!(gem.take_actions().as_slice(), [GemAction::GoOfflineRequested]));
        assert!(gem.ctrl_state.is_offline());
        assert_eq!(gem.com_state, CommunicationState::EnabledNotCommunicating);
        // Host requests online -> accepted, action queued.
        gem.handle_message(&msg(1, 17, None));
        assert!(matches!(gem.take_actions().as_slice(), [GemAction::GoOnlineRequested]));
        // The simulator then applies the default online mode.
        gem.operator_request_online(gem.default_online_mode);
        assert_eq!(gem.ctrl_state, ControlState::OnlineRemote);
    }
}
