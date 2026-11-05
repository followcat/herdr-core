use super::*;

#[tokio::test(start_paused = true)]
async fn collected_snapshots_carry_the_vts_logical_footer() {
    let (core, root) = test_core();
    let catalog = crate::detect::Catalog::load(None, None);
    let (id, _frames) = screen_agent(&core, "cmd");
    {
        let mut st = core.lock();
        st.reg.terminals.get_mut(&id).unwrap().foreground.as_mut().unwrap().agent = Some("command-code");
        st.screens.insert(id, Screen::new(45, 24, 0));
    }
    let bar = "─".repeat(45);
    let text =
        format!(" ✧ Connection dropped mid-response. Retrying...  1.1k\r\n{bar}\r\n❯ Ask\r\n{bar}\r\n? for shortcuts");
    core.output(&mut core.lock(), id, text.as_bytes());
    core.screen_status_pass(&catalog).await;
    assert_eq!(word(&core, id).as_deref(), Some("working"));
    drop(core);
    remove_root(&root);
}
