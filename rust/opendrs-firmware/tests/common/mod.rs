//! Shared helpers for the integration tests: the Python oracle (tests/parity.json, made by
//! rust/tools/make_parity_firmware.py), fixture paths, and the scripted / wrapping transports the
//! Python tests use.

#![allow(dead_code)] // each test binary uses a different subset

use std::cell::RefCell;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use opendrs_core::radio_link::{RadioError, Transport};
use opendrs_firmware::sha256_hex;
use serde_json::{Value, json};

pub fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../fixtures")
}

/// The folder above the repository checkout, where the Python tests look for the vendor files.
pub fn analysis_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

pub fn oracle() -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/parity.json");
    serde_json::from_str(&std::fs::read_to_string(path).expect("parity.json")).expect("valid JSON")
}

/// A fresh, empty directory for one test.
pub fn temp_dir(name: &str) -> PathBuf {
    static SERIAL: AtomicUsize = AtomicUsize::new(0);
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "{name}_{}_{}",
        std::process::id(),
        SERIAL.fetch_add(1, Ordering::Relaxed)
    ));
    if dir.exists() {
        std::fs::remove_dir_all(&dir).expect("clear temp dir");
    }
    std::fs::create_dir_all(&dir).expect("temp dir");
    dir
}

pub fn hex(text: &str) -> Vec<u8> {
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).expect("hex"))
        .collect()
}

pub fn hex_of(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// `{"len", "sha256", "hex" (when short)}`, as `bytes_facts` in the oracle script.
pub fn bytes_facts(data: &[u8]) -> Value {
    let mut out = json!({"len": data.len(), "sha256": sha256_hex(data)});
    if data.len() <= 256 {
        out["hex"] = json!(hex_of(data));
    }
    out
}

/// The test suite's synthetic image: (i * 7 + seed) & 0xFF.
pub fn pattern(size: usize, seed: usize) -> Vec<u8> {
    (0..size).map(|i| ((i * 7 + seed) & 0xFF) as u8).collect()
}

pub fn xorshift(size: usize, seed: u32) -> Vec<u8> {
    let mut x = seed;
    (0..size)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            (x & 0xFF) as u8
        })
        .collect()
}

/// Field-by-field comparison so a failure names every difference, not just the first.
pub fn diff(path: &str, expected: &Value, actual: &Value, out: &mut Vec<String>) {
    match (expected, actual) {
        (Value::Object(e), Value::Object(a)) => {
            for (key, value) in e {
                diff(
                    &format!("{path}.{key}"),
                    value,
                    a.get(key).unwrap_or(&Value::Null),
                    out,
                );
            }
            for key in a.keys().filter(|k| !e.contains_key(*k)) {
                out.push(format!("{path}.{key}: missing in python"));
            }
        }
        (Value::Array(e), Value::Array(a)) if e.len() == a.len() => {
            for (i, (ev, av)) in e.iter().zip(a).enumerate() {
                diff(&format!("{path}[{i}]"), ev, av, out);
            }
        }
        _ if expected == actual => {}
        _ => out.push(format!("{path}: python {expected} / rust {actual}")),
    }
}

pub fn assert_same(mismatches: &[String]) {
    assert!(
        mismatches.is_empty(),
        "{} differences:\n{}",
        mismatches.len(),
        mismatches.join("\n")
    );
}

/// Answers from a list in turn, then keeps repeating the last one (like `Cancel` in the oracle
/// script and the `iter([...])` cancel callbacks of the Python tests).
pub struct Answers {
    answers: Vec<bool>,
    calls: usize,
}

impl Answers {
    pub fn new(answers: Vec<bool>) -> Self {
        Answers { answers, calls: 0 }
    }

    pub fn next(&mut self) -> bool {
        let answer = self.answers[self.calls.min(self.answers.len() - 1)];
        self.calls += 1;
        answer
    }
}

/// Hands out `chunks` one per read (an empty chunk = an empty read, like a quiet serial port).
pub struct ScriptedTransport {
    chunks: VecDeque<Vec<u8>>,
    pub sent: Vec<Vec<u8>>,
    fail_on_write: Option<usize>,
    fail_reset: bool,
}

