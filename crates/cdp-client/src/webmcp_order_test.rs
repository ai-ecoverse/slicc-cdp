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
fn iframe_attach_without_a_holder_is_never_consumed_and_protected_iframes_stay_blocked() {
    let (mut cdp, _) = client();
    cdp.absorb(json!({"method":"Target.attachedToTarget","params":{"sessionId":"CHILD","targetInfo":{"targetId":"SEVEN","type":"iframe","url":"https://seven.sliccy.ai/app","parentFrameId":"F"},"waitingForDebugger":false}}));
    assert!(cdp.webmcp.recorded.is_empty());
    assert!(cdp.protected_targets.contains("SEVEN"));
    cdp.absorb(json!({"method":"Target.attachedToTarget","params":{"sessionId":"ORDINARY","targetInfo":{"targetId":"IFRAME","type":"iframe","url":"https://foreign.test/cross","parentFrameId":"F"},"waitingForDebugger":false}}));
    assert!(cdp.webmcp.recorded.is_empty());
    assert!(!cdp.protected_targets.contains("IFRAME"));
}

#[test]
fn holder_does_not_consume_protected_or_extension_iframe_attaches() {
    let (mut cdp, _) = client();
    let holder = reconcile_holder(&mut cdp);
    cdp.webmcp_auto_attach(&holder).unwrap();
    cdp.absorb(json!({"method":"Target.attachedToTarget","sessionId":&holder,"params":{"sessionId":"SEVEN","targetInfo":{"targetId":"SEVEN","type":"iframe","url":"https://seven.sliccy.ai/app","parentFrameId":"F"},"waitingForDebugger":false}}));
    assert!(!cdp.webmcp.recorded.contains_key("SEVEN"));
    assert!(cdp.protected_targets.contains("SEVEN"));
    cdp.absorb(json!({"method":"Target.attachedToTarget","sessionId":&holder,"params":{"sessionId":"EXT","targetInfo":{"targetId":"EXT","type":"iframe","url":"chrome-extension://abcdefghijklmnop/page.html","parentFrameId":"F"},"waitingForDebugger":false}}));
    assert!(!cdp.webmcp.recorded.contains_key("EXT"));
}

#[test]
fn iframe_attach_on_a_non_arm_session_is_not_consumed() {
    let (mut cdp, _) = client();
    let holder = reconcile_holder(&mut cdp);
    cdp.webmcp_auto_attach(&holder).unwrap();
    cdp.absorb(json!({"method":"Target.attachedToTarget","sessionId":"OTHER","params":{"sessionId":"CHILD","targetInfo":{"targetId":"IFRAME","type":"iframe","url":"https://foreign.test/cross","parentFrameId":"F"},"waitingForDebugger":false}}));
    assert!(!cdp.webmcp.recorded.contains_key("IFRAME"));
    assert!(!cdp.webmcp.parents.contains_key("CHILD"));
}

#[test]
fn unowned_reattach_after_admit_does_not_retire_or_replace_the_live_child() {
    let (mut cdp, state) = client();
    state.borrow_mut().oopif_tree = Some(cross_tree());
    let holder = reconcile_holder(&mut cdp);
    cdp.webmcp_auto_attach(&holder).unwrap();
    cdp.absorb(child_attach(&holder, "CHILD"));
    cdp.webmcp_install_children();
    assert_eq!(cdp.webmcp.arms.get("CHILD").map(|arm| arm.admitted), Some(1));
    let recorded = cdp.webmcp.recorded.get("IFRAME").map(|recorded| recorded.session.clone());
    assert_eq!(recorded.as_deref(), Some("CHILD"));
    cdp.absorb(json!({"method":"Target.attachedToTarget","params":{"sessionId":"OTHER","targetInfo":{"targetId":"IFRAME","type":"iframe","url":"https://foreign.test/cross","parentFrameId":"F"},"waitingForDebugger":false}}));
    assert_eq!(cdp.webmcp.recorded.get("IFRAME").map(|recorded| recorded.session.as_str()), Some("CHILD"));
    assert_eq!(cdp.webmcp.parents.get("CHILD").map(String::as_str), Some(holder.as_str()));
    assert_eq!(cdp.webmcp.arms.get("CHILD").map(|arm| arm.admitted), Some(1));
    assert!(!cdp.session_blocked("CHILD"));
    assert!(!cdp.webmcp.parents.contains_key("OTHER"));
}

