// Offline wrapper unit tests; original GPLv3 applies, 2026-11-06.
use super::tree;

#[test]
fn runner_requires_exact_visible_request_and_opt_in() {
    assert_eq!(offline_acceptance_entry(&[]), 0);
    let mode = |args: &[&str], opt_in| {
        entry_mode(
            &args
                .iter()
                .map(std::ffi::OsString::from)
                .collect::<Vec<_>>(),
            opt_in,
        )
    };
    assert_eq!(mode(&["--visible"], false), EntryMode::Denied);
    assert_eq!(mode(&["--visible"], true), EntryMode::Visible);
    assert_eq!(
        mode(&["--visible", "--skip", "anything"], true),
        EntryMode::Denied
    );
    assert_eq!(mode(&["--preflight"], false), EntryMode::Preflight);
    for args in [
        &[][..],
        &["--skip", "catalog::tests::test_catalog_download"][..],
        &["--list"][..],
        &["--ignored"][..],
    ] {
        assert_eq!(mode(args, false), EntryMode::Skip);
        assert_eq!(mode(args, true), EntryMode::Skip);
    }
}

#[test]
fn runner_preflight_is_confined_and_leaves_environment_and_unrelated_data_untouched() {
    let environment = [
        std::env::var_os("APPDATA"),
        std::env::var_os("LOCALAPPDATA"),
    ];
    let harness = Acceptance::create();
    let scope = harness.fixture.temp.path().to_path_buf();
    let before = tree(&scope);
    let report = preflight(&harness);
    assert_eq!(report["native_window_started"], false);
    assert_eq!(report["no_self_update"], true);
    assert_eq!(tree(&scope), before);
    assert_eq!(
        [
            std::env::var_os("APPDATA"),
            std::env::var_os("LOCALAPPDATA")
        ],
        environment
    );
    drop(harness);
    assert!(!scope.exists());
}

