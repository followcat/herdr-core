//! Framed calls and attach reply mapping on one remote line.

use super::*;

impl Line {
    pub(super) fn new(frames: mpsc::UnboundedSender<Vec<u8>>, hello: &Value) -> Arc<Line> {
        let features = hello["features"].as_array().into_iter().flatten();
        Arc::new(Line {
            features: features.filter_map(|f| f.as_str().map(String::from)).collect(),
            frames,
            pending: Mutex::default(),
            streams: Mutex::default(),
            next: AtomicU64::new(2),
            mirror: Mutex::default(),
        })
    }

    /// A reply to one of our calls: to whoever waits for it.
    pub(super) fn take_reply(&self, mut reply: Value) {
        let Some((waiter, sink)) = reply["id"].as_u64().and_then(|id| lock(&self.pending).remove(&id)) else { return };
        if let (Some((sink, required)), Some(there)) = (sink, reply["result"]["channel"].as_u64()) {
            let lost = if required.resize && reply["result"]["resize_notifications"] != true {
                Some(RESIZE_LOST)
            } else if required.quiet
                && (!self.features.iter().any(|f| f == "quiet_human_release")
                    || reply["result"]["quiet_human_release"] != true)
            {
                Some(QUIET_LOST)
            } else {
                None
            };
            if let Some(lost) = lost {
                let _ = self.frames.send(wire::encode(there as u32, wire::CLOSE, b""));
                reply =
                    crate::rpc::response(reply["id"].clone(), Err(RpcError::new("unavailable", lost, "attach again")));
            } else {
                lock(&self.streams).insert(there as u32, sink);
            }
        }
        let _ = waiter.send(reply);
    }

    pub(super) fn message(&self, message: &Value) {
        let _ = self.frames.send(wire::encode(0, wire::DATA, message.to_string().as_bytes()));
    }

    /// A call that gets `deadline` to answer: a stuck machine never holds us.
    pub async fn call(
        &self,
        machine: &str,
        method: &str,
        params: Value,
        deadline: Duration,
    ) -> Result<Value, RpcError> {
        self.call_into(machine, method, params, None, deadline).await
    }

    /// A call whose reply may open a stream (`channel`): it goes to `sink`
    /// from its first byte.
    pub(super) async fn call_into(
        &self,
        machine: &str,
        method: &str,
        params: Value,
        sink: Option<(Sink, Required)>,
        deadline: Duration,
    ) -> Result<Value, RpcError> {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let req = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}).to_string();
        // Too big for one frame would end the whole line: refuse just this call.
        if req.len() > wire::MAX_PAYLOAD {
            return Err(RpcError::invalid_params(format!(
                "too big to send to {machine} ({} MB at most)",
                wire::MAX_PAYLOAD >> 20
            )));
        }
        let (tx, rx) = oneshot::channel();
        lock(&self.pending).insert(id, (tx, sink));
        let _ = self.frames.send(wire::encode(0, wire::DATA, req.as_bytes()));
        let _asked = Asked { line: self, id };
        let Ok(reply) = tokio::time::timeout(deadline, rx).await else {
            return Err(RpcError::new(
                "unavailable",
                format!("{machine} didn't answer within {} s", deadline.as_secs()),
                "it may be busy or stuck; hc machine ls shows its line",
            ));
        };
        let reply = reply.map_err(|_| {
            RpcError::new(
                "unavailable",
                format!("lost the line to {machine} during the call"),
                "it may have happened there; check, then retry",
            )
        })?;
        match reply.get("error") {
            Some(e) => Err(RpcError::from_json(e)),
            None => Ok(reply["result"].clone()),
        }
    }
}