#[test]
fn reconcile_adopts_a_child_whose_attach_was_recorded_before_the_root_arm_existed() {
    let (mut cdp, state) = client();
    state.borrow_mut().oopif_tree = Some(cross_tree());
    cdp.webmcp.attach_grace = Some(Duration::ZERO);
    cdp.absorb(child_attach("H", "CHILD"));
    assert!(cdp.webmcp.recorded.is_empty(), "no holder means the attach is never consumed or recorded");
    assert!(!cdp.webmcp.parents.contains_key("CHILD"));
    let holder = reconcile_holder(&mut cdp);
    assert!(!cdp.webmcp.parents.contains_key("CHILD"), "a pre-arm attach never adopts");
    cdp.webmcp_auto_attach(&holder).unwrap();
    cdp.webmcp_install_children();
    assert!(state.borrow().sent.iter().any(|command| command["method"] == "Target.attachToTarget" && command["params"]["targetId"] == "IFRAME" && command["params"]["flatten"] == true));
    assert_eq!(cdp.webmcp.parents.get("TOUCH-IFRAME").map(String::as_str), Some(holder.as_str()));
    assert_eq!(cdp.webmcp.arms.get("TOUCH-IFRAME").map(|arm| arm.admitted), Some(1));
    assert_eq!(shim_evals(&state, "TOUCH-IFRAME"), 1);
    assert!(cdp.webmcp_generations().iter().any(|generation| generation["session"] == "TOUCH-IFRAME"));
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
    cdp.webmcp_auto_attach(&holder).unwrap();
    cdp.absorb(child_attach(&holder, "CHILD"));
    cdp.webmcp_install_children();
    assert!(!state.borrow().sent.iter().any(|command| command["method"] == "Target.attachToTarget" && command["params"]["targetId"] == "SEVEN"));
    assert!(!cdp.protected_targets.contains("IFRAME"));
    assert!(!cdp.protected_targets.contains("SEVEN"), "listing never protects");
    assert_eq!(cdp.webmcp.arms.get("CHILD").map(|arm| arm.admitted), Some(1));
    assert_eq!(shim_evals(&state, "CHILD"), 1);
    assert!(cdp.webmcp.arms.get(&holder).is_some_and(|arm| !arm.revoked));
}

fn child_enables(state: &Rc<RefCell<BackendState>>, session: &str) -> Vec<String> {
    state.borrow().sent.iter().filter(|command| {
        command["sessionId"] == session
            && matches!(command["method"].as_str(), Some("Network.enable" | "Runtime.enable" | "Page.enable"))
    }).filter_map(|command| command["method"].as_str().map(str::to_string)).collect()
}

#[test]
fn adopt_judges_seed_tree_before_any_child_domain_enable() {
    let (mut cdp, state) = client();
    state.borrow_mut().iframe_targets = Some(vec![json!({"targetId":"IFRAME","type":"iframe","url":"https://foreign.test/cross","parentFrameId":"F"})]);
    state.borrow_mut().oopif_tree = Some(json!({"frameTree":{"frame":{"id":"SEVEN","url":"https://seven.sliccy.ai/app","securityOrigin":"https://seven.sliccy.ai","loaderId":"P"}}}));
    let holder = reconcile_holder(&mut cdp);
    cdp.webmcp_auto_attach(&holder).unwrap();
    let before = state.borrow().sent.len();
    cdp.absorb(json!({"method":"Target.attachedToTarget","sessionId":&holder,"params":{"sessionId":"CHILD","targetInfo":{"targetId":"IFRAME","type":"iframe","url":"https://foreign.test/cross","parentFrameId":"F"},"waitingForDebugger":false}}));
    assert!(child_enables(&state, "CHILD").is_empty(), "protected seed must never receive domain enables: {:?}", child_enables(&state, "CHILD"));
    assert!(!state.borrow().sent[before..].iter().any(|command| command["sessionId"] == "CHILD" && command["method"] == "Runtime.evaluate"));
    assert!(cdp.session_blocked("CHILD"));
    assert!(!cdp.webmcp.parents.contains_key("CHILD") || cdp.webmcp.arms.get("CHILD").is_none_or(|arm| arm.revoked));
    assert!(cdp.protected_targets.contains("IFRAME"));
    assert!(state.borrow().sent.iter().any(|command| command["method"] == "Target.detachFromTarget" && command["params"]["sessionId"] == "CHILD"));
}

