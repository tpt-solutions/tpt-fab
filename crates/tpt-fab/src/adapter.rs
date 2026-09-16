//! Opt-in vendor-adapter seam.
//!
//! Core `tpt-fab` deliberately excludes vendor-proprietary SECS/GEM extensions. This module is
//! the *only* sanctioned seam: a fab running a tool with nonstandard behavior installs a
//! [`VendorAdapter`] explicitly at startup. With no adapter installed (the default) the stack
//! speaks pure SEMI-standard GEM.

use crate::secs::SecsMessage;

/// Decision returned by [`VendorAdapter::intercept_incoming`].
#[derive(Debug, Clone, PartialEq)]
pub enum AdapterDecision {
    /// Process the message normally.
    Pass,
    /// Drop the message silently (vendor protocol says this stream is not used).
    Drop,
}

/// Hook points for vendor-specific protocol behavior.
///
/// Adapters must be pure transformations — they never perform I/O. Registered adapters run
/// before the GEM state model sees a message and after it produces a reply.
pub trait VendorAdapter: Send + Sync {
    /// Vendor identifier for logs, e.g. `"acme-99"`.
    fn vendor_id(&self) -> &str;

    /// Human-readable summary of which extensions this adapter implements.
    fn describe(&self) -> String;

    /// Called for every incoming (host→equipment) data message before GEM processing.
    fn intercept_incoming(&self, _msg: &SecsMessage) -> AdapterDecision {
        AdapterDecision::Pass
    }

    /// Called for every outgoing (equipment→host) data message after GEM processing. The
    /// adapter may rewrite the message in place (e.g. add a vendor-specific list entry).
    fn intercept_outgoing(&self, _msg: &mut SecsMessage) {}
}

/// Registry of installed adapters. Empty by default.
#[derive(Default)]
pub struct AdapterRegistry {
    adapters: Vec<Box<dyn VendorAdapter>>,
}

impl AdapterRegistry {
    /// Empty registry — pure-standard behavior.
    pub fn new() -> Self {
        Self::default()
    }

    /// Installs an adapter (appended; runs after previously installed adapters).
    pub fn install(&mut self, adapter: Box<dyn VendorAdapter>) {
        self.adapters.push(adapter);
    }

    /// Whether any adapter is installed.
    pub fn is_empty(&self) -> bool {
        self.adapters.is_empty()
    }

    /// Descriptions of all installed adapters.
    pub fn describe(&self) -> Vec<String> {
        self.adapters.iter().map(|a| format!("{}: {}", a.vendor_id(), a.describe())).collect()
    }

    /// Runs incoming interception through all adapters in order; first Drop wins.
    pub fn intercept_incoming(&self, msg: &SecsMessage) -> AdapterDecision {
        for a in &self.adapters {
            if a.intercept_incoming(msg) == AdapterDecision::Drop {
                return AdapterDecision::Drop;
            }
        }
        AdapterDecision::Pass
    }

    /// Runs outgoing interception through all adapters in order.
    pub fn intercept_outgoing(&self, msg: &mut SecsMessage) {
        for a in &self.adapters {
            a.intercept_outgoing(msg);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secs::Item;

    struct NoopAdapter;
    impl VendorAdapter for NoopAdapter {
        fn vendor_id(&self) -> &str {
            "acme"
        }
        fn describe(&self) -> String {
            "maps S150 vendor stream".into()
        }
    }

    struct DroppingAdapter;
    impl VendorAdapter for DroppingAdapter {
        fn vendor_id(&self) -> &str {
            "acme-drop"
        }
        fn describe(&self) -> String {
            String::new()
        }
        fn intercept_incoming(&self, msg: &SecsMessage) -> AdapterDecision {
            if msg.header.stream == 150 {
                AdapterDecision::Drop
            } else {
                AdapterDecision::Pass
            }
        }
    }

    #[test]
    fn default_registry_is_pure_standard() {
        let reg = AdapterRegistry::new();
        assert!(reg.is_empty());
        let msg = SecsMessage::data(0, 1, 13, false, 1, Some(Item::A("x".into())));
        assert_eq!(reg.intercept_incoming(&msg), AdapterDecision::Pass);
    }

    #[test]
    fn adapter_can_drop_vendor_streams() {
        let mut reg = AdapterRegistry::new();
        reg.install(Box::new(NoopAdapter));
        reg.install(Box::new(DroppingAdapter));
        let vendor = SecsMessage::data(0, 150, 1, false, 1, None);
        assert_eq!(reg.intercept_incoming(&vendor), AdapterDecision::Drop);
        let standard = SecsMessage::data(0, 1, 13, false, 1, None);
        assert_eq!(reg.intercept_incoming(&standard), AdapterDecision::Pass);
        assert_eq!(reg.describe().len(), 2);
    }
}
