use cdp_client::cdp::{self, ActionNote, Cdp};

pub struct Report {
    note: ActionNote,
    started: bool,
    ended: bool,
}

impl Report {
    pub fn request(method: &str, url: &str) -> Self {
        Self {
            note: ActionNote {
                kind: "request".to_string(),
                method: Some(method.to_string()),
                url: Some(url.to_string()),
                agent: cdp::slicc_agent(),
                ..ActionNote::default()
            },
            started: false,
            ended: false,
        }
    }

    pub fn tab(&mut self, tab: &str) {
        if !tab.is_empty() {
            self.note.tab = Some(tab.to_string());
        }
    }

    pub fn begin(&mut self, cdp: &mut Cdp) {
        if self.started || self.ended {
            return;
        }
        self.started = true;
        cdp.notify("Slicc.action", self.note.start_params(), None);
    }

    pub fn finish(&mut self, cdp: &mut Cdp, code: i32, stderr: &str) {
        if self.ended {
            return;
        }
        if !self.started {
            self.begin(cdp);
        }
        self.ended = true;
        cdp.notify("Slicc.action", self.note.end_params(code == 0, stderr), None);
    }
}