#[test]
fn adopt_settles_chrome_error_seed_without_enabling_domains() {
    let (mut cdp, state) = client();
    state.borrow_mut().oopif_tree = Some(json!({"frameTree":{"frame":{"id":"ERR","url":"chrome-error://chromewebdata/","securityOrigin":"://","loaderId":"E","unreachableUrl":"https://foreign.test/cross"}}}));
    let holder = reconcile_holder(&mut cdp);
    cdp.webmcp_auto_attach(&holder).unwrap();
    cdp.absorb(json!({"method":"Target.attachedToTarget","sessionId":&holder,"params":{"sessionId":"CHILD","targetInfo":{"targetId":"IFRAME","type":"iframe","url":"https://foreign.test/cross","parentFrameId":"F"},"waitingForDebugger":false}}));
    assert!(child_enables(&state, "CHILD").is_empty(), "chrome-error seed must never receive domain enables: {:?}", child_enables(&state, "CHILD"));
    assert!(cdp.session_blocked("CHILD"));
    assert!(state.borrow().sent.iter().any(|command| command["method"] == "Target.detachFromTarget" && command["params"]["sessionId"] == "CHILD"));
}

#[test]
fn provisional_child_arms_without_domain_enables_until_verified() {
    let (mut cdp, state) = client();
    state.borrow_mut().oopif_tree = Some(json!({"frameTree":{"frame":{"id":"FRAME","url":"about:blank","securityOrigin":"","loaderId":""}}}));
    let holder = reconcile_holder(&mut cdp);
    cdp.webmcp_auto_attach(&holder).unwrap();
    cdp.absorb(json!({"method":"Target.attachedToTarget","sessionId":&holder,"params":{"sessionId":"CHILD","targetInfo":{"targetId":"IFRAME","type":"iframe","url":"about:blank","parentFrameId":"F"},"waitingForDebugger":false}}));
    assert!(cdp.webmcp.arms.get("CHILD").is_some_and(|arm| !arm.revoked && !arm.domains && arm.pending_install.is_some()));
    assert!(child_enables(&state, "CHILD").is_empty(), "provisional arm must not enable domains: {:?}", child_enables(&state, "CHILD"));
    state.borrow_mut().oopif_tree = Some(cross_tree());
    cdp.absorb(json!({"method":"Page.frameNavigated","sessionId":"CHILD","params":{"frame":{"id":"FRAME","url":"https://foreign.test/cross","securityOrigin":"https://foreign.test","loaderId":"CROSS"}}}));
    cdp.absorb(json!({"method":"Runtime.executionContextCreated","sessionId":"CHILD","params":{"context":{"id":9,"uniqueId":"CROSS-CTX","origin":"https://foreign.test","auxData":{"frameId":"FRAME","isDefault":true}}}}));
    assert!(cdp.webmcp.arms.get("CHILD").is_some_and(|arm| arm.domains && arm.admitted > 0));
    assert!(!child_enables(&state, "CHILD").is_empty());
    assert_eq!(shim_evals(&state, "CHILD"), 1);
}

#[test]
fn provisional_child_promotes_on_target_info_commit_without_lifecycle_events() {
    let (mut cdp, state) = client();
    state.borrow_mut().chrome_shaped_lifecycle = true;
    state.borrow_mut().oopif_tree = Some(json!({"frameTree":{"frame":{"id":"FRAME","url":"about:blank","securityOrigin":"","loaderId":""}}}));
    let holder = reconcile_holder(&mut cdp);
    cdp.webmcp_auto_attach(&holder).unwrap();
    cdp.absorb(json!({"method":"Target.attachedToTarget","sessionId":&holder,"params":{"sessionId":"CHILD","targetInfo":{"targetId":"IFRAME","type":"iframe","url":"","parentFrameId":"F"},"waitingForDebugger":false}}));
    assert!(cdp.webmcp.arms.get("CHILD").is_some_and(|arm| !arm.revoked && !arm.domains && arm.pending_install.is_some()));
    assert!(child_enables(&state, "CHILD").is_empty());
    let before = state.borrow().sent.len();
    state.borrow_mut().oopif_tree = Some(cross_tree());
    cdp.absorb(json!({"method":"Target.targetInfoChanged","params":{"targetInfo":{"targetId":"IFRAME","type":"iframe","url":"https://foreign.test/cross","parentFrameId":"F"}}}));
    assert!(cdp.webmcp.arms.get("CHILD").is_some_and(|arm| arm.domains && arm.admitted > 0), "targetInfo commit must promote a provisional park");
    assert_eq!(shim_evals(&state, "CHILD"), 1);
    let child_sent = state.borrow().sent[before..].iter().filter(|command| command["sessionId"] == "CHILD").cloned().collect::<Vec<_>>();
    let tree = child_sent.iter().position(|command| command["method"] == "Page.getFrameTree");
    let enable = child_sent.iter().position(|command| matches!(command["method"].as_str(), Some("Network.enable" | "Runtime.enable" | "Page.enable")));
    assert!(tree.is_some_and(|tree| enable.is_some_and(|enable| tree < enable)), "no domain enable before the committed tree is read: {child_sent:?}");
}

