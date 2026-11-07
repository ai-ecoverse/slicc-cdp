fn interleave(left: &[String], right: &[String]) -> Vec<Vec<String>> {
    fn walk(left: &[String], right: &[String], prefix: &mut Vec<String>, out: &mut Vec<Vec<String>>) {
        if left.is_empty() && right.is_empty() {
            out.push(prefix.clone());
            return;
        }
        if let Some((head, rest)) = left.split_first() {
            prefix.push(head.clone());
            walk(rest, right, prefix, out);
            prefix.pop();
        }
        if let Some((head, rest)) = right.split_first() {
            prefix.push(head.clone());
            walk(left, rest, prefix, out);
            prefix.pop();
        }
    }
    let mut out = Vec::new();
    walk(left, right, &mut Vec::new(), &mut out);
    out
}

fn holder_sequences() -> Vec<Vec<&'static str>> {
    let events = ["attach", "root_nav", "root_cleared", "root_created"];
    let mut sequences = Vec::new();
    let mut order = [0usize, 1, 2, 3];
    loop {
        let sequence = order.map(|index| events[index]).to_vec();
        let cleared = sequence.iter().position(|event| *event == "root_cleared").unwrap();
        let created = sequence.iter().position(|event| *event == "root_created").unwrap();
        let navigated = sequence.iter().position(|event| *event == "root_nav").unwrap();
        if cleared < created && navigated < created {
            sequences.push(sequence);
        }
        if !next_permutation(&mut order) { break; }
    }
    sequences
}

fn next_permutation(values: &mut [usize]) -> bool {
    let Some(pivot) = values.windows(2).rposition(|pair| pair[0] < pair[1]) else { return false; };
    let successor = values.iter().rposition(|value| *value > values[pivot]).unwrap();
    values.swap(pivot, successor);
    values[pivot + 1..].reverse();
    true
}

fn child_sequences() -> Vec<Vec<&'static str>> {
    vec![vec!["child_ctx", "child_nav"], vec!["child_nav", "child_ctx"]]
}

fn insert_detach(sequence: &[String]) -> Vec<Vec<String>> {
    (0..=sequence.len()).map(|index| {
        let mut next = sequence.to_vec();
        next.insert(index, "old_detach".into());
        next
    }).collect()
}

fn legal_nav_sequences(blank_attach: bool) -> Vec<(bool, Vec<String>)> {
    let mut sequences = Vec::new();
    for holder in holder_sequences() {
        let attach_at = holder.iter().position(|event| *event == "attach").unwrap();
        let before = holder[..attach_at].iter().map(|event| (*event).to_string()).collect::<Vec<_>>();
        let after = holder[attach_at + 1..].iter().map(|event| (*event).to_string()).collect::<Vec<_>>();
        for child in child_sequences() {
            let child_names = child.iter().map(|event| (*event).to_string()).collect::<Vec<_>>();
            for merged in interleave(&after, &child_names) {
                let mut base = before.clone();
                base.push("attach".into());
                base.extend(merged);
                for sequence in insert_detach(&base) {
                    sequences.push((blank_attach, sequence));
                }
            }
        }
    }
    sequences
}

fn shim_evals(state: &Rc<RefCell<BackendState>>, session: &str) -> usize {
    state.borrow().sent.iter().filter(|command| {
        command["sessionId"] == session
            && command["method"] == "Runtime.evaluate"
            && command["params"]["expression"].as_str().is_some_and(webmcp_shim_install)
    }).count()
}

fn assert_nav_ordering(cdp: &Cdp, state: &Rc<RefCell<BackendState>>, holder: &str, label: &str) {
    assert!(!cdp.session_blocked(holder), "root blocked: {label}");
    assert!(!cdp.content_blocked(holder), "root content-blocked: {label}");
    assert!(cdp.webmcp.arms.get(holder).is_some_and(|arm| !arm.revoked), "root revoked: {label}");
    assert!(!cdp.webmcp.parents.contains_key("CHILD-OLD") && !cdp.webmcp.arms.contains_key("CHILD-OLD"), "old child remains: {label}");
    assert!(cdp.webmcp.arms.get("CHILD-NEW").is_some_and(|arm| arm.admitted == 1), "new child admitted!=1: {label} admitted={:?}", cdp.webmcp.arms.get("CHILD-NEW").map(|arm| arm.admitted));
    assert_eq!(shim_evals(state, "CHILD-NEW"), 1, "new child shim evaluates!=1: {label}");
    assert!(cdp.webmcp_generations().iter().any(|generation| generation["session"] == "CHILD-NEW" && generation["frameId"] == "FRAME"), "tools-changed generation missing: {label}");
}

