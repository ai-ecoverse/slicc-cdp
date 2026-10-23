use cdp_client::cdp::Transport;
use serde_json::{json, Value};
use std::cell::Cell;
use std::io::{self, Write};
use std::rc::Rc;
use std::time::Instant;

fn record(id: u64, stage: &str, method: &str, upstream: Option<i64>) {
    println!("{}", json!({"audit":{"id":id,"stage":stage,"method":method,"upstreamId":upstream}}));
    io::stdout().flush().unwrap();
}

pub struct Audited<T> {
    inner: T,
    command: Rc<Cell<Option<u64>>>,
}

impl<T: Transport> Audited<T> {
    pub fn new(inner: T, command: Rc<Cell<Option<u64>>>) -> Self { Self { inner, command } }
}

impl<T: Transport> Transport for Audited<T> {
    fn send_text(&mut self, text: &str) -> Result<(), String> {
        let message: Value = serde_json::from_str(text).map_err(|error| format!("invalid audited CDP command: {error}"))?;
        let id = self.command.get();
        let method = message["method"].as_str().ok_or("audited CDP command lacks method")?;
        let upstream = message["id"].as_i64();
        if let Some(id) = id { record(id, "attempted", method, upstream); }
        self.inner.send_text(text)?;
        if let Some(id) = id { record(id, "forwarded", method, upstream); }
        Ok(())
    }
    fn recv_text(&mut self, deadline: Instant) -> Result<Option<String>, String> { self.inner.recv_text(deadline) }
}