#[test]
fn provisional_already_adopted_child_promotes_only_via_promote_path() {
    let (mut cdp, state) = client();
    state.borrow_mut().chrome_shaped_lifecycle = true;
    state.borrow_mut().oopif_tree = Some(json!({"frameTree":{"frame":{"id":"FRAME","url":"about:blank","securityOrigin":"","loaderId":""}}}));
    let holder = reconcile_holder(&mut cdp);
    cdp.webmcp_auto_attach(&holder).unwrap();
    cdp.absorb(json!({"method":"Target.attachedToTarget","sessionId":&holder,"params":{"sessionId":"CHILD","targetInfo":{"targetId":"IFRAME","type":"iframe","url":"","parentFrameId":"F"},"waitingForDebugger":false}}));
    assert!(cdp.webmcp.adopted.values().any(|(owner, child)| owner == &holder && child == "CHILD"), "child must already be adopted so reconcile_once skips it");
    assert!(cdp.webmcp.arms.get("CHILD").is_some_and(|arm| !arm.domains && arm.admitted == 0 && arm.pending_install.is_some()));
    assert!(child_enables(&state, "CHILD").is_empty());
    if let Some(arm) = cdp.webmcp.arms.get_mut("CHILD") {
        arm.promote_read_at = None;
    }
    state.borrow_mut().oopif_tree = Some(cross_tree());
    let before = state.borrow().sent.len();
    cdp.webmcp_reconcile();
    assert!(cdp.webmcp.arms.get("CHILD").is_some_and(|arm| arm.domains && arm.admitted > 0), "only webmcp_promote_provisional_children can adopt an already-recorded provisional child");
    assert_eq!(shim_evals(&state, "CHILD"), 1);
    let child_sent = state.borrow().sent[before..].iter().filter(|command| command["sessionId"] == "CHILD").cloned().collect::<Vec<_>>();
    let tree = child_sent.iter().position(|command| command["method"] == "Page.getFrameTree");
    let enable = child_sent.iter().position(|command| matches!(command["method"].as_str(), Some("Network.enable" | "Runtime.enable" | "Page.enable")));
    assert!(tree.is_some_and(|tree| enable.is_some_and(|enable| tree < enable)), "Chrome-shaped promote must verify before enabling: {child_sent:?}");
}

#[test]
fn provisional_child_that_commits_to_a_protected_origin_is_refused() {
    let (mut cdp, state) = client();
    state.borrow_mut().chrome_shaped_lifecycle = true;
    state.borrow_mut().oopif_tree = Some(json!({"frameTree":{"frame":{"id":"FRAME","url":"about:blank","securityOrigin":"","loaderId":""}}}));
    let holder = reconcile_holder(&mut cdp);
    cdp.webmcp_auto_attach(&holder).unwrap();
    cdp.absorb(json!({"method":"Target.attachedToTarget","sessionId":&holder,"params":{"sessionId":"CHILD","targetInfo":{"targetId":"IFRAME","type":"iframe","url":"","parentFrameId":"F"},"waitingForDebugger":false}}));
    assert!(cdp.webmcp.arms.get("CHILD").is_some_and(|arm| !arm.revoked && !arm.domains));
    state.borrow_mut().oopif_tree = Some(json!({"frameTree":{"frame":{"id":"SEVEN","url":"https://seven.sliccy.ai/app","securityOrigin":"https://seven.sliccy.ai","loaderId":"P"}}}));
    cdp.absorb(json!({"method":"Target.targetInfoChanged","params":{"targetInfo":{"targetId":"IFRAME","type":"iframe","url":"https://seven.sliccy.ai/app","parentFrameId":"F"}}}));
    assert!(child_enables(&state, "CHILD").is_empty(), "protected commit must never enable domains: {:?}", child_enables(&state, "CHILD"));
    assert!(cdp.session_blocked("CHILD"));
    assert!(cdp.protected_targets.contains("IFRAME"));
    assert!(state.borrow().sent.iter().any(|command| command["method"] == "Target.detachFromTarget" && command["params"]["sessionId"] == "CHILD"));
    assert_eq!(shim_evals(&state, "CHILD"), 0);
}