impl ScriptedTransport {
    pub fn new(chunks: Vec<Vec<u8>>, fail_on_write: Option<usize>, fail_reset: bool) -> Self {
        ScriptedTransport {
            chunks: chunks.into(),
            sent: Vec::new(),
            fail_on_write,
            fail_reset,
        }
    }
}

impl Transport for ScriptedTransport {
    fn write(&mut self, data: &[u8]) -> Result<(), RadioError> {
        if self.fail_on_write == Some(self.sent.len()) {
            return Err(RadioError::Failed("WriteFile failed".into()));
        }
        self.sent.push(data.to_vec());
        Ok(())
    }

    fn read_exact(&mut self, _n: usize, _timeout: Duration) -> Result<Vec<u8>, RadioError> {
        unreachable!("not used")
    }

    fn read_idle(&mut self, _idle: Duration, _max_wait: Duration) -> Result<Vec<u8>, RadioError> {
        Ok(self.chunks.pop_front().unwrap_or_default())
    }

    fn reset_input(&mut self) -> Result<(), RadioError> {
        if self.fail_reset {
            return Err(RadioError::Failed("ClearCommError failed".into()));
        }
        Ok(())
    }
}

/// A transport the test can still look at while a client uses it (e.g. from a cancel callback).
pub struct Shared<T>(pub Rc<RefCell<T>>);

impl<T: Transport> Transport for Shared<T> {
    fn write(&mut self, data: &[u8]) -> Result<(), RadioError> {
        self.0.borrow_mut().write(data)
    }

    fn read_exact(&mut self, n: usize, timeout: Duration) -> Result<Vec<u8>, RadioError> {
        self.0.borrow_mut().read_exact(n, timeout)
    }

    fn read_idle(&mut self, idle: Duration, max_wait: Duration) -> Result<Vec<u8>, RadioError> {
        self.0.borrow_mut().read_idle(idle, max_wait)
    }

    fn reset_input(&mut self) -> Result<(), RadioError> {
        self.0.borrow_mut().reset_input()
    }
}

/// Hands the inner transport's replies out a few bytes at a time (with empty reads).
pub struct Chunked<T> {
    pub inner: T,
    sizes: Vec<usize>,
    i: usize,
    buf: Vec<u8>,
}

impl<T> Chunked<T> {
    pub fn new(inner: T) -> Self {
        Chunked {
            inner,
            sizes: vec![1, 0, 7, 37, 3, 0, 13],
            i: 0,
            buf: Vec::new(),
        }
    }
}

impl<T: Transport> Transport for Chunked<T> {
    fn write(&mut self, data: &[u8]) -> Result<(), RadioError> {
        self.inner.write(data)
    }

    fn read_exact(&mut self, _n: usize, _timeout: Duration) -> Result<Vec<u8>, RadioError> {
        unreachable!("not used")
    }

    fn read_idle(&mut self, idle: Duration, max_wait: Duration) -> Result<Vec<u8>, RadioError> {
        let more = self.inner.read_idle(idle, max_wait)?;
        self.buf.extend(more);
        let n = self.sizes[self.i % self.sizes.len()].min(self.buf.len());
        self.i += 1;
        Ok(self.buf.drain(..n).collect())
    }

    fn reset_input(&mut self) -> Result<(), RadioError> {
        self.buf.clear();
        self.inner.reset_input()
    }
}

/// Passes at most `left` reply bytes through, then stays silent.
pub struct Truncated<T> {
    pub inner: T,
    left: usize,
}

impl<T> Truncated<T> {
    pub fn new(inner: T, limit: usize) -> Self {
        Truncated { inner, left: limit }
    }
}

impl<T: Transport> Transport for Truncated<T> {
    fn write(&mut self, data: &[u8]) -> Result<(), RadioError> {
        self.inner.write(data)
    }

    fn read_exact(&mut self, _n: usize, _timeout: Duration) -> Result<Vec<u8>, RadioError> {
        unreachable!("not used")
    }

    fn read_idle(&mut self, idle: Duration, max_wait: Duration) -> Result<Vec<u8>, RadioError> {
        let mut data = self.inner.read_idle(idle, max_wait)?;
        data.truncate(self.left);
        self.left -= data.len();
        Ok(data)
    }

    fn reset_input(&mut self) -> Result<(), RadioError> {
        self.inner.reset_input()
    }
}
