mod link;
mod probe;
mod classifier;
mod event;
mod fingerprint;
mod ignore;
mod standards;

pub use link::{parse_link_header, Link};
pub use probe::{Probe, ProbeRegistry, ProbeMatch, ProbeRequest};
pub use classifier::{ClassifierRegistry, Classification, Validity};
pub use event::{DiscoveryEvent, Via, EVENT_VERSION};
pub use fingerprint::{fingerprint, DedupSet};
pub use ignore::IgnoreList;
pub use standards::{
    default_classifiers, default_llmstxtignore_entries, default_probes,
    known_link_rels, ResponseSignal, classify_response, classify_slicc_handoff,
    is_slicc_handoff_rel, parse_llms_txt_summary,
    parse_robots_txt_agentmap, classify_ard_body, classify_ai_catalog_body,
    classify_poppy_body, registrable_domain,
    HANDOFF_REL, UPSKILL_REL, LLMS_TXT_REL,
};