#[test]
fn parked_unverified_non_provisional_child_gets_a_deadline_and_settles() {
    let (mut cdp, state) = client();
    state.borrow_mut().iframe_targets = Some(vec![json!({"targetId":"IFRAME","type":"iframe","url":"https://foreign.test/cross","parentFrameId":"F"})]);
    state.borrow_mut().oopif_tree = Some(json!({"frameTree":{"frame":{"id":"FRAME","url":"https://foreign.test/cross","securityOrigin":"","loaderId":""}}}));
    let holder = reconcile_holder(&mut cdp);
    cdp.webmcp_auto_attach(&holder).unwrap();
    cdp.absorb(json!({"method":"Target.attachedToTarget","sessionId":&holder,"params":{"sessionId":"CHILD","targetInfo":{"targetId":"IFRAME","type":"iframe","url":"https://foreign.test/cross","parentFrameId":"F"},"waitingForDebugger":false}}));
    assert!(child_enables(&state, "CHILD").is_empty(), "unverified non-provisional must not enable before verification: {:?}", child_enables(&state, "CHILD"));
    assert!(cdp.webmcp.arms.get("CHILD").is_some_and(|arm| !arm.revoked && !arm.domains && arm.pending_install.is_some()));
    if let Some(arm) = cdp.webmcp.arms.get_mut("CHILD") {
        arm.pending_install = Some(Instant::now() - Duration::from_secs(1));
    }
    cdp.webmcp_sweep();
    assert!(cdp.session_blocked("CHILD"), "deadline must settle a parked unverified child");
}

#[test]
fn provisional_child_commit_after_three_seconds_still_promotes() {
    let (mut cdp, state) = client();
    state.borrow_mut().chrome_shaped_lifecycle = true;
    state.borrow_mut().oopif_tree = Some(json!({"frameTree":{"frame":{"id":"FRAME","url":"about:blank","securityOrigin":"","loaderId":""}}}));
    let holder = reconcile_holder(&mut cdp);
    cdp.webmcp_auto_attach(&holder).unwrap();
    cdp.absorb(json!({"method":"Target.attachedToTarget","sessionId":&holder,"params":{"sessionId":"CHILD","targetInfo":{"targetId":"IFRAME","type":"iframe","url":"","parentFrameId":"F"},"waitingForDebugger":false}}));
    let due = cdp.webmcp.arms.get("CHILD").and_then(|arm| arm.pending_install).expect("provisional park deadline");
    assert!(due >= Instant::now() + Duration::from_secs(8), "PROVISIONAL_PARK is 10s so a slow iframe past DOCUMENT_CHECK(2s) still has room");
    if let Some(arm) = cdp.webmcp.arms.get_mut("CHILD") {
        arm.pending_install = Some(Instant::now() + Duration::from_secs(7));
        arm.promote_read_at = None;
    }
    state.borrow_mut().oopif_tree = Some(cross_tree());
    cdp.webmcp_sweep();
    assert!(cdp.webmcp.arms.get("CHILD").is_some_and(|arm| arm.domains && arm.admitted > 0), "a commit ~3s after attach must still promote inside the 10s park");
    assert_eq!(shim_evals(&state, "CHILD"), 1);
}

