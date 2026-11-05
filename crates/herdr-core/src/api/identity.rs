use super::*;

pub(super) async fn hello(core: Ctx<'_>, p: HelloParams) -> Result<HelloResult, RpcError> {
    if let Some(g) = p.generation.filter(|g| *g != GENERATION) {
        return Err(RpcError::new(
            "unsupported_generation",
            format!("protocol generation {g} not supported"),
            format!("this core speaks generation {GENERATION}"),
        ));
    }
    Ok(HelloResult {
        generation: GENERATION,
        version: version(),
        core: core.info.observed(),
        framed: p.framed,
        methods: METHOD_NAMES.iter().map(|m| m.to_string()).collect(),
        features: FEATURES.iter().map(|f| f.to_string()).collect(),
    })
}

pub(super) async fn server_status(core: Ctx<'_>, _: Empty) -> Result<StatusResult, RpcError> {
    let identity = core.info.observed();
    let st = core.lock();
    let live = st.reg.terminals.values().filter(|t| t.is_live()).count();
    let holders: Vec<HolderInfo> = st
        .holders
        .iter()
        .map(|l| HolderInfo {
            pid: l.pid,
            instance: l.instance.clone(),
            current: l.current,
            terminals: st
                .reg
                .terminals
                .values()
                .filter(|t| t.is_live() && t.holder.as_ref() == Some(&l.instance))
                .count(),
        })
        .collect();
    Ok(StatusResult {
        core: identity,
        version: version(),
        pid: std::process::id(),
        uptime_ms: core.started.elapsed().as_millis() as u64,
        holder: holders.iter().find(|h| h.current).cloned(),
        holders,
        live,
        finished: st.reg.terminals.len() - live,
        installed_by: crate::client::installed_by().map(String::from),
    })
}
