#[test]
fn duplicate_caller_requests_do_not_replace_the_original_session() {
    let mut input = VecDeque::from([
        json!({"id":7,"method":"Page.navigate","sessionId":"S1","params":{"url":"https://ordinary.test/first"}}).to_string(),
        json!({"id":7,"method":"Page.navigate","sessionId":"S2","params":{"url":"https://ordinary.test/duplicate"}}).to_string(),
    ]);
    let mut dispatcher = Dispatcher { read:Box::new(move || input.pop_front().ok_or(mpsc::TryRecvError::Empty)), queue:VecDeque::new(), requests:BTreeSet::new(), closed:false, busy:true };
    dispatcher.ingest();
    assert_eq!(dispatcher.queue.len(), 1);
    assert_eq!(dispatcher.queue.front().unwrap()["sessionId"], "S1");
}

#[test]
fn duplicate_caller_requests_leave_inflight_work_and_completed_ids_owned() {
    let input = Rc::new(RefCell::new(VecDeque::from([json!({"id":7,"method":"Page.navigate","sessionId":"S1","params":{}}).to_string()])));
    let read = input.clone();
    let mut dispatcher = Dispatcher { read:Box::new(move || read.borrow_mut().pop_front().ok_or(mpsc::TryRecvError::Empty)), queue:VecDeque::new(), requests:BTreeSet::new(), closed:false, busy:true };
    dispatcher.ingest();
    let original = dispatcher.queue.pop_front().unwrap();
    let pending = BTreeMap::from([(7, (101, "S1"))]);
    let duplicate = json!({"id":7,"method":"Page.navigate","sessionId":"S2","params":{}});
    input.borrow_mut().extend([duplicate.to_string(), json!({"id":8,"method":"Page.navigate","sessionId":"S2","params":{}}).to_string()]);
    dispatcher.ingest();
    assert_eq!(dispatcher.queue.len(), 1);
    assert_eq!(dispatcher.queue.pop_front().unwrap()["id"], 8);
    assert_eq!(original["sessionId"], "S1");
    assert_eq!(pending[&7], (101, "S1"));
    let notice = rejected(&duplicate);
    assert!(notice.get("id").is_none());
    assert_eq!(notice["rejected"]["id"], 7);
    assert_eq!(notice["rejected"]["sessionId"], "S2");
    assert_eq!(notice["rejected"]["error"]["message"], "duplicate request id");
    dispatcher.busy = false;
    input.borrow_mut().push_back(duplicate.to_string());
    dispatcher.ingest();
    assert!(dispatcher.queue.is_empty());
}