#[test]
fn provisional_child_deadline_settles_without_tools_when_still_blank() {
    let (mut cdp, state) = client();
    state.borrow_mut().chrome_shaped_lifecycle = true;
    state.borrow_mut().oopif_tree = Some(json!({"frameTree":{"frame":{"id":"FRAME","url":"about:blank","securityOrigin":"","loaderId":""}}}));
    let holder = reconcile_holder(&mut cdp);
    cdp.webmcp_auto_attach(&holder).unwrap();
    cdp.absorb(json!({"method":"Target.attachedToTarget","sessionId":&holder,"params":{"sessionId":"CHILD","targetInfo":{"targetId":"IFRAME","type":"iframe","url":"","parentFrameId":"F"},"waitingForDebugger":false}}));
    let due = cdp.webmcp.arms.get("CHILD").and_then(|arm| arm.pending_install).expect("provisional park deadline");
    assert!(due >= Instant::now() + Duration::from_secs(8), "provisional park uses PROVISIONAL_PARK (10s), not DOCUMENT_CHECK (2s)");
    if let Some(arm) = cdp.webmcp.arms.get_mut("CHILD") {
        arm.pending_install = Some(Instant::now() - Duration::from_secs(1));
        arm.promote_read_at = None;
    }
    state.borrow_mut().oopif_tree = Some(cross_tree());
    let before = state.borrow().sent.len();
    cdp.webmcp_sweep();
    assert!(cdp.session_blocked("CHILD"), "after the provisional deadline the park settles to no tools");
    assert!(child_enables(&state, "CHILD").is_empty(), "expired provisional must not enable even if the tree commits late: {:?}", child_enables(&state, "CHILD"));
    assert_eq!(shim_evals(&state, "CHILD"), 0);
    assert!(!state.borrow().sent[before..].iter().any(|command| command["sessionId"] == "CHILD" && matches!(command["method"].as_str(), Some("Network.enable" | "Runtime.enable" | "Page.enable" | "Runtime.evaluate"))));
}

#[test]
fn provisional_child_frame_tree_reread_is_rate_limited() {
    let (mut cdp, state) = client();
    state.borrow_mut().oopif_tree = Some(json!({"frameTree":{"frame":{"id":"FRAME","url":"about:blank","securityOrigin":"","loaderId":""}}}));
    let holder = reconcile_holder(&mut cdp);
    cdp.webmcp_auto_attach(&holder).unwrap();
    cdp.absorb(json!({"method":"Target.attachedToTarget","sessionId":&holder,"params":{"sessionId":"CHILD","targetInfo":{"targetId":"IFRAME","type":"iframe","url":"","parentFrameId":"F"},"waitingForDebugger":false}}));
    let trees = |state: &Rc<RefCell<BackendState>>| state.borrow().sent.iter().filter(|command| command["sessionId"] == "CHILD" && command["method"] == "Page.getFrameTree").count();
    let after_attach = trees(&state);
    if let Some(arm) = cdp.webmcp.arms.get_mut("CHILD") {
        arm.promote_read_at = Some(Instant::now());
    }
    cdp.webmcp_sweep();
    cdp.webmcp_sweep();
    assert_eq!(trees(&state), after_attach, "sweeps inside PROVISIONAL_READ must not re-issue Page.getFrameTree");
    if let Some(arm) = cdp.webmcp.arms.get_mut("CHILD") {
        arm.promote_read_at = Some(Instant::now() - Duration::from_millis(300));
    }
    cdp.webmcp_sweep();
    assert_eq!(trees(&state), after_attach + 1, "a re-read is allowed once the 250ms gate elapses");
}

#[test]
fn stop_webmcp_detaches_a_provisional_child_before_the_next_sweep() {
    let (mut cdp, state) = client();
    state.borrow_mut().chrome_shaped_lifecycle = true;
    state.borrow_mut().oopif_tree = Some(json!({"frameTree":{"frame":{"id":"FRAME","url":"about:blank","securityOrigin":"","loaderId":""}}}));
    let holder = reconcile_holder(&mut cdp);
    cdp.webmcp_auto_attach(&holder).unwrap();
    cdp.absorb(json!({"method":"Target.attachedToTarget","sessionId":&holder,"params":{"sessionId":"CHILD","targetInfo":{"targetId":"IFRAME","type":"iframe","url":"","parentFrameId":"F"},"waitingForDebugger":false}}));
    assert!(cdp.webmcp.arms.get("CHILD").is_some_and(|arm| !arm.domains && arm.pending_install.is_some()));
    let before = state.borrow().sent.len();
    cdp.stop_webmcp(&holder);
    assert!(cdp.session_blocked("CHILD"), "stop must detach and block a provisional park");
    assert!(state.borrow().sent[before..].iter().any(|command| command["method"] == "Target.detachFromTarget" && command["params"]["sessionId"] == "CHILD"));
    assert!(child_enables(&state, "CHILD").is_empty());
    state.borrow_mut().oopif_tree = Some(cross_tree());
    if let Some(arm) = cdp.webmcp.arms.get_mut("CHILD") {
        arm.promote_read_at = None;
    }
    cdp.webmcp_sweep();
    assert!(cdp.session_blocked("CHILD"));
    assert!(child_enables(&state, "CHILD").is_empty(), "post-stop sweep must not enable a provisional child: {:?}", child_enables(&state, "CHILD"));
    assert_eq!(shim_evals(&state, "CHILD"), 0);
    assert!(cdp.webmcp.arms.get("CHILD").is_none_or(|arm| arm.revoked || !arm.domains));
}