fn play_nav_event(cdp: &mut Cdp, state: &Rc<RefCell<BackendState>>, holder: &str, event: &str, blank_attach: bool) {
    match event {
        "attach" => {
            let url = if blank_attach { "about:blank" } else { "https://foreign.test/cross" };
            if blank_attach {
                state.borrow_mut().oopif_tree = Some(json!({"frameTree":{"frame":{"id":"FRAME","url":"about:blank","securityOrigin":"","loaderId":""}}}));
            } else {
                state.borrow_mut().oopif_tree = Some(json!({"frameTree":{"frame":{"id":"FRAME","url":"https://foreign.test/cross","securityOrigin":"https://foreign.test","loaderId":"CROSS-NEW"}}}));
            }
            cdp.absorb(json!({"method":"Target.attachedToTarget","sessionId":holder,"params":{"sessionId":"CHILD-NEW","targetInfo":{"targetId":"IFRAME","type":"iframe","url":url,"parentFrameId":"F"},"waitingForDebugger":false}}));
        }
        "root_nav" => {
            state.borrow_mut().tree = json!({"frameTree":{"frame":{"id":"F","url":"https://ordinary.test/next","securityOrigin":"https://ordinary.test","loaderId":"NEXT"}}});
            cdp.absorb(json!({"method":"Page.frameNavigated","sessionId":holder,"params":{"frame":{"id":"F","url":"https://ordinary.test/next","securityOrigin":"https://ordinary.test","loaderId":"NEXT"}}}));
        }
        "root_cleared" => {
            cdp.absorb(json!({"method":"Runtime.executionContextsCleared","sessionId":holder,"params":{}}));
        }
        "root_created" => {
            cdp.absorb(json!({"method":"Runtime.executionContextCreated","sessionId":holder,"params":{"context":{"id":11,"uniqueId":"ROOT-NEXT","origin":"https://ordinary.test","auxData":{"frameId":"F","isDefault":true}}}}));
        }
        "old_detach" => {
            cdp.absorb(json!({"method":"Target.detachedFromTarget","params":{"sessionId":"CHILD-OLD","targetId":"IFRAME"}}));
        }
        "child_ctx" => {
            cdp.absorb(json!({"method":"Runtime.executionContextCreated","sessionId":"CHILD-NEW","params":{"context":{"id":9,"uniqueId":"CROSS-NEW","origin":"https://foreign.test","auxData":{"frameId":"FRAME","isDefault":true}}}}));
        }
        "child_nav" => {
            state.borrow_mut().oopif_tree = Some(json!({"frameTree":{"frame":{"id":"FRAME","url":"https://foreign.test/cross","securityOrigin":"https://foreign.test","loaderId":"CROSS-NEW"}}}));
            cdp.absorb(json!({"method":"Page.frameNavigated","sessionId":"CHILD-NEW","params":{"frame":{"id":"FRAME","url":"https://foreign.test/cross","securityOrigin":"https://foreign.test","loaderId":"CROSS-NEW"}}}));
        }
        _ => unreachable!("{event}"),
    }
}