async fn execute(harness: &mut Acceptance, command: Command<Event>) {
    let mut pending = command.futures();
    for _ in 0..8 {
        if pending.is_empty() {
            return;
        }
        assert_eq!(pending.len(), 1);
        pending = harness.handle(pending.remove(0).await).futures();
    }
    panic!("unexpected fixture command loop");
}
async fn reached(gate: &Gate, point: Point) {
    async_std::future::timeout(Duration::from_secs(10), async {
        while gate.reached() != Some(point) {
            async_std::task::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
}

#[test]
fn wrapper_rejects_external_scope_and_provider_messages_without_commands() {
    let mut h = Acceptance::create();
    let before = tree(h.fixture.temp.path());
    for m in [
        Message::RootInputChanged("C:/not-a-fixture".into()),
        Message::OpenRoot,
        Message::RepositoryChanged("external/repository".into()),
        Message::CheckRelease,
        Message::ReceiptIdChanged("unverified".into()),
        Message::InspectRecovery,
    ] {
        assert!(h.handle(Event::App(m)).futures().is_empty());
    }
    drop(h.view());
    assert_eq!(tree(h.fixture.temp.path()), before);
    assert!(h.fixture.app.model.release_worker.is_none());
    assert!(h.fixture.app.model.install.job.is_none());
    assert_eq!(h.fixture.app.model.root, h.fixture.temp.path().join("wow"));
}

#[test]
fn preparation_gate_cancel_releases_worker_and_preserves_all_fixture_data() {
    async_std::task::block_on(async {
        let mut h = Acceptance::create();
        let before = tree(&h.fixture.addons);
        let data = tree(&h.fixture.data);
        h.handle(Event::FixtureRelease(false));
        h.handle(Event::Hold(Point::Preparation));
        let mut futures = h
            .handle(Event::App(Message::Install(InstallMessage::Prepare(false))))
            .futures();
        let task = async_std::task::spawn(futures.remove(0));
        reached(&h.gate, Point::Preparation).await;
        let request = h.fixture.app.model.install.job.as_ref().unwrap().clone();
        let cancel = h.handle(Event::App(Message::Install(InstallMessage::Cancel(
            request,
        ))));
        execute(&mut h, cancel).await;
        let message = task.await;
        let command = h.handle(message);
        execute(&mut h, command).await;
        assert!(h.fixture.app.model.install.job.is_none());
        assert!(h.fixture.app.model.install.ready.is_none());
        assert!(fs::read_dir(&h.fixture.cache).unwrap().next().is_none());
        assert_eq!(tree(&h.fixture.addons), before);
        assert_eq!(tree(&h.fixture.data), data);
    });
}

#[test]
fn actual_worker_preapply_and_postapply_gates_defer_close_and_finish_matching_work() {
    async_std::task::block_on(async {
        for point in [Point::BeforeApply, Point::PostInspection] {
            let mut h = Acceptance::create();
            h.handle(Event::FixtureRelease(false));
            let prepare = h.handle(Event::App(Message::Install(InstallMessage::Prepare(false))));
            execute(&mut h, prepare).await;
            let request = h
                .fixture
                .app
                .model
                .install
                .ready
                .as_ref()
                .unwrap()
                .request
                .clone();
            h.handle(Event::App(Message::Install(InstallMessage::Ack(
                request.clone(),
                "APPLY".into(),
            ))));
            h.handle(Event::Hold(point));
            let mut futures = h
                .handle(Event::App(Message::Install(InstallMessage::Apply(request))))
                .futures();
            let task = async_std::task::spawn(futures.remove(0));
            reached(&h.gate, point).await;
            assert!(h.fixture.app.model.restoring());
            assert_eq!(
                h.fixture.addons.join("Alpha").exists(),
                point == Point::PostInspection
            );
            h.handle(Event::App(Message::CloseRequested));
            h.handle(Event::App(Message::CloseRequested));
            h.handle(Event::FixtureRelease(true));
            assert!(!h.should_exit());
            h.handle(Event::Continue);
            let message = task.await;
            let command = h.handle(message);
            execute(&mut h, command).await;
            assert!(h.should_exit());
            assert!(!h.fixture.app.model.restoring());
            assert!(h.fixture.addons.join("Alpha/Alpha.toc").is_file());
            assert_eq!(
                fs::read(h.fixture.addons.join("Keep/unrelated.lua")).unwrap(),
                b"unrelated user data"
            );
        }
    });
}

#[test]
fn wrapper_real_receipt_update_preview_and_apply_preserve_unrelated_data() {
    async_std::task::block_on(async {
        let mut h = Acceptance::create();
        let data = tree(&h.fixture.data);
        for update in [false, true] {
            h.handle(Event::FixtureRelease(update));
            let command = h.handle(Event::App(Message::Install(InstallMessage::Prepare(
                update,
            ))));
            execute(&mut h, command).await;
            let ready = h.fixture.app.model.install.ready.as_ref().unwrap();
            if update {
                assert_eq!(ready.added, ["New"]);
                assert_eq!(ready.replaced, ["Alpha"]);
                assert_eq!(ready.removed, ["Old"]);
            } else {
                assert_eq!(ready.added, ["Alpha", "Old"]);
            }
            let request = ready.request.clone();
            h.handle(Event::App(Message::Install(InstallMessage::Ack(
                request.clone(),
                "APPLY".into(),
            ))));
            let command = h.handle(Event::App(Message::Install(InstallMessage::Apply(request))));
            execute(&mut h, command).await;
            assert!(h
                .fixture
                .app
                .model
                .install
                .outcome
                .as_ref()
                .unwrap()
                .operation
                .is_ok());
            assert!(h.fixture.app.model.receipt.is_some());
        }
        assert!(!h.fixture.addons.join("Old").exists());
        assert_eq!(
            fs::read(h.fixture.addons.join("Keep/unrelated.lua")).unwrap(),
            b"unrelated user data"
        );
        assert_eq!(tree(&h.fixture.data), data);
    });
}

#[test]
fn wrapper_drop_wakes_prepare_worker_and_keeps_owned_root_alive_until_return() {
    async_std::task::block_on(async {
        let mut h = Acceptance::create();
        let root = h.fixture.temp.path().to_path_buf();
        h.handle(Event::FixtureRelease(false));
        h.handle(Event::Hold(Point::Preparation));
        let mut futures = h
            .handle(Event::App(Message::Install(InstallMessage::Prepare(false))))
            .futures();
        let task = async_std::task::spawn(futures.remove(0));
        reached(&h.gate, Point::Preparation).await;
        drop(h);
        let result = async_std::future::timeout(Duration::from_secs(10), task)
            .await
            .unwrap();
        assert!(matches!(
            result,
            Event::App(Message::Install(InstallMessage::Done(
                _,
                ResultMessage::Prepared(Err(_))
            )))
        ));
        assert!(!root.exists());
    });
}

#[test]
fn gate_timeout_fails_closed_without_an_unbounded_wait() {
    let gate = Gate::default();
    gate.arm(Point::BeforeApply);
    assert!(gate
        .wait_for(Point::BeforeApply, Duration::from_millis(1))
        .is_err());
    gate.close();
    assert!(gate.wait(Point::Preparation).is_err());
}

#[test]
fn wrapper_drop_keeps_apply_scope_alive_through_every_post_read() {
    async_std::task::block_on(async {
        for point in [Point::BeforeApply, Point::PostInspection] {
            let mut h = Acceptance::create();
            let root = h.fixture.temp.path().to_path_buf();
            h.handle(Event::FixtureRelease(false));
            let prepare = h.handle(Event::App(Message::Install(InstallMessage::Prepare(false))));
            execute(&mut h, prepare).await;
            let request = h
                .fixture
                .app
                .model
                .install
                .ready
                .as_ref()
                .unwrap()
                .request
                .clone();
            h.handle(Event::App(Message::Install(InstallMessage::Ack(
                request.clone(),
                "APPLY".into(),
            ))));
            h.handle(Event::Hold(point));
            let mut futures = h
                .handle(Event::App(Message::Install(InstallMessage::Apply(request))))
                .futures();
            let task = async_std::task::spawn(futures.remove(0));
            reached(&h.gate, point).await;
            drop(h);
            let message = async_std::future::timeout(Duration::from_secs(10), task)
                .await
                .unwrap();
            let outcome = match message {
                Event::App(Message::Install(InstallMessage::Done(
                    _,
                    ResultMessage::Applied(outcome),
                ))) => outcome,
                _ => panic!("wrong worker result"),
            };
            assert_eq!(outcome.operation.is_ok(), point == Point::PostInspection);
            assert!(
                outcome.inventory.is_ok(),
                "fixture must survive all post-reads"
            );
            assert!(outcome.recovery.is_ok());
            if point == Point::PostInspection {
                assert!(outcome.receipt.as_ref().unwrap().is_ok());
            }
            assert!(
                !root.exists(),
                "last worker releases only its disposable scope"
            );
        }
    });
}
