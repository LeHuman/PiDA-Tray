//! PiDA HTTP client and schema types.
//!
//! Server key order is meaningful (YAML declaration order), so every ordered
//! collection here is a `Vec<(String, _)>`, never a `HashMap`/`BTreeMap`.

use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::constants::{CONNECT_TIMEOUT, DISPATCH_TIMEOUT, REQUEST_TIMEOUT};

// ---------------------------------------------------------------------------
// Schema types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ParamKind {
    Bool,
    Int,
    Float,
    Enum,
    String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ParamSpec {
    pub kind: ParamKind,
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub step: Option<f64>,
    /// Allowed members for `Enum`, kept as raw JSON so dispatch sends the exact value.
    pub values: Vec<Value>,
    #[serde(default)]
    pub required: bool,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct ActionSchema {
    pub params: Vec<(String, ParamSpec)>,
    pub inputs: Vec<String>,
    pub outputs: Vec<String>,
    pub timeout: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct GroupSchema {
    pub actions: Vec<(String, ActionSchema)>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Schema {
    pub groups: Vec<(String, GroupSchema)>,
}

impl ParamSpec {
    pub fn from_value(v: &Value) -> Self {
        let values: Vec<Value> = v
            .get("enum")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();

        let kind = match v
            .get("type")
            .and_then(Value::as_str)
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            Some("bool" | "boolean") => ParamKind::Bool,
            Some("int" | "integer") => ParamKind::Int,
            Some("float" | "number" | "double") => ParamKind::Float,
            Some("enum") => ParamKind::Enum,
            Some("string" | "str") => ParamKind::String,
            _ if !values.is_empty() => ParamKind::Enum,
            _ => ParamKind::String,
        };

        // Accept both `"range": [min, max]` and separate `min` / `max` keys.
        let (mut min, mut max) = (None, None);
        if let Some([lo, hi]) = v.get("range").and_then(Value::as_array).map(Vec::as_slice) {
            min = lo.as_f64();
            max = hi.as_f64();
        }
        min = min.or_else(|| v.get("min").and_then(Value::as_f64));
        max = max.or_else(|| v.get("max").and_then(Value::as_f64));

        Self {
            kind,
            min,
            max,
            step: v.get("step").and_then(Value::as_f64),
            values,
            required: v.get("required").and_then(Value::as_bool).unwrap_or(false),
        }
    }
}

fn string_list(v: Option<&Value>) -> Vec<String> {
    v.and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

impl ActionSchema {
    pub fn from_value(v: &Value) -> Self {
        let params = v
            .get("params")
            .and_then(Value::as_object)
            .map(|m| {
                m.iter()
                    .map(|(k, p)| (k.clone(), ParamSpec::from_value(p)))
                    .collect()
            })
            .unwrap_or_default();
        Self {
            params,
            inputs: string_list(v.get("inputs")),
            outputs: string_list(v.get("outputs")),
            timeout: v.get("timeout").and_then(Value::as_f64),
        }
    }

    pub fn param(&self, name: &str) -> Option<&ParamSpec> {
        self.params.iter().find(|(k, _)| k == name).map(|(_, p)| p)
    }
}

impl Schema {
    /// Parse `{"groups": {"<path>": {"actions": {"<name>": {...}}}}}`, preserving key order.
    pub fn from_value(v: &Value) -> Result<Self, ApiError> {
        let groups = v
            .get("groups")
            .and_then(Value::as_object)
            .ok_or_else(|| ApiError::Parse("schema is missing `groups`".into()))?;
        let groups = groups
            .iter()
            .map(|(path, g)| {
                let actions = g
                    .get("actions")
                    .and_then(Value::as_object)
                    .map(|m| {
                        m.iter()
                            .map(|(n, a)| (n.clone(), ActionSchema::from_value(a)))
                            .collect()
                    })
                    .unwrap_or_default();
                (path.clone(), GroupSchema { actions })
            })
            .collect();
        Ok(Self { groups })
    }
}

/// `"<group>/<action>"`, or just `"<action>"` for top-level (`""`) groups.
pub fn action_path(group: &str, action: &str) -> String {
    if group.is_empty() {
        action.to_owned()
    } else {
        format!("{group}/{action}")
    }
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApiError {
    /// Connection / timeout / TLS failure — the server is unreachable.
    Network(String),
    /// Non-2xx response with the server's `error` message when available.
    Status {
        code: u16,
        message: String,
    },
    Parse(String),
}

impl ApiError {
    pub fn is_network(&self) -> bool {
        matches!(self, Self::Network(_))
    }
    /// `409`: a newer request in the same group queue replaced this one.
    pub fn is_superseded(&self) -> bool {
        matches!(self, Self::Status { code: 409, .. })
    }
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Network(m) => write!(f, "Can't reach server: {m}"),
            Self::Status { code, message } => write!(f, "{message} (HTTP {code})"),
            Self::Parse(m) => write!(f, "Unexpected response: {m}"),
        }
    }
}

impl std::error::Error for ApiError {}

// ---------------------------------------------------------------------------
// Client
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
#[allow(dead_code)] // fields mirror /api/health; only `ok` is read today
pub struct Health {
    pub ok: bool,
    pub deps: Option<String>,
    pub groups: Vec<Value>,
    pub endpoints: Option<u64>,
}

#[derive(Clone)]
pub struct PidaClient {
    http: reqwest::Client,
    base: String,
}

/// Percent-encode everything except unreserved characters and `/`.
pub fn encode_path(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for b in path.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

impl PidaClient {
    /// `base` must already be normalized (see `config::normalize_server_url`).
    pub fn new(base: &str) -> Result<Self, ApiError> {
        let http = reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            .user_agent(concat!("pida-tray/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| ApiError::Network(e.to_string()))?;
        Ok(Self {
            http,
            base: base.trim_end_matches('/').to_owned(),
        })
    }

    fn url(&self, path: &str) -> String {
        format!(
            "{}/api/{}",
            self.base,
            encode_path(path.trim_start_matches('/'))
        )
    }

    pub async fn health(&self) -> Result<Health, ApiError> {
        let v = parse(self.http.get(self.url("health")).send().await).await?;
        serde_json::from_value(v).map_err(|e| ApiError::Parse(e.to_string()))
    }

    pub async fn schema(&self) -> Result<Schema, ApiError> {
        let v = parse(self.http.get(self.url("schema")).send().await).await?;
        Schema::from_value(&v)
    }

    /// Empty-delta state readback for a parameterized endpoint.
    pub async fn get_state(&self, path: &str) -> Result<Map<String, Value>, ApiError> {
        into_outputs(parse(self.http.get(self.url(path)).send().await).await?)
    }

    /// Parameterized endpoints take a partial-delta JSON body; legacy ones take `{}`.
    pub async fn dispatch(&self, path: &str, body: &Value) -> Result<Map<String, Value>, ApiError> {
        let req = self
            .http
            .post(self.url(path))
            .timeout(DISPATCH_TIMEOUT)
            .json(body);
        into_outputs(parse(req.send().await).await?)
    }

    /// `Ok(None)` for a 404 (no icon for that path).
    pub async fn icon(&self, path: &str) -> Result<Option<Vec<u8>>, ApiError> {
        let resp = self
            .http
            .get(self.url(&format!("icon/{path}")))
            .send()
            .await
            .map_err(|e| ApiError::Network(e.to_string()))?;
        match resp.status().as_u16() {
            404 => Ok(None),
            200..=299 => resp
                .bytes()
                .await
                .map(|b| Some(b.to_vec()))
                .map_err(|e| ApiError::Network(e.to_string())),
            code => Err(ApiError::Status {
                code,
                message: "icon request failed".into(),
            }),
        }
    }
}

async fn parse(resp: Result<reqwest::Response, reqwest::Error>) -> Result<Value, ApiError> {
    let resp = resp.map_err(|e| ApiError::Network(e.to_string()))?;
    let code = resp.status().as_u16();
    let text = resp
        .text()
        .await
        .map_err(|e| ApiError::Network(e.to_string()))?;
    let value: Value = serde_json::from_str(&text).map_err(|e| ApiError::Parse(e.to_string()))?;
    if !(200..300).contains(&code) || value.get("ok") == Some(&Value::Bool(false)) {
        let message = value
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("request failed")
            .to_owned();
        return Err(ApiError::Status { code, message });
    }
    Ok(value)
}

/// Strip the `ok` envelope key, leaving only output values.
fn into_outputs(v: Value) -> Result<Map<String, Value>, ApiError> {
    match v {
        Value::Object(mut m) => {
            m.remove("ok");
            Ok(m)
        }
        _ => Err(ApiError::Parse("expected a JSON object".into())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCHEMA: &str = r#"{
      "groups": {
        "zeta/lamp": {"actions": {
          "set": {
            "params": {
              "power": {"type": "bool", "required": true},
              "brightness": {"type": "int", "range": [0, 100]},
              "mode": {"type": "enum", "enum": ["warm", "cool"]},
              "alpha": {"type": "float", "min": 0.0, "max": 1.0}
            },
            "inputs": ["power", "brightness", "mode", "alpha"],
            "outputs": ["power", "brightness"],
            "timeout": 5
          }
        }},
        "alpha/fan": {"actions": {"zap": {}, "boost": {}}},
        "": {"actions": {"ping": {}}}
      }
    }"#;

    #[test]
    fn preserves_server_key_order() {
        let schema = Schema::from_value(&serde_json::from_str(SCHEMA).unwrap()).unwrap();
        let groups: Vec<_> = schema.groups.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(groups, ["zeta/lamp", "alpha/fan", ""]);

        let actions: Vec<_> = schema.groups[1]
            .1
            .actions
            .iter()
            .map(|(k, _)| k.as_str())
            .collect();
        assert_eq!(actions, ["zap", "boost"]);

        let set = &schema.groups[0].1.actions[0].1;
        let params: Vec<_> = set.params.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(params, ["power", "brightness", "mode", "alpha"]);
    }

    #[test]
    fn parses_param_kinds_and_ranges() {
        let schema = Schema::from_value(&serde_json::from_str(SCHEMA).unwrap()).unwrap();
        let set = &schema.groups[0].1.actions[0].1;
        assert_eq!(set.param("power").unwrap().kind, ParamKind::Bool);
        assert!(set.param("power").unwrap().required);
        let b = set.param("brightness").unwrap();
        assert!(!b.required);
        assert_eq!(
            (b.kind, b.min, b.max),
            (ParamKind::Int, Some(0.0), Some(100.0))
        );
        assert_eq!(set.param("mode").unwrap().values.len(), 2);
        let a = set.param("alpha").unwrap();
        assert_eq!((a.min, a.max), (Some(0.0), Some(1.0)));
        assert_eq!(set.timeout, Some(5.0));
    }

    #[test]
    fn schema_round_trips_through_json_with_order() {
        let schema = Schema::from_value(&serde_json::from_str(SCHEMA).unwrap()).unwrap();
        let back: Schema = serde_json::from_str(&serde_json::to_string(&schema).unwrap()).unwrap();
        assert_eq!(schema, back);
    }

    #[test]
    fn missing_groups_is_an_error() {
        assert!(Schema::from_value(&serde_json::json!({"nope": 1})).is_err());
    }

    #[test]
    fn action_paths() {
        assert_eq!(
            action_path("living_room/lamp", "set"),
            "living_room/lamp/set"
        );
        assert_eq!(action_path("", "ping"), "ping");
    }

    #[test]
    fn path_encoding() {
        assert_eq!(encode_path("office/fan set"), "office/fan%20set");
        assert_eq!(encode_path("a-b_c.d~e/f"), "a-b_c.d~e/f");
    }

    #[test]
    fn outputs_drop_ok_key() {
        let m = into_outputs(serde_json::json!({"ok": true, "power": true})).unwrap();
        assert!(m.get("ok").is_none() && m["power"] == true);
        assert!(into_outputs(serde_json::json!([1])).is_err());
    }

    fn spawn_stub_server(
        responses: Vec<(u16, &'static str, Vec<u8>)>,
    ) -> (String, std::sync::mpsc::Receiver<(String, String)>) {
        use std::io::{Read, Write};
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for (status, ctype, body) in responses {
                let (mut stream, _) = listener.accept().unwrap();
                let mut buf = [0u8; 4096];
                let n = stream.read(&mut buf).unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]);
                let first_line = req.lines().next().unwrap_or("").to_owned();
                let req_body = req.split("\r\n\r\n").nth(1).unwrap_or("").to_owned();
                let _ = tx.send((first_line, req_body));
                let reason = match status {
                    200 => "OK",
                    404 => "Not Found",
                    409 => "Conflict",
                    _ => "Error",
                };
                let head = format!(
                    "HTTP/1.1 {status} {reason}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(&body);
            }
        });
        (format!("http://{addr}"), rx)
    }

    #[test]
    fn client_round_trips_health_schema_dispatch_errors_and_icons() {
        let png = vec![0x89, 0x50, 0x4E, 0x47];
        let (base, rx) = spawn_stub_server(vec![
            (
                200,
                "application/json",
                br#"{"ok":true,"endpoints":2}"#.to_vec(),
            ),
            (200, "application/json", SCHEMA.as_bytes().to_vec()),
            (
                200,
                "application/json",
                br#"{"ok":true,"power":true,"brightness":50}"#.to_vec(),
            ),
            (
                409,
                "application/json",
                br#"{"ok":false,"error":"superseded"}"#.to_vec(),
            ),
            (200, "image/png", png.clone()),
            (
                404,
                "application/json",
                br#"{"ok":false,"error":"no icon"}"#.to_vec(),
            ),
        ]);

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async move {
            let client = PidaClient::new(&base).unwrap();

            assert!(client.health().await.unwrap().ok);
            let schema = client.schema().await.unwrap();
            assert_eq!(schema.groups[0].0, "zeta/lamp");

            let out = client
                .dispatch("zeta/lamp/set", &serde_json::json!({"power": true}))
                .await
                .unwrap();
            assert_eq!(out["power"], serde_json::json!(true));

            let err = client
                .dispatch("zeta/lamp/set", &serde_json::json!({"power": false}))
                .await
                .unwrap_err();
            assert!(err.is_superseded());

            assert_eq!(client.icon("zeta/lamp/set").await.unwrap(), Some(png));
            assert_eq!(client.icon("missing/icon").await.unwrap(), None);
        });

        let calls: Vec<_> = rx.try_iter().collect();
        assert_eq!(calls[0].0, "GET /api/health HTTP/1.1");
        assert_eq!(calls[1].0, "GET /api/schema HTTP/1.1");
        assert_eq!(calls[2].0, "POST /api/zeta/lamp/set HTTP/1.1");
        assert_eq!(
            serde_json::from_str::<Value>(&calls[2].1).unwrap(),
            serde_json::json!({"power": true})
        );
    }
}
