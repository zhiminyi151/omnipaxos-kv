use reqwest::blocking::Client as HttpClient;
use serde::Deserialize;
use serde_json::{json, Value};
use std::io::{self, BufRead, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

#[derive(Debug, Deserialize)]
struct Envelope {
    src: String,
    #[serde(rename = "dest")]
    _dest: String,
    body: Value,
}

#[derive(Debug, Deserialize)]
struct GetBody {
    ok: bool,
    value: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CasBody {
    applied: bool,
}

#[derive(Debug, Deserialize)]
struct ShimErrorBody {
    indeterminate: Option<bool>,
}

struct Node {
    node_id: String,
    shim_url: String,
    next_msg_id: AtomicU64,
    http: HttpClient,
}

impl Node {
    fn new() -> Self {
        let shim_url = std::env::var("SHIM_URL").unwrap_or_else(|_| "http://127.0.0.1:3000".into());
        let timeout_ms = std::env::var("MAELSTROM_SHIM_TIMEOUT_MS")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(4000);
        let http = HttpClient::builder()
            .timeout(Duration::from_millis(timeout_ms))
            .build()
            .expect("failed to build HTTP client");
        Self {
            node_id: String::new(),
            shim_url,
            next_msg_id: AtomicU64::new(1),
            http,
        }
    }

    fn handle(&mut self, req: Envelope) -> Value {
        let msg_type = req
            .body
            .get("type")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        match msg_type {
            "init" => self.handle_init(req),
            "read" => self.handle_read(req),
            "write" => self.handle_write(req),
            "cas" => self.handle_cas(req),
            _ => self.error_reply(req, 10, "unsupported request type"),
        }
    }

    fn handle_init(&mut self, req: Envelope) -> Value {
        self.node_id = req
            .body
            .get("node_id")
            .and_then(|v| v.as_str())
            .unwrap_or("n0")
            .to_string();
        self.reply(req, json!({"type": "init_ok"}))
    }

    fn handle_read(&self, req: Envelope) -> Value {
        let key = match req.body.get("key") {
            Some(k) => encode_json(k),
            None => return self.error_reply(req, 13, "read missing key"),
        };
        let path = format!("{}/kv/{}", self.shim_url, urlencoding::encode(&key));
        match self.http.get(path).send() {
            Ok(resp) if resp.status().is_success() => match resp.json::<GetBody>() {
                Ok(body) if body.ok => {
                    let value = match body.value {
                        Some(s) => decode_json(&s),
                        None => Value::Null,
                    };
                    self.reply(req, json!({"type": "read_ok", "value": value}))
                }
                Ok(_) => self.error_reply(req, 13, "shim returned unsuccessful read"),
                Err(_) => self.indeterminate_reply(req, "invalid read response from shim"),
            },
            Ok(resp) => match resp.json::<ShimErrorBody>() {
                Ok(err) if err.indeterminate.unwrap_or(false) => {
                    self.indeterminate_reply(req, "indeterminate read")
                }
                _ => self.error_reply(req, 13, "read failed"),
            },
            Err(_) => self.indeterminate_reply(req, "read request failed"),
        }
    }

    fn handle_write(&self, req: Envelope) -> Value {
        let key = match req.body.get("key") {
            Some(k) => encode_json(k),
            None => return self.error_reply(req, 13, "write missing key"),
        };
        let value = match req.body.get("value") {
            Some(v) => encode_json(v),
            None => return self.error_reply(req, 13, "write missing value"),
        };
        let path = format!("{}/kv/{}", self.shim_url, urlencoding::encode(&key));
        match self.http.put(path).json(&json!({ "value": value })).send() {
            Ok(resp) if resp.status().is_success() => self.reply(req, json!({"type": "write_ok"})),
            Ok(resp) => match resp.json::<ShimErrorBody>() {
                Ok(err) if err.indeterminate.unwrap_or(false) => {
                    self.indeterminate_reply(req, "indeterminate write")
                }
                _ => self.error_reply(req, 13, "write failed"),
            },
            Err(_) => self.indeterminate_reply(req, "write request failed"),
        }
    }

    fn handle_cas(&self, req: Envelope) -> Value {
        let key_val = match req.body.get("key") {
            Some(v) => v.clone(),
            None => return self.error_reply(req, 13, "cas missing key"),
        };
        let from_val = match req.body.get("from") {
            Some(v) => v.clone(),
            None => return self.error_reply(req, 13, "cas missing from"),
        };
        let to_val = match req.body.get("to") {
            Some(v) => v.clone(),
            None => return self.error_reply(req, 13, "cas missing to"),
        };
        let key = encode_json(&key_val);
        let from = encode_json(&from_val);
        let to = encode_json(&to_val);

        let path = format!("{}/kv/cas", self.shim_url);
        let cas_resp = self
            .http
            .post(path)
            .json(&json!({ "key": key, "from": from, "to": to }))
            .send();

        match cas_resp {
            Ok(resp) if resp.status().is_success() => match resp.json::<CasBody>() {
                Ok(body) if body.applied => self.reply(req, json!({"type": "cas_ok"})),
                Ok(_) => self.classify_cas_failure(req, &key_val, &from_val),
                Err(_) => self.indeterminate_reply(req, "invalid cas response from shim"),
            },
            Ok(resp) => match resp.json::<ShimErrorBody>() {
                Ok(err) if err.indeterminate.unwrap_or(false) => {
                    self.indeterminate_reply(req, "indeterminate cas")
                }
                _ => self.error_reply(req, 13, "cas failed"),
            },
            Err(_) => self.indeterminate_reply(req, "cas request failed"),
        }
    }

    fn classify_cas_failure(&self, req: Envelope, key: &Value, from: &Value) -> Value {
        let encoded_key = encode_json(key);
        let path = format!("{}/kv/{}", self.shim_url, urlencoding::encode(&encoded_key));
        match self.http.get(path).send() {
            Ok(resp) if resp.status().is_success() => match resp.json::<GetBody>() {
                Ok(body) => {
                    let cur = body.value.map(|v| decode_json(&v));
                    match cur {
                        None => self.error_reply(req, 20, "key does not exist"),
                        Some(v) if v != *from => self.error_reply(req, 22, "cas from mismatch"),
                        Some(_) => self.error_reply(req, 22, "cas failed"),
                    }
                }
                Err(_) => self.error_reply(req, 13, "unable to classify cas failure"),
            },
            _ => self.error_reply(req, 13, "unable to classify cas failure"),
        }
    }

    fn indeterminate_reply(&self, req: Envelope, text: &str) -> Value {
        self.error_reply(req, 14, text)
    }

    fn error_reply(&self, req: Envelope, code: i64, text: &str) -> Value {
        self.reply(
            req,
            json!({
                "type": "error",
                "code": code,
                "text": text
            }),
        )
    }

    fn reply(&self, req: Envelope, mut body: Value) -> Value {
        let in_reply_to = req
            .body
            .get("msg_id")
            .and_then(|v| v.as_i64())
            .unwrap_or_default();
        if let Some(map) = body.as_object_mut() {
            map.insert("in_reply_to".to_string(), Value::from(in_reply_to));
            map.insert(
                "msg_id".to_string(),
                Value::from(self.next_msg_id.fetch_add(1, Ordering::SeqCst)),
            );
        }
        json!({
            "src": self.node_id,
            "dest": req.src,
            "body": body
        })
    }
}

fn encode_json(v: &Value) -> String {
    serde_json::to_string(v).unwrap_or_else(|_| "null".to_string())
}

fn decode_json(s: &str) -> Value {
    serde_json::from_str::<Value>(s).unwrap_or(Value::String(s.to_string()))
}

fn main() {
    let stdin = io::stdin();
    let mut stdout = io::stdout();
    let mut node = Node::new();

    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) if !l.trim().is_empty() => l,
            Ok(_) => continue,
            Err(_) => continue,
        };

        let req: Envelope = match serde_json::from_str(&line) {
            Ok(msg) => msg,
            Err(_) => continue,
        };
        let response = node.handle(req);
        let serialized = serde_json::to_string(&response).expect("failed to serialize response");
        writeln!(stdout, "{serialized}").expect("failed to write response");
        stdout.flush().expect("failed to flush stdout");
    }
}