fn seed_nav_holder(blank_attach: bool) -> (Cdp, Rc<RefCell<BackendState>>, String) {
    let (mut cdp, state) = client();
    state.borrow_mut().suppress_runtime_child = true;
    state.borrow_mut().oopif_tree = Some(json!({"frameTree":{"frame":{"id":"FRAME","url":"https://foreign.test/old","securityOrigin":"https://foreign.test","loaderId":"CROSS-OLD"}}}));
    cdp.start_webmcp_for("H", true).unwrap();
    let holder = cdp.webmcp_session("H").to_string();
    cdp.webmcp_auto_attach(&holder).unwrap();
    if let Some(arm) = cdp.webmcp.arms.get_mut(&holder) {
        arm.pending_contexts.clear();
        arm.pending_frames.clear();
        arm.pending_install = None;
    }
    cdp.webmcp_request_origins(&["https://foreign.test".into()]);
    state.borrow_mut().suppress_runtime_child = false;
    cdp.absorb(json!({"method":"Target.attachedToTarget","sessionId":holder,"params":{"sessionId":"CHILD-OLD","targetInfo":{"targetId":"IFRAME","type":"iframe","url":"https://foreign.test/old","parentFrameId":"F"},"waitingForDebugger":false}}));
    cdp.absorb(json!({"method":"Runtime.executionContextCreated","sessionId":"CHILD-OLD","params":{"context":{"id":8,"uniqueId":"CROSS-OLD","origin":"https://foreign.test","auxData":{"frameId":"FRAME","isDefault":true}}}}));
    assert!(cdp.webmcp.arms.get("CHILD-OLD").is_some_and(|arm| arm.admitted > 0), "seed old child");
    state.borrow_mut().suppress_runtime_child = true;
    if blank_attach {
        state.borrow_mut().oopif_tree = Some(json!({"frameTree":{"frame":{"id":"FRAME","url":"about:blank","securityOrigin":"","loaderId":""}}}));
    } else {
        state.borrow_mut().oopif_tree = Some(json!({"frameTree":{"frame":{"id":"FRAME","url":"https://foreign.test/cross","securityOrigin":"https://foreign.test","loaderId":"CROSS-NEW"}}}));
    }
    (cdp, state, holder)
}

#[test]
fn ordinary_navigation_order_permutations_admit_new_child_once() {
    let mut failed = Vec::new();
    let mut total = 0usize;
    for blank_attach in [true, false] {
        for (_, sequence) in legal_nav_sequences(blank_attach) {
            total += 1;
            let label = format!("blank={blank_attach} order={}", sequence.join(">"));
            let (mut cdp, state, holder) = seed_nav_holder(blank_attach);
            for event in &sequence {
                play_nav_event(&mut cdp, &state, &holder, event, blank_attach);
            }
            cdp.webmcp_finish_awaiting_children();
            cdp.webmcp_install_children();
            let ok = !cdp.session_blocked(&holder)
                && !cdp.content_blocked(&holder)
                && cdp.webmcp.arms.get(&holder).is_some_and(|arm| !arm.revoked)
                && !cdp.webmcp.parents.contains_key("CHILD-OLD")
                && !cdp.webmcp.arms.contains_key("CHILD-OLD")
                && cdp.webmcp.arms.get("CHILD-NEW").is_some_and(|arm| arm.admitted == 1)
                && shim_evals(&state, "CHILD-NEW") == 1
                && cdp.webmcp_generations().iter().any(|generation| generation["session"] == "CHILD-NEW");
            if !ok {
                let admitted = cdp.webmcp.arms.get("CHILD-NEW").map(|arm| arm.admitted);
                let revoked = cdp.webmcp.arms.get(&holder).is_some_and(|arm| arm.revoked);
                let old_present = cdp.webmcp.arms.contains_key("CHILD-OLD") || cdp.webmcp.parents.contains_key("CHILD-OLD");
                let gens = cdp.webmcp_generations().len();
                let notices = cdp.take_notices().join("|");
                failed.push(format!(
                    "{label} admitted={admitted:?} evals={} blocked={} revoked={revoked} old_present={old_present} gens={gens} notices={notices}",
                    shim_evals(&state, "CHILD-NEW"),
                    cdp.session_blocked(&holder),
                ));
            } else {
                assert_nav_ordering(&cdp, &state, &holder, &label);
            }
        }
    }
    assert!(failed.is_empty(), "failing orderings {}/{}:\n{}", failed.len(), total, failed.join("\n"));
}