#[test]
fn provisional_get_frame_tree_reply_after_deadline_settles_without_tools() {
    let (mut cdp, state) = client();
    state.borrow_mut().chrome_shaped_lifecycle = true;
    state.borrow_mut().child_get_frame_tree_delay = Some(Duration::from_millis(120));
    state.borrow_mut().oopif_tree = Some(json!({"frameTree":{"frame":{"id":"FRAME","url":"about:blank","securityOrigin":"","loaderId":""}}}));
    let holder = reconcile_holder(&mut cdp);
    cdp.webmcp_auto_attach(&holder).unwrap();
    cdp.absorb(json!({"method":"Target.attachedToTarget","sessionId":&holder,"params":{"sessionId":"CHILD","targetInfo":{"targetId":"IFRAME","type":"iframe","url":"","parentFrameId":"F"},"waitingForDebugger":false}}));
    if let Some(arm) = cdp.webmcp.arms.get_mut("CHILD") {
        arm.pending_install = Some(Instant::now() + Duration::from_millis(40));
        arm.promote_read_at = None;
    }
    state.borrow_mut().oopif_tree = Some(cross_tree());
    cdp.webmcp_sweep();
    assert!(cdp.session_blocked("CHILD"), "an in-flight getFrameTree that returns after the deadline must settle");
    assert!(child_enables(&state, "CHILD").is_empty(), "late reply must not enable domains: {:?}", child_enables(&state, "CHILD"));
    assert_eq!(shim_evals(&state, "CHILD"), 0);
    assert!(cdp.webmcp.arms.get("CHILD").is_none_or(|arm| arm.admitted == 0));
}

#[test]
fn stop_or_parent_revoke_during_child_enable_settles_an_enabling_child() {
    for stop in [false, true] {
        let (mut cdp, state) = client();
        state.borrow_mut().chrome_shaped_lifecycle = true;
        state.borrow_mut().oopif_tree = Some(json!({"frameTree":{"frame":{"id":"FRAME","url":"about:blank","securityOrigin":"","loaderId":""}}}));
        let holder = reconcile_holder(&mut cdp);
        cdp.webmcp_auto_attach(&holder).unwrap();
        cdp.absorb(json!({"method":"Target.attachedToTarget","sessionId":&holder,"params":{"sessionId":"CHILD","targetInfo":{"targetId":"IFRAME","type":"iframe","url":"","parentFrameId":"F"},"waitingForDebugger":false}}));
        if let Some(arm) = cdp.webmcp.arms.get_mut("CHILD") {
            arm.promote_read_at = None;
        }
        state.borrow_mut().oopif_tree = Some(cross_tree());
        if stop {
            let cdp_ptr = Rc::new(RefCell::new(std::ptr::null_mut::<Cdp>()));
            let hook = Rc::new(RefCell::new(None));
            {
                let cdp_ptr = cdp_ptr.clone();
                let holder = holder.clone();
                *hook.borrow_mut() = Some(Box::new(move || {
                    let ptr = *cdp_ptr.borrow();
                    if !ptr.is_null() {
                        unsafe { (*ptr).stop_webmcp(&holder); }
                    }
                }) as Box<dyn FnMut()>);
            }
            state.borrow_mut().child_network_enable_hook = Some(hook);
            *cdp_ptr.borrow_mut() = &mut cdp as *mut Cdp;
        } else {
            state.borrow_mut().inject_before_child_network_enable = Some(json!({
                "method":"Page.frameNavigated",
                "sessionId":holder,
                "params":{"frame":{"id":"F","url":"https://catalog.test/vault","securityOrigin":"https://catalog.test","loaderId":"PROTECTED"}}
            }));
        }
        cdp.webmcp_sweep();
        assert!(cdp.session_blocked("CHILD"), "stop={stop}: enabling child must settle");
        assert!(state.borrow().sent.iter().any(|command| command["method"] == "Target.detachFromTarget" && command["params"]["sessionId"] == "CHILD"), "stop={stop}");
        assert_eq!(shim_evals(&state, "CHILD"), 0, "stop={stop}");
        assert!(cdp.webmcp.arms.get("CHILD").is_none_or(|arm| arm.admitted == 0 && !arm.domains), "stop={stop}");
    }
}

