fn spelling_row(origin: &str, want: bool) {
    use crate::protected::{about_lookalike, Opener, Protected};
    let vectors = slicc_browsers::protected::vectors();
    let rows: Vec<_> = vectors
        .targets
        .iter()
        .filter(|vector| {
            vector.context_origin.as_deref() == Some(origin)
                && vector.url == "about:blank"
                && vector.opener.is_none()
                && !vector.previous
                && vector.kind.is_none()
                && vector.document_url.is_none()
                && vector.unreachable_url.is_none()
                && !vector.navigating
        })
        .collect();
    assert_eq!(rows.len(), 1, "{origin:?}");
    assert_eq!(rows[0].protected, want, "{origin:?}");
    let origins: Vec<&str> = vectors.origins.iter().map(String::as_str).collect();
    for guard in [Protected::new(&origins), Protected::default()] {
        assert_eq!(guard.target_with_document("about:blank", false, Opener::None, None, None, false, Some(origin)), want, "{origin:?}");
    }
    let created = json!({"method":"Runtime.executionContextCreated","sessionId":"S","params":{"context":{"id":1,"origin":origin,"name":"","uniqueId":"U","auxData":{"isDefault":true,"type":"default","frameId":"F"}}}}).to_string();
    let received = serde_json::from_str::<Value>(&super::opaque_blank_origin(created)).unwrap();
    let seen = received.pointer("/params/context/origin").and_then(Value::as_str).unwrap().to_string();
    assert_eq!(seen, if origin == "about://" { "://" } else { origin }, "{origin:?}");
    assert!(super::default_frame_context(received.pointer("/params/context")), "{origin:?}");
    assert_eq!(about_lookalike(&seen), want, "{origin:?}");
    assert_eq!(default_context_marks(origin, json!({"isDefault":true,"type":"default","frameId":"F"})), want, "{origin:?}");
}

macro_rules! spelling_rows {
    ($($name:ident: $origin:expr => $want:expr;)*) => {
        $(
            #[test]
            fn $name() {
                spelling_row($origin, $want);
            }
        )*
    };
}

spelling_rows! {
    row_01_protected: "AbOuT:blank" => true;
    row_02_protected: "about:" => true;
    row_03_protected: "abouT:" => true;
    row_04_protected: "about:\n" => true;
    row_05_protected: "about://\n" => true;
    row_06_protected: "about://\r" => true;
    row_07_protected: "about://\t" => true;
    row_08_protected: "about:blank " => true;
    row_09_protected: "about://x\u{0}" => true;
    row_10_protected: "about:%20" => true;
    row_11_protected: "ABOUT://" => true;
    row_12_unprotected: "" => false;
    row_13_unprotected: "null" => false;
    row_14_unprotected: "://" => false;
    row_15_unprotected: "about://" => false;
}
