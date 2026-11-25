use std::fmt;

/// Where the endpoint gets the random identifiers a browser generates: its UUID, its default
/// browser context and the targets it creates.
pub trait IdSource: Send {
    fn browser_id(&mut self) -> String;
    fn context_id(&mut self) -> String;
    fn target_id(&mut self) -> String;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetKind {
    Page,
    BrowserUi,
}

impl TargetKind {
    pub fn as_str(self) -> &'static str {
        match self {
            TargetKind::Page => "page",
            TargetKind::BrowserUi => "browser_ui",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Target {
    pub id: String,
    pub kind: TargetKind,
    pub title: String,
    pub url: String,
    pub description: String,
    activity: u64,
}

impl Target {
    pub fn new(id: &str, kind: TargetKind, title: &str, url: &str) -> Self {
        Self { id: id.to_owned(), kind, title: title.to_owned(), url: url.to_owned(), description: String::new(), activity: 0 }
    }

    /// Marks the target as last used at `activity`; larger values are more recent.
    pub fn used_at(mut self, activity: u64) -> Self {
        self.activity = activity;
        self
    }
}

pub struct Registry {
    browser_id: String,
    context_id: String,
    targets: Vec<Target>,
    clock: u64,
    ids: Box<dyn IdSource>,
}

impl fmt::Debug for Registry {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("Registry").field("browser_id", &self.browser_id).field("targets", &self.targets).finish_non_exhaustive()
    }
}

impl Registry {
    /// A registry holding `targets` in creation order, each with the time it was last used.
    pub fn new(mut ids: Box<dyn IdSource>, browser_id: Option<String>, targets: Vec<Target>) -> Self {
        let browser_id = browser_id.unwrap_or_else(|| ids.browser_id());
        let context_id = ids.context_id();
        let clock = targets.iter().map(|target| target.activity).max().unwrap_or(0);
        Self { browser_id, context_id, targets, clock, ids }
    }

    pub fn browser_id(&self) -> &str {
        &self.browser_id
    }

    pub fn context_id(&self) -> &str {
        &self.context_id
    }

    pub fn get(&self, id: &str) -> Option<&Target> {
        self.targets.iter().find(|target| target.id == id)
    }

    /// Targets in creation order.
    pub fn targets(&self) -> &[Target] {
        &self.targets
    }

    /// Targets with the most recently used first.
    pub fn by_activity(&self) -> Vec<&Target> {
        let mut targets: Vec<&Target> = self.targets.iter().collect();
        targets.sort_by_key(|target| std::cmp::Reverse(target.activity));
        targets
    }

    pub fn create(&mut self, url: &str) -> Target {
        self.clock += 1;
        let mut target = Target::new(&self.ids.target_id(), TargetKind::Page, "", url);
        target.activity = self.clock;
        self.targets.push(target.clone());
        target
    }

    pub fn activate(&mut self, id: &str) -> bool {
        self.clock += 1;
        let clock = self.clock;
        self.targets.iter_mut().find(|target| target.id == id).map(|target| target.activity = clock).is_some()
    }

    pub fn close(&mut self, id: &str) -> bool {
        let before = self.targets.len();
        self.targets.retain(|target| target.id != id);
        self.targets.len() != before
    }
}

/// Identifiers from the operating system's random source, as a browser generates them.
#[derive(Debug, Default)]
pub struct RandomIds;

impl RandomIds {
    fn random_bytes() -> [u8; 16] {
        let mut bytes = [0u8; 16];
        getrandom::fill(&mut bytes).expect("the operating system provides random bytes");
        bytes
    }

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|byte| format!("{byte:02X}")).collect()
    }
}

impl IdSource for RandomIds {
    fn browser_id(&mut self) -> String {
        let mut bytes = Self::random_bytes();
        bytes[6] = bytes[6] & 0x0f | 0x40;
        bytes[8] = bytes[8] & 0x3f | 0x80;
        let hex = Self::hex(&bytes).to_ascii_lowercase();
        format!("{}-{}-{}-{}-{}", &hex[..8], &hex[8..12], &hex[12..16], &hex[16..20], &hex[20..])
    }

    fn context_id(&mut self) -> String {
        Self::hex(&Self::random_bytes())
    }

    fn target_id(&mut self) -> String {
        Self::hex(&Self::random_bytes())
    }
}

#[derive(Debug)]
pub struct SequenceIds {
    next: u64,
}

impl SequenceIds {
    pub fn new() -> Self {
        Self { next: 0 }
    }

    fn next_hex(&mut self) -> String {
        self.next += 1;
        format!("{:032X}", self.next)
    }
}

impl Default for SequenceIds {
    fn default() -> Self {
        Self::new()
    }
}

impl IdSource for SequenceIds {
    fn browser_id(&mut self) -> String {
        let id = self.next_hex();
        format!("{}-{}-4{}-8{}-{}", &id[..8], &id[8..12], &id[13..16], &id[17..20], &id[20..32])
    }

    fn context_id(&mut self) -> String {
        self.next_hex()
    }

    fn target_id(&mut self) -> String {
        self.next_hex()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn launch() -> Registry {
        Registry::new(
            Box::new(SequenceIds::new()),
            Some("00000000-0000-4000-8000-000000000001".into()),
            vec![
                Target::new("1", TargetKind::Page, "about:blank", "about:blank").used_at(2),
                Target::new("2", TargetKind::BrowserUi, "Omnibox Popup", "chrome://omnibox-popup.top-chrome/").used_at(1),
            ],
        )
    }

    #[test]
    fn listing_follows_activity_and_creation_order_is_kept() {
        let mut registry = launch();
        let created = registry.create("about:blank");
        assert_eq!(registry.by_activity().iter().map(|target| target.id.as_str()).collect::<Vec<_>>(), [created.id.as_str(), "1", "2"]);
        assert!(registry.activate("2"));
        assert_eq!(registry.by_activity()[0].id, "2");
        assert_eq!(registry.targets().iter().map(|target| target.id.as_str()).collect::<Vec<_>>(), ["1", "2", created.id.as_str()]);
        assert!(registry.close("2") && !registry.close("2"));
        assert!(registry.get("2").is_none());
    }
}