#[test]
fn adopt_get_frame_tree_reply_after_deadline_settles_without_tools() {
    let (mut cdp, state) = client();
    state.borrow_mut().chrome_shaped_lifecycle = true;
    state.borrow_mut().child_get_frame_tree_nth_delay = Some((3, Duration::from_millis(200)));
    state.borrow_mut().oopif_tree = Some(json!({"frameTree":{"frame":{"id":"FRAME","url":"about:blank","securityOrigin":"","loaderId":""}}}));
    let holder = reconcile_holder(&mut cdp);
    cdp.webmcp_auto_attach(&holder).unwrap();
    cdp.absorb(json!({"method":"Target.attachedToTarget","sessionId":&holder,"params":{"sessionId":"CHILD","targetInfo":{"targetId":"IFRAME","type":"iframe","url":"","parentFrameId":"F"},"waitingForDebugger":false}}));
    if let Some(arm) = cdp.webmcp.arms.get_mut("CHILD") {
        arm.pending_install = Some(Instant::now() + Duration::from_millis(80));
        arm.promote_read_at = None;
    }
    state.borrow_mut().oopif_tree = Some(cross_tree());
    cdp.webmcp_sweep();
    assert!(cdp.session_blocked("CHILD"), "adopt's late getFrameTree must settle after the deadline");
    assert!(state.borrow().sent.iter().any(|command| command["method"] == "Target.detachFromTarget" && command["params"]["sessionId"] == "CHILD"));
    assert_eq!(shim_evals(&state, "CHILD"), 0);
    assert!(cdp.webmcp.arms.get("CHILD").is_none_or(|arm| arm.admitted == 0 && !arm.domains));
}

#[test]
fn runtime_enable_reentry_does_not_evaluate_before_page_enable() {
    let (mut cdp, state) = client();
    state.borrow_mut().chrome_shaped_lifecycle = true;
    state.borrow_mut().fail_child_page_enable = true;
    state.borrow_mut().oopif_tree = Some(json!({"frameTree":{"frame":{"id":"FRAME","url":"about:blank","securityOrigin":"","loaderId":""}}}));
    let holder = reconcile_holder(&mut cdp);
    cdp.webmcp_auto_attach(&holder).unwrap();
    cdp.absorb(json!({"method":"Target.attachedToTarget","sessionId":&holder,"params":{"sessionId":"CHILD","targetInfo":{"targetId":"IFRAME","type":"iframe","url":"","parentFrameId":"F"},"waitingForDebugger":false}}));
    if let Some(arm) = cdp.webmcp.arms.get_mut("CHILD") {
        arm.promote_read_at = None;
    }
    state.borrow_mut().oopif_tree = Some(cross_tree());
    cdp.webmcp_sweep();
    let child_sent = state.borrow().sent.iter().filter(|command| command["sessionId"] == "CHILD").cloned().collect::<Vec<_>>();
    let evaluate = child_sent.iter().position(|command| {
        command["method"] == "Runtime.evaluate" && command["params"]["expression"].as_str().is_some_and(webmcp_shim_install)
    });
    let page = child_sent.iter().position(|command| command["method"] == "Page.enable");
    assert!(evaluate.is_none(), "nested adopt during Runtime.enable must not evaluate the shim: {child_sent:?}");
    assert!(page.is_some(), "outer enable must still reach Page.enable: {child_sent:?}");
    assert_eq!(shim_evals(&state, "CHILD"), 0);
    assert!(cdp.session_blocked("CHILD"));
}