fn play_start_event(cdp: &mut Cdp, state: &Rc<RefCell<BackendState>>, holder: &str, event: &str) {
    match event {
        "auto_attach" => {
            cdp.webmcp_auto_attach(holder).unwrap();
        }
        "attach" => {
            state.borrow_mut().oopif_tree = Some(json!({"frameTree":{"frame":{"id":"FRAME","url":"https://foreign.test/cross","securityOrigin":"https://foreign.test","loaderId":"CROSS"}}}));
            cdp.absorb(json!({"method":"Target.attachedToTarget","sessionId":holder,"params":{"sessionId":"CHILD","targetInfo":{"targetId":"IFRAME","type":"iframe","url":"https://foreign.test/cross","parentFrameId":"F"},"waitingForDebugger":false}}));
        }
        "attach_during" => {
            state.borrow_mut().attach_on_auto_attach = true;
            state.borrow_mut().oopif_tree = Some(json!({"frameTree":{"frame":{"id":"FRAME","url":"https://foreign.test/cross","securityOrigin":"https://foreign.test","loaderId":"CROSS"}}}));
            cdp.webmcp_auto_attach(holder).unwrap();
            state.borrow_mut().attach_on_auto_attach = false;
        }
        "pause" => {
            cdp.absorb(json!({"method":"Debugger.paused","sessionId":holder,"params":{"reason":"other","callFrames":[{"callFrameId":"CALL","functionName":"","location":{"scriptId":"SCRIPT","lineNumber":0,"columnNumber":0}}]}}));
        }
        "resume" => {
            let _ = cdp.send("Debugger.resume", json!({}), Some(holder));
            cdp.webmcp.paused.remove(holder);
        }
        "child_ctx" => {
            cdp.absorb(json!({"method":"Runtime.executionContextCreated","sessionId":"CHILD","params":{"context":{"id":9,"uniqueId":"CROSS-CTX","origin":"https://foreign.test","auxData":{"frameId":"FRAME","isDefault":true}}}}));
        }
        "child_nav" => {
            state.borrow_mut().oopif_tree = Some(json!({"frameTree":{"frame":{"id":"FRAME","url":"https://foreign.test/cross","securityOrigin":"https://foreign.test","loaderId":"CROSS"}}}));
            cdp.absorb(json!({"method":"Page.frameNavigated","sessionId":"CHILD","params":{"frame":{"id":"FRAME","url":"https://foreign.test/cross","securityOrigin":"https://foreign.test","loaderId":"CROSS"}}}));
        }
        _ => unreachable!("{event}"),
    }
}

fn start_sequences() -> Vec<Vec<&'static str>> {
    vec![
        vec!["auto_attach", "attach", "child_nav", "child_ctx"],
        vec!["auto_attach", "attach", "child_ctx", "child_nav"],
        vec!["attach_during", "child_nav", "child_ctx"],
        vec!["attach_during", "child_ctx", "child_nav"],
        vec!["auto_attach", "pause", "attach", "resume", "child_nav", "child_ctx"],
        vec!["auto_attach", "attach", "pause", "resume", "child_ctx", "child_nav"],
        vec!["pause", "auto_attach", "attach", "resume", "child_ctx", "child_nav"],
        vec!["auto_attach", "attach", "child_ctx", "pause", "resume", "child_nav"],
        vec!["auto_attach", "attach", "child_nav", "pause", "resume", "child_ctx"],
        vec!["auto_attach", "attach", "pause", "child_ctx", "resume", "child_nav"],
        vec!["auto_attach", "attach", "pause", "child_nav", "resume", "child_ctx"],
        vec!["attach_during", "pause", "resume", "child_ctx", "child_nav"],
    ]
}

