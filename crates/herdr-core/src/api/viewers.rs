use super::*;

pub(super) async fn terminal_attach(core: Ctx<'_>, p: AttachParams) -> Result<AttachResult, RpcError> {
    let Some(conn) = core.conn else {
        return Err(RpcError::new(
            "needs_framed",
            "attach needs a framed connection",
            "send hello with framed: true first, or use `hc attach`",
        ));
    };
    let id = resolve(core, &p.terminal)?;
    if p.caller.as_deref().and_then(crate::terminals::parse_id) == Some(id) {
        return Err(RpcError::new(
            "refused",
            format!(
                "{} is the terminal you are in: attached to itself, it would show its own output forever",
                crate::terminals::address(id)
            ),
            "attach from another terminal",
        ));
    }
    let req = crate::viewers::AttachRequest {
        observe: p.observe,
        resize_notifications: p.resize_notifications,
        quiet_human_release: p.quiet_human_release,
        history: p.history,
        size: (p.cols, p.rows),
        display: p.display,
        resumed: p.resumed,
    };
    let channel = core.attach(conn, id, req)?;
    Ok(AttachResult {
        channel,
        terminal: Public(record(core, id)?),
        resize_notifications: p.observe && p.resize_notifications,
        automatic_input: true,
        quiet_human_release: p.quiet_human_release.then_some(!p.observe),
    })
}
