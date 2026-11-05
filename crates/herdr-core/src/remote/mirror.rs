//! Remote terminal snapshots and ordered event patches.

use super::*;

/// Optional origin facts need support at every hop. Only inspect protocol
/// envelopes: document values and user labels are opaque.
pub(super) fn supported(value: &mut Value, features: &[String]) {
    if let Some(values) = value.as_array_mut() {
        for value in values {
            supported(value, features);
        }
        return;
    }
    let Some(object) = value.as_object_mut() else { return };
    if object.get("address").is_some_and(Value::is_string) && !features.iter().any(|f| f == "repository_facts") {
        object.remove("repository");
    }
    for key in ["terminals", "terminal", "record"] {
        if let Some(value) = object.get_mut(key) {
            supported(value, features);
        }
    }
}

/// A machine's terminals now (after a connect, or after it says we missed
/// some): subscribers here hear what appeared, changed or went.
pub(super) fn mirror_replace(core: &Core, m: &Machine, terminals: Vec<Value>) {
    let mut st = core.lock();
    let Some(name) = st.machines.iter().find(|current| current.id == m.id).map(|m| m.name.clone()) else { return };
    let fresh: std::collections::BTreeMap<String, Value> = terminals
        .into_iter()
        .map(|t| localize_in(core, &st, &name, &[], t))
        .map(|t| (t["address"].as_str().unwrap_or_default().to_string(), t))
        .collect();
    let old = st.mirrors.remove(&m.id).unwrap_or_default();
    for (address, record) in old.iter().filter(|(a, _)| !fresh.contains_key(*a)) {
        core.deliver(&mut st, "removed", address, json!({}), record, Some(record));
    }
    for (address, record) in fresh.iter().filter(|(a, r)| old.get(*a) != Some(*r)) {
        core.deliver(&mut st, "created", address, record.clone(), record, None);
    }
    st.mirrors.insert(m.id.clone(), fresh);
}

pub fn rename_mirror(core: &Core, st: &mut State, id: &str, old: &str, new: &str) {
    let Some(records) = st.mirrors.remove(id) else { return };
    let mut renamed = std::collections::BTreeMap::new();
    for record in records.values() {
        let mut record = record.clone();
        map_addresses(&mut record, &|s| match s.split_once('/') {
            Some((prefix, rest)) if prefix == old => format!("{new}/{rest}"),
            _ => s.to_owned(),
        });
        let address = record["address"].as_str().unwrap_or_default().to_owned();
        renamed.insert(address, record);
    }
    st.mirrors.insert(id.to_owned(), renamed.clone());
    for (address, record) in records {
        core.deliver(st, "removed", &address, json!({}), &record, Some(&record));
    }
    for (address, record) in renamed {
        core.deliver(st, "created", &address, record.clone(), &record, None);
    }
}

/// Event lines from a machine: applied to its mirror, passed on as seen from here.
pub(super) fn mirror_events(core: &Core, m: &Machine, line: &Line, payload: &[u8]) {
    let lines: Vec<Vec<u8>> = {
        let mut mirror = lock(&line.mirror);
        mirror.2.extend_from_slice(payload);
        let whole = mirror.2.iter().rposition(|b| *b == b'\n').map_or(0, |at| at + 1);
        let done: Vec<u8> = mirror.2.drain(..whole).collect();
        done.split(|b| *b == b'\n').filter(|l| !l.is_empty()).map(<[u8]>::to_vec).collect()
    };
    for l in lines {
        let Ok(mut msg) = serde_json::from_slice::<Value>(&l) else { continue };
        match msg["method"].as_str() {
            Some("event") => {
                if msg["params"]["type"] == "repository" && !line.features.iter().any(|f| f == "repository_facts") {
                    continue;
                }
                supported(&mut msg["params"]["data"], &line.features);
                mirror_event(core, m, msg["params"].clone());
            }
            Some("missed") => {
                supported(&mut msg["params"], &line.features);
                mirror_replace(core, m, msg["params"]["terminals"].as_array().cloned().unwrap_or_default());
                core.documents_missed(&mut core.lock());
            }
            _ => {}
        }
    }
}

pub(super) fn mirror_event(core: &Core, m: &Machine, p: Value) {
    let mut st = core.lock();
    let Some(name) = st.machines.iter().find(|current| current.id == m.id).map(|m| m.name.clone()) else { return };
    let p = localize_in(core, &st, &name, &[], p);
    let (kind, address, data) =
        (p["type"].as_str().unwrap_or_default(), p["terminal"].as_str().unwrap_or_default(), p["data"].clone());
    if kind == "doc" {
        // Only this machine, not docs it in turn watches on a third core.
        if p.get("machine").is_none() {
            core.doc_event(&mut st, data, Some(&name));
        }
        return;
    }
    let mirror = st.mirrors.entry(m.id.clone()).or_default();
    let (record, was) = match kind {
        "created" | "restarted" => {
            mirror.insert(address.into(), data.clone());
            (data.clone(), None)
        }
        "removed" => match mirror.remove(address) {
            Some(record) => (record.clone(), Some(record)),
            None => return,
        },
        _ => {
            let Some(record) = mirror.get_mut(address) else { return };
            let was = record.clone();
            apply(record, kind, &data);
            (record.clone(), Some(was))
        }
    };
    core.deliver(&mut st, kind, address, data, &record, was.as_ref());
}

/// An event's change, applied to a terminal record (see schema/README.md).
pub(super) fn apply(record: &mut Value, kind: &str, data: &Value) {
    let set = |record: &mut Value, keys: &[&str]| keys.iter().for_each(|k| record[*k] = data[*k].clone());
    match kind {
        "finished" | "lost" => {
            record["state"] = if kind == "lost" { "live" } else { "finished" }.into();
            if kind == "lost" {
                record["pid"] = Value::Null;
            }
            set(record, &["reason", "exit_code", "signal", "finished_ms"]);
            record.as_object_mut().map(|r| r.remove("foreground"));
        }
        "status" => set(record, &["status", "records", "reported"]),
        "turn" => set(record, &["turn"]),
        "title" => set(record, &["title"]),
        "labels" => set(record, &["labels"]),
        "keep" => set(record, &["keep"]),
        "size" => set(record, &["cols", "rows"]),
        "foreground" => record["foreground"] = data.clone(),
        "repository" if data.get("repository").is_some() => set(record, &["repository"]),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsupported_facts_are_removed_only_from_terminal_envelopes() {
        let original = json!({"terminals":[{"address":"t1","repository":{"branch":"old"},
            "labels":{"repository":"ordinary"}}],"value":{"repository":"document"}});
        let mut old = original.clone();
        supported(&mut old, &[]);
        assert!(old["terminals"][0].get("repository").is_none());
        assert_eq!(old["terminals"][0]["labels"]["repository"], "ordinary");
        assert_eq!(old["value"]["repository"], "document");
        let mut new = original.clone();
        supported(&mut new, &["repository_facts".into()]);
        assert_eq!(new, original);
    }
}
