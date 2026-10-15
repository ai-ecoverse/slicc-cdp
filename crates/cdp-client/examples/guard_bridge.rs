use cdp_client::cdp::Cdp;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::io::{self, BufRead, Write};
use std::sync::mpsc;
use std::time::{Duration, Instant};

fn emit(value: Value) {
    println!("{value}");
    io::stdout().flush().unwrap();
}

fn answer(id: Value, result: Result<Value, String>) {
    match result {
        Ok(value) => emit(json!({"id":id,"result":value})),
        Err(message) => emit(json!({"id":id,"error":{"message":message}})),
    }
}

fn main() {
    let url = std::env::args().nth(1).unwrap();
    let mut cdp = Cdp::connect(&url).unwrap();
    let origin = std::env::args().nth(2).unwrap();
    cdp.protect_origins(&[&origin]);
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in io::stdin().lock().lines() {
            if tx.send(line.unwrap()).is_err() {
                break;
            }
        }
    });
    let mut seen = 0;
    let mut pending = BTreeMap::<u64, (String, String, Instant)>::new();
    loop {
        match rx.try_recv() {
            Ok(line) => {
                let message: Value = serde_json::from_str(&line).unwrap();
                let method = message["method"].as_str().unwrap();
                let params = message["params"].clone();
                let session = message["sessionId"].as_str();
                if method == "Runtime.evaluate" && params["awaitPromise"] == true {
                    let id = message["id"].as_u64().unwrap();
                    let key = format!("__guard_bridge_{id}");
                    let expression =
                        serde_json::to_string(params["expression"].as_str().unwrap()).unwrap();
                    let script = format!("(() => {{ const key = {key:?}; globalThis[key] = {{ done: false }}; Promise.resolve((0, eval)({expression})).then(value => {{ globalThis[key] = {{ done: true, value }}; }}, error => {{ globalThis[key] = {{ done: true, error: String(error) }}; }}); return true; }})()");
                    match cdp.call(
                        "Runtime.evaluate",
                        json!({"expression":script,"returnByValue":true,"userGesture":true}),
                        session,
                    ) {
                        Ok(value) if value.get("exceptionDetails").is_none() => {
                            pending.insert(
                                id,
                                (key, session.unwrap_or("").to_string(), Instant::now()),
                            );
                        }
                        result => answer(message["id"].clone(), result),
                    }
                } else {
                    let result = cdp.call(method, params, session);
                    answer(message["id"].clone(), result);
                }
            }
            Err(mpsc::TryRecvError::Disconnected) => break,
            Err(mpsc::TryRecvError::Empty) => {}
        }
        let _ = cdp.pump(Instant::now() + Duration::from_millis(5));
        while let Ok(Some(paused)) = cdp.next_paused(None, Duration::ZERO) {
            emit(
                json!({"event":{"method":"Fetch.requestPaused","sessionId":paused.session_id,"params":{"requestId":paused.request_id,"frameId":paused.frame_id,"resourceType":paused.resource_type,"responseStatusCode":paused.status,"request":{"url":paused.url,"method":paused.method,"headers":paused.headers}}}}),
            );
        }
        for event in &cdp.events()[seen..] {
            emit(json!({"event":event}));
        }
        seen = cdp.events().len();
        let mut done = Vec::new();
        for (id, (key, session, started)) in &pending {
            let expression = format!("JSON.stringify(globalThis[{key:?}])");
            let result = cdp.call_for(
                "Runtime.evaluate",
                json!({"expression":expression,"returnByValue":true}),
                Some(session),
                Duration::from_secs(2),
            );
            match result {
                Ok(value) => {
                    if let Some(text) = value.pointer("/result/value").and_then(Value::as_str) {
                        if let Ok(state) = serde_json::from_str::<Value>(text) {
                            if state["done"] == true {
                                if state.get("error").is_some() {
                                    answer(
                                        json!(id),
                                        Err(state["error"].as_str().unwrap_or("").to_string()),
                                    );
                                } else {
                                    answer(
                                        json!(id),
                                        Ok(
                                            json!({"result":{"type":"object","value":state["value"]}}),
                                        ),
                                    );
                                }
                                done.push(*id);
                            }
                        }
                    }
                    if started.elapsed() > Duration::from_secs(30) {
                        answer(json!(id), Err("bridge evaluation timeout".into()));
                        done.push(*id);
                    }
                }
                Err(message) => {
                    answer(json!(id), Err(message));
                    done.push(*id);
                }
            }
        }
        for id in done {
            pending.remove(&id);
        }
    }
}
