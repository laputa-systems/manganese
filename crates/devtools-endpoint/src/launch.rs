//! The endpoint in the state the oracle's launch harness leaves the frozen browser in.

use crate::cdp::Identity;
use crate::handler::Config;
use crate::handler::ProtocolSource;
use crate::registry::IdSource;
use crate::registry::RandomIds;
use crate::registry::Registry;
use crate::registry::Target;
use crate::registry::TargetKind;

pub const BROWSER_ID: &str = "00000000-0000-4000-8000-000000000001";
pub const PAGE_ID: &str = "00000000000000000000000000000001";
pub const OMNIBOX_ID: &str = "00000000000000000000000000000002";
pub const OMNIBOX_AIM_ID: &str = "00000000000000000000000000000003";
pub const PROTOCOL_ENV: &str = "MANGANESE_PROTOCOL_JSON";

const REVISION: &str = "@a21ce95c24ce3964416f09ae1862f53f319eb4d0";

pub fn identity() -> Identity {
    Identity {
        protocol_version: "1.3".into(),
        product: "Chrome/156.0.8078.79".into(),
        revision: REVISION.into(),
        user_agent: "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) HeadlessChrome/156.0.0.0 Safari/537.36".into(),
        js_version: "15.6.75.27".into(),
    }
}

/// The protocol description may be overridden by the file named by `MANGANESE_PROTOCOL_JSON`.
pub fn config() -> Config {
    let protocol = std::env::var_os(PROTOCOL_ENV).map_or_else(
        || ProtocolSource::Bytes(include_bytes!("../protocol.json").to_vec()),
        |path| ProtocolSource::File(path.into()),
    );
    Config {
        identity: identity(),
        frontend_host: "chrome-devtools-frontend.8pp2p8t.qjz9zk".into(),
        discovery_page: String::new(),
        allowed_origins: Vec::new(),
        protocol,
    }
}

/// A page on `about:blank` and the two omnibox popups, listed in the order the browser reports them.
pub fn registry(ids: Box<dyn IdSource>) -> Registry {
    Registry::new(
        ids,
        Some(BROWSER_ID.into()),
        vec![
            Target::new(PAGE_ID, TargetKind::Page, "about:blank", "about:blank").used_at(3),
            Target::new(OMNIBOX_AIM_ID, TargetKind::BrowserUi, "Omnibox Popup", "chrome://omnibox-popup.top-chrome/omnibox_popup_aim.html").used_at(1),
            Target::new(OMNIBOX_ID, TargetKind::BrowserUi, "Omnibox Popup", "chrome://omnibox-popup.top-chrome/").used_at(2),
        ],
    )
}

pub fn random_registry() -> Registry {
    registry(Box::new(RandomIds))
}