#[test]
fn from_origin_start_order_permutations_admit_child_once() {
    let mut failed = Vec::new();
    let mut total = 0usize;
    for sequence in start_sequences() {
        total += 1;
        let label = sequence.join(">");
        let (mut cdp, state) = client();
        state.borrow_mut().suppress_runtime_child = true;
        state.borrow_mut().oopif_tree = Some(json!({"frameTree":{"frame":{"id":"FRAME","url":"https://foreign.test/cross","securityOrigin":"https://foreign.test","loaderId":"CROSS"}}}));
        cdp.start_webmcp_for("H", true).unwrap();
        let holder = cdp.webmcp_session("H").to_string();
        if let Some(arm) = cdp.webmcp.arms.get_mut(&holder) {
            arm.pending_contexts.clear();
            arm.pending_frames.clear();
            arm.pending_install = None;
        }
        cdp.webmcp_request_origins(&["https://foreign.test".into()]);
        for event in &sequence {
            play_start_event(&mut cdp, &state, &holder, event);
        }
        cdp.webmcp_finish_awaiting_children();
        cdp.webmcp_install_children();
        let ok = !cdp.session_blocked(&holder)
            && !cdp.content_blocked(&holder)
            && cdp.webmcp.arms.get(&holder).is_some_and(|arm| !arm.revoked)
            && cdp.webmcp.arms.get("CHILD").is_some_and(|arm| arm.admitted == 1)
            && shim_evals(&state, "CHILD") == 1
            && cdp.webmcp_generations().iter().any(|generation| generation["session"] == "CHILD");
        if !ok {
            let admitted = cdp.webmcp.arms.get("CHILD").map(|arm| arm.admitted);
            let parented = cdp.webmcp.parents.get("CHILD").cloned();
            let revoked = cdp.webmcp.arms.get(&holder).is_some_and(|arm| arm.revoked);
            let notices = cdp.take_notices().join("|");
            failed.push(format!(
                "{label} admitted={admitted:?} evals={} blocked={} revoked={revoked} parents={parented:?} notices={notices}",
                shim_evals(&state, "CHILD"),
                cdp.session_blocked(&holder),
            ));
        }
    }
    assert!(failed.is_empty(), "failing start orderings {}/{}:\n{}", failed.len(), total, failed.join("\n"));
}

fn cross_tree() -> Value {
    json!({"frameTree":{"frame":{"id":"FRAME","url":"https://foreign.test/cross","securityOrigin":"https://foreign.test","loaderId":"CROSS"}}})
}

fn reconcile_holder(cdp: &mut Cdp) -> String {
    cdp.start_webmcp_for("H", true).unwrap();
    let holder = cdp.webmcp_session("H").to_string();
    if let Some(arm) = cdp.webmcp.arms.get_mut(&holder) {
        arm.pending_contexts.clear();
        arm.pending_frames.clear();
        arm.pending_install = None;
    }
    cdp.webmcp_request_origins(&["https://foreign.test".into()]);
    holder
}

fn child_attach(holder: &str, session: &str) -> Value {
    json!({"method":"Target.attachedToTarget","sessionId":holder,"params":{"sessionId":session,"targetInfo":{"targetId":"IFRAME","type":"iframe","url":"https://foreign.test/cross","parentFrameId":"F"},"waitingForDebugger":false}})
}

#[test]
fn reconcile_adopts_a_child_whose_attach_was_recorded_before_the_root_arm_existed() {
    let (mut cdp, state) = client();
    state.borrow_mut().oopif_tree = Some(cross_tree());
    cdp.absorb(child_attach("H", "CHILD"));
    assert!(!cdp.webmcp.parents.contains_key("CHILD"));
    let holder = reconcile_holder(&mut cdp);
    assert!(!cdp.webmcp.parents.contains_key("CHILD"), "recording alone never adopts");
    cdp.webmcp_auto_attach(&holder).unwrap();
    cdp.webmcp_install_children();
    assert_eq!(cdp.webmcp.parents.get("CHILD").map(String::as_str), Some(holder.as_str()));
    assert_eq!(cdp.webmcp.arms.get("CHILD").map(|arm| arm.admitted), Some(1));
    assert_eq!(shim_evals(&state, "CHILD"), 1);
    assert!(!state.borrow().sent.iter().any(|command| command["method"] == "Target.attachToTarget" && command["params"]["targetId"] == "IFRAME"));
    assert!(cdp.webmcp_generations().iter().any(|generation| generation["session"] == "CHILD"));
}

