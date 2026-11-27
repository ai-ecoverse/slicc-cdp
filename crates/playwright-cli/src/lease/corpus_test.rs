use super::Call;
use serde_json::Value;
use std::collections::BTreeMap;

const CORPUS: &[u8] = include_bytes!("../../../../test/integration/lease-scope-v1.candidate.json");
const DIGEST: &str = "0fbec3e0add0af611c196a0ab5cb8918a933db243d7724cfe9df441889e01246";
const LENGTH: usize = 151818;
const SOURCE: &str = "1c98de71a01b0ad4cdf597c43e95b69e240bde9c";
const ALABEL: &str = "invalid-alabel-appears-ascii";

const K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

fn sha256(data: &[u8]) -> String {
    let mut state: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    let mut message = data.to_vec();
    message.push(0x80);
    while message.len() % 64 != 56 {
        message.push(0);
    }
    message.extend_from_slice(&((data.len() as u64) * 8).to_be_bytes());
    for block in message.chunks_exact(64) {
        let mut w = [0u32; 64];
        for (word, bytes) in w.iter_mut().zip(block.chunks_exact(4)) {
            *word = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let mut v = state;
        for (k, word) in K.iter().zip(w) {
            let [a, b, c, d, e, f, g, h] = v;
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ (!e & g);
            let t1 = h
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(*k)
                .wrapping_add(word);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            v = [t1.wrapping_add(t2), a, b, c, d.wrapping_add(t1), e, f, g];
        }
        for (x, y) in state.iter_mut().zip(v) {
            *x = x.wrapping_add(y);
        }
    }
    state.iter().map(|x| format!("{x:08x}")).collect()
}

#[derive(Clone)]
enum Outcome {
    NotApplicable,
    Accepted(String),
    Denied(String),
}

impl Outcome {
    fn label(&self) -> String {
        match self {
            Self::NotApplicable => "n/a".into(),
            Self::Accepted(detail) => detail.clone(),
            Self::Denied(variant) => variant.clone(),
        }
    }

    fn accepted(&self) -> Option<bool> {
        match self {
            Self::NotApplicable => None,
            Self::Accepted(_) => Some(true),
            Self::Denied(_) => Some(false),
        }
    }
}

fn surface(args: &[&str], kind: bool) -> Outcome {
    let argv: Vec<String> = args.iter().map(|arg| arg.to_string()).collect();
    match Call::parse(&argv) {
        Ok(call) if kind => Outcome::Accepted(format!(
            "Ok(kind={})",
            call.params["allow"][0]["kind"].as_str().unwrap_or("none")
        )),
        Ok(_) => Outcome::Accepted("Ok".into()),
        Err(err) => Outcome::Denied(format!("{err:?}")),
    }
}

fn flag(row: &Value, pointer: &str) -> bool {
    row.pointer(pointer)
        .and_then(Value::as_bool)
        .unwrap_or_else(|| panic!("{} lacks {pointer}", row["id"]))
}

fn ids(list: &[String]) -> String {
    if list.is_empty() {
        "(none)".into()
    } else {
        list.join(" ")
    }
}

#[test]
fn candidate_exact_initial_corpus_against_the_released_parser() {
    assert_eq!(
        sha256(b"abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    let digest = sha256(CORPUS);
    println!(
        "CANDIDATE-TARGET arch={} os={} family={}",
        std::env::consts::ARCH,
        std::env::consts::OS,
        std::env::consts::FAMILY
    );
    println!(
        "CANDIDATE-SOURCE commit={SOURCE} length={} sha256={digest}",
        CORPUS.len()
    );
    assert_eq!(CORPUS.len(), LENGTH);
    assert_eq!(digest, DIGEST);
    let corpus: Value = serde_json::from_slice(CORPUS).expect("candidate corpus is JSON");
    let cases = corpus["exactInitialPilotCases"]
        .as_array()
        .expect("exactInitialPilotCases");
    let echoes = corpus["grantEchoCases"].as_array().expect("grantEchoCases");
    let reviews = corpus["exactInitialPilotReviewCases"]
        .as_array()
        .expect("exactInitialPilotReviewCases");
    println!(
        "CANDIDATE-COUNTS exactInitialPilotCases={} grantEchoCases={} exactInitialPilotReviewCases={}",
        cases.len(),
        echoes.len(),
        reviews.len()
    );
    assert_eq!(cases.len(), 215);
    assert_eq!(echoes.len(), 16);
    assert_eq!(reviews.len(), 8);
    let mut tallies: [BTreeMap<String, usize>; 3] = Default::default();
    let mut s1_wider = Vec::new();
    let mut s1_narrower = Vec::new();
    let mut s3_wider = Vec::new();
    let mut s3_narrower = Vec::new();
    let mut s2_wider = Vec::new();
    let mut s2_narrower = Vec::new();
    let mut alabel = None;
    let mut evaluated = 0;
    for row in cases {
        let id = row["id"].as_str().expect("row id").to_string();
        let pilot_rule = flag(row, "/contexts/pilot/ruleAccepted");
        let pilot_goto = flag(row, "/contexts/pilot/gotoAccepted");
        let general = flag(row, "/contexts/general/ruleValid");
        let allow = row["allow"].as_array().expect("row allow");
        let single = match allow.as_slice() {
            [rule] if rule["kind"] == "exact" => rule["url"].as_str(),
            _ => None,
        };
        let (s1, s2) = match single {
            Some(url) => (
                surface(&["lease", "request", "--allow", url, "--for", "15m"], true),
                surface(&["lease", "request", "--from", "L1", "--add", url], false),
            ),
            None => (Outcome::NotApplicable, Outcome::NotApplicable),
        };
        let s3 = match row["url"].as_str() {
            Some(url) => surface(&["--lease", "L1", "goto", url], false),
            None => Outcome::NotApplicable,
        };
        println!(
            "CANDIDATE-ROW id={id} pilotRule={pilot_rule} pilotGoto={pilot_goto} generalRuleValid={general} S1={} S2={} S3={}",
            s1.label(),
            s2.label(),
            s3.label()
        );
        for (tally, outcome) in tallies.iter_mut().zip([&s1, &s2, &s3]) {
            let key = match outcome.accepted() {
                None => "n/a",
                Some(true) => "accepted",
                Some(false) => "denied",
            };
            *tally.entry(key.into()).or_default() += 1;
            *tally.entry(format!("  {}", outcome.label())).or_default() += 1;
        }
        match s1.accepted() {
            Some(true) if !pilot_rule => s1_wider.push(id.clone()),
            Some(false) if pilot_rule => s1_narrower.push(id.clone()),
            _ => {}
        }
        match s2.accepted() {
            Some(true) if !general => s2_wider.push(id.clone()),
            Some(false) if general => s2_narrower.push(id.clone()),
            _ => {}
        }
        match s3.accepted() {
            Some(true) if !pilot_goto => s3_wider.push(id.clone()),
            Some(false) if pilot_goto => s3_narrower.push(id.clone()),
            _ => {}
        }
        if id == ALABEL {
            alabel = Some((s1.clone(), s2.clone(), s3.clone()));
        }
        evaluated += 1;
    }
    println!("CANDIDATE-EVALUATED rows={evaluated}");
    assert_eq!(evaluated, cases.len());
    println!("CANDIDATE-SUMMARY");
    for (name, tally) in ["S1 initial", "S2 add", "S3 goto"].iter().zip(&tallies) {
        let count = |key: &str| tally.get(key).copied().unwrap_or(0);
        println!(
            "{name}: accepted={} denied={} n/a={}",
            count("accepted"),
            count("denied"),
            count("n/a")
        );
        for (key, n) in tally.iter().filter(|(key, _)| key.starts_with("  ")) {
            println!("{key}={n}");
        }
    }
    println!("S1 accepted, pilotRule=false: {}", ids(&s1_wider));
    println!("S1 denied, pilotRule=true: {}", ids(&s1_narrower));
    println!(
        "S3 accepted, pilotGoto=false (released parser WIDER than the pilot): {}",
        ids(&s3_wider)
    );
    println!(
        "S3 denied, pilotGoto=true (released parser NARROWER than the pilot): {}",
        ids(&s3_narrower)
    );
    println!("S2 accepted, generalRuleValid=false: {}", ids(&s2_wider));
    println!("S2 denied, generalRuleValid=true: {}", ids(&s2_narrower));
    let (a1, a2, a3) = alabel.expect("the candidate carries the invalid A-label row");
    let any = [&a1, &a2, &a3]
        .iter()
        .any(|outcome| outcome.accepted() == Some(true));
    println!(
        "{ALABEL}: S1={} S2={} S3={} anySurfaceAccepts={any}",
        a1.label(),
        a2.label(),
        a3.label()
    );
    println!(
        "Surfaces accepting {ALABEL}: {}",
        if any { ALABEL } else { "(none)" }
    );
    let named = |rows: &[Value]| -> Vec<String> {
        rows.iter()
            .map(|row| row["id"].as_str().expect("row id").to_string())
            .collect()
    };
    println!(
        "exactInitialPilotReviewCases ({}): {}: not exercised: needs runtime change",
        reviews.len(),
        named(reviews).join(" ")
    );
    println!(
        "grantEchoCases ({}): {}: not exercised: needs runtime change (broker side, not a client cell)",
        echoes.len(),
        named(echoes).join(" ")
    );
    println!("released 2.2.0 behaviour only; not an acknowledgment of the candidate");
}