#[test]
fn reconcile_adopts_a_child_whose_attach_never_arrived() {
    let (mut cdp, state) = client();
    state.borrow_mut().oopif_tree = Some(cross_tree());
    cdp.webmcp.attach_grace = Some(Duration::ZERO);
    let holder = reconcile_holder(&mut cdp);
    cdp.webmcp_auto_attach(&holder).unwrap();
    cdp.webmcp_install_children();
    assert!(state.borrow().sent.iter().any(|command| command["method"] == "Target.attachToTarget" && command["params"]["targetId"] == "IFRAME" && command["params"]["flatten"] == true));
    assert_eq!(cdp.webmcp.parents.get("TOUCH-IFRAME").map(String::as_str), Some(holder.as_str()));
    assert_eq!(cdp.webmcp.arms.get("TOUCH-IFRAME").map(|arm| arm.admitted), Some(1));
    assert_eq!(shim_evals(&state, "TOUCH-IFRAME"), 1);
    cdp.webmcp_reconcile();
    cdp.webmcp_reconcile();
    assert_eq!(state.borrow().sent.iter().filter(|command| command["method"] == "Target.attachToTarget" && command["params"]["targetId"] == "IFRAME").count(), 1, "adopt once per target and root loader");
    assert_eq!(shim_evals(&state, "TOUCH-IFRAME"), 1);
}

#[test]
fn reconcile_refuses_a_wrong_parent_frame_even_when_the_child_tree_reports_the_root_frame() {
    let (mut cdp, state) = client();
    state.borrow_mut().iframe_targets = Some(vec![json!({"targetId":"IFRAME","type":"iframe","url":"https://foreign.test/cross","parentFrameId":"OTHER"})]);
    cdp.webmcp.attach_grace = Some(Duration::ZERO);
    let holder = reconcile_holder(&mut cdp);
    cdp.webmcp_auto_attach(&holder).unwrap();
    cdp.absorb(child_attach(&holder, "CHILD"));
    cdp.webmcp_install_children();
    assert!(!cdp.webmcp.parents.contains_key("CHILD") && !cdp.webmcp.arms.contains_key("CHILD"));
    assert!(!state.borrow().sent.iter().any(|command| command["sessionId"] == "CHILD" && matches!(command["method"].as_str(), Some("Runtime.enable" | "Runtime.evaluate" | "Page.enable"))));
    assert!(state.borrow().sent.iter().any(|command| command["method"] == "Target.detachFromTarget" && command["params"]["sessionId"] == "CHILD"));
    assert!(!state.borrow().sent.iter().any(|command| command["method"] == "Target.attachToTarget" && command["params"]["targetId"] == "IFRAME"));
    assert!(cdp.webmcp.arms.get(&holder).is_some_and(|arm| !arm.revoked));
}

#[test]
fn reconcile_skips_a_protected_sibling_without_protecting_the_foreign_child() {
    let (mut cdp, state) = client();
    state.borrow_mut().oopif_tree = Some(cross_tree());
    state.borrow_mut().iframe_targets = Some(vec![
        json!({"targetId":"SEVEN","type":"iframe","url":"https://seven.sliccy.ai/","parentFrameId":"F"}),
        json!({"targetId":"IFRAME","type":"iframe","url":"https://foreign.test/cross","parentFrameId":"F"}),
    ]);
    cdp.webmcp.attach_grace = Some(Duration::ZERO);
    let holder = reconcile_holder(&mut cdp);
    cdp.absorb(child_attach(&holder, "CHILD"));
    cdp.webmcp_auto_attach(&holder).unwrap();
    cdp.webmcp_install_children();
    assert!(!state.borrow().sent.iter().any(|command| command["method"] == "Target.attachToTarget" && command["params"]["targetId"] == "SEVEN"));
    assert!(!cdp.protected_targets.contains("IFRAME"));
    assert!(!cdp.protected_targets.contains("SEVEN"), "listing never protects");
    assert_eq!(cdp.webmcp.arms.get("CHILD").map(|arm| arm.admitted), Some(1));
    assert_eq!(shim_evals(&state, "CHILD"), 1);
    assert!(cdp.webmcp.arms.get(&holder).is_some_and(|arm| !arm.revoked));
}
