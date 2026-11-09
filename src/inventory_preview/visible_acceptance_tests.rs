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
    assert_eq!(mode(&["--recovery-preflight"], false), EntryMode::Preflight);
    assert_eq!(mode(&["--recovery-visible"], false), EntryMode::Denied);
    assert_eq!(mode(&["--recovery-visible"], true), EntryMode::Visible);
    assert_eq!(
        mode(&["--recovery-visible", "--skip", "anything"], true),
        EntryMode::Denied
    );
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

async fn recovery_plan(h: &mut Acceptance) -> RecoveryPreview {
    let before = tree(h.fixture.temp.path());
    let command = h.handle(Event::App(Message::InspectRecovery));
    execute(h, command).await;
    assert!(h.fixture.app.model.recovery_error.is_none());
    assert_ne!(
        h.fixture
            .app
            .model
            .recovery_inspection
            .as_ref()
            .unwrap()
            .phase,
        "none"
    );
    let command = h.handle(Event::App(Message::PreviewRecovery));
    execute(h, command).await;
    assert!(h.fixture.app.model.recovery_error.is_none());
    let preview = h.fixture.app.model.recovery_preview.clone().unwrap();
    assert_eq!(preview.approval.originals(), ["Bar", "Foo"]);
    // The checked plan names every affected destination, including Bar which
    // is currently absent. It authorizes moving these aside only if present.
    assert_eq!(
        preview.approval.move_aside_if_present(),
        ["Bar", "Foo", "New"]
    );
    drop(h.view());
    assert_eq!(tree(h.fixture.temp.path()), before);
    preview
}

fn confirm_recovery(h: &mut Acceptance, preview: &RecoveryPreview) -> Command<Event> {
    h.handle(Event::App(Message::RecoveryAckChanged(
        preview.clone(),
        "RESTORE".into(),
    )));
    h.handle(Event::App(Message::RestoreRecovery(preview.clone())))
}

#[test]
fn recovery_wrapper_cancel_and_restore_use_checked_commands_and_preserve_all_versions() {
    async_std::task::block_on(async {
        let mut h = Acceptance::create_recovery();
        let unrelated = tree(&h.fixture.addons.join("Keep"));
        let data = tree(&h.fixture.data);
        let saved = tree(&h.fixture.app.model.root.join("_retail_/WTF"));
        let before = tree(h.fixture.temp.path());
        drop(h.view());
        assert_eq!(tree(h.fixture.temp.path()), before);
        let preview = recovery_plan(&mut h).await;
        h.handle(Event::App(Message::RestoreRecovery(preview.clone())));
        assert!(h.fixture.app.model.recovery_job.is_none());
        h.handle(Event::App(Message::CancelRecoveryPreview(preview.clone())));
        assert_eq!(tree(h.fixture.temp.path()), before);
        assert!(confirm_recovery(&mut h, &preview).futures().is_empty());
        let preview = recovery_plan(&mut h).await;
        let command = confirm_recovery(&mut h, &preview);
        assert!(confirm_recovery(&mut h, &preview).futures().is_empty());
        execute(&mut h, command).await;
        assert!(matches!(h.fixture.app.model.recovery_outcome, Some(Ok(_))));
        assert!(h.fixture.app.model.recovery_post_error.is_none());
        for (path, bytes) in [
            ("Foo/old.lua", b"original".as_slice()),
            ("Bar/old.lua", b"original-bar".as_slice()),
            (
                ".patchloom-restore/backup/Foo/new.lua",
                b"replacement".as_slice(),
            ),
            (
                ".patchloom-restore/backup/New/new.lua",
                b"additional".as_slice(),
            ),
        ] {
            assert_eq!(fs::read(h.fixture.addons.join(path)).unwrap(), bytes);
        }
        assert_eq!(tree(&h.fixture.addons.join("Keep")), unrelated);
        assert_eq!(tree(&h.fixture.data), data);
        assert_eq!(tree(&h.fixture.app.model.root.join("_retail_/WTF")), saved);
        assert!(h
            .fixture
            .addons
            .join(".patchloom-install/plan.yml")
            .is_file());
        assert!(h
            .fixture
            .addons
            .join(".patchloom-restore/committed")
            .is_file());
        drop(h.view());
    });
}

#[test]
fn recovery_wrapper_refuses_changed_content_without_moving_any_folder() {
    async_std::task::block_on(async {
        for path in [
            "Foo/new.lua",
            ".patchloom-install/backup/Foo/old.lua",
            ".patchloom-install/plan.yml",
        ] {
            let mut h = Acceptance::create_recovery();
            let preview = recovery_plan(&mut h).await;
            fs::write(h.fixture.addons.join(path), b"changed after preview").unwrap();
            let before = tree(h.fixture.temp.path());
            let command = confirm_recovery(&mut h, &preview);
            execute(&mut h, command).await;
            // Valid-but-changed state is a typed refusal, malformed state an
            // error. Neither may restore or move any fixture folder.
            assert!(matches!(
                h.fixture.app.model.recovery_outcome,
                Some(Ok(ajour_core::fs::RecoveryRestoreOutcome::Changed)) | Some(Err(_))
            ));
            assert_eq!(tree(h.fixture.temp.path()), before);
            assert!(!h.fixture.app.model.restoring());
            drop(h.view());
        }
    });
}

#[test]
fn recovery_wrapper_gates_keep_capacity_until_matching_completion_after_close() {
    async_std::task::block_on(async {
        for point in [Point::BeforeApply, Point::PostInspection] {
            let mut h = Acceptance::create_recovery();
            let preview = recovery_plan(&mut h).await;
            h.handle(Event::Hold(point));
            let mut futures = confirm_recovery(&mut h, &preview).futures();
            assert_eq!(futures.len(), 1);
            let request = h.fixture.app.model.recovery_job.clone().unwrap();
            let task = async_std::task::spawn(futures.remove(0));
            reached(&h.gate, point).await;
            assert_eq!(
                h.fixture.addons.join("Foo/old.lua").is_file(),
                point == Point::PostInspection
            );
            h.handle(Event::App(Message::CloseRequested));
            h.handle(Event::App(Message::CloseRequested));
            assert!(!h.should_exit());
            assert!(confirm_recovery(&mut h, &preview).futures().is_empty());
            let mut foreign = request.clone();
            foreign.token = Arc::new(());
            h.handle(Event::App(Message::RecoveryDone(
                foreign,
                RecoveryResult::Restore {
                    outcome: Err("foreign".into()),
                    inspection: Err("foreign".into()),
                },
            )));
            h.handle(Event::App(Message::RecoveryDone(
                request,
                RecoveryResult::Inspection(Err("wrong action".into())),
            )));
            assert!(!h.should_exit());
            h.handle(Event::Continue);
            let message = task.await;
            let duplicate = message.clone();
            let command = h.handle(message);
            execute(&mut h, command).await;
            assert!(h.should_exit());
            assert!(!h.fixture.app.model.restoring());
            assert!(matches!(h.fixture.app.model.recovery_outcome, Some(Ok(_))));
            let after = tree(h.fixture.temp.path());
            assert!(h.handle(duplicate).futures().is_empty());
            assert_eq!(tree(h.fixture.temp.path()), after);
        }
    });
}

#[test]
fn recovery_wrapper_preflight_and_rejected_messages_never_change_scope_or_environment() {
    let environment = [
        std::env::var_os("APPDATA"),
        std::env::var_os("LOCALAPPDATA"),
    ];
    let mut h = Acceptance::create_recovery();
    let scope = h.fixture.temp.path().to_path_buf();
    let before = tree(&scope);
    for message in [
        Message::RootInputChanged("C:/outside-fixture".into()),
        Message::OpenRoot,
        Message::CheckRelease,
        Message::RepositoryChanged("external/provider".into()),
        Message::Install(InstallMessage::Prepare(false)),
        Message::Rescan,
    ] {
        assert!(h.handle(Event::App(message)).futures().is_empty());
    }
    h.handle(Event::FixtureRelease(false));
    let report = preflight(&h);
    assert_eq!(report["scenario"], "recovery");
    assert_eq!(report["native_window_started"], false);
    drop(h.view());
    assert_eq!(tree(&scope), before);
    assert_eq!(
        [
            std::env::var_os("APPDATA"),
            std::env::var_os("LOCALAPPDATA")
        ],
        environment
    );
    drop(h);
    assert!(!scope.exists());
}

#[test]
fn recovery_wrapper_drop_wakes_both_gates_and_retains_scope_through_postinspection() {
    async_std::task::block_on(async {
        for point in [Point::BeforeApply, Point::PostInspection] {
            let mut h = Acceptance::create_recovery();
            let scope = h.fixture.temp.path().to_path_buf();
            let preview = recovery_plan(&mut h).await;
            h.handle(Event::Hold(point));
            let mut futures = confirm_recovery(&mut h, &preview).futures();
            let task = async_std::task::spawn(futures.remove(0));
            reached(&h.gate, point).await;
            drop(h);
            let result = async_std::future::timeout(Duration::from_secs(10), task)
                .await
                .unwrap();
            match result {
                Event::App(Message::RecoveryDone(
                    _,
                    RecoveryResult::Restore {
                        outcome,
                        inspection,
                    },
                )) => {
                    assert_eq!(outcome.is_ok(), point == Point::PostInspection);
                    assert!(
                        inspection.is_ok(),
                        "post-inspection must execute even on drop/error"
                    );
                }
                _ => panic!("wrong recovery completion"),
            }
            drop(preview);
            assert!(!scope.exists());
        }
    });
}

#[test]
fn recovery_wrapper_stale_matching_completion_releases_capacity_without_publication() {
    async_std::task::block_on(async {
        let mut h = Acceptance::create_recovery();
        let preview = recovery_plan(&mut h).await;
        h.handle(Event::Hold(Point::BeforeApply));
        let mut futures = confirm_recovery(&mut h, &preview).futures();
        let task = async_std::task::spawn(futures.remove(0));
        reached(&h.gate, Point::BeforeApply).await;
        // Fault-injected logical staleness, not a reachable root-edit action:
        // production deliberately refuses invalidation while restoring.
        let generation = h.fixture.app.model.generation.clone();
        h.fixture.app.model.generation = Arc::new(());
        assert!(!Arc::ptr_eq(&generation, &h.fixture.app.model.generation));
        assert!(h.fixture.app.model.recovery_job.is_some());
        assert!(h.fixture.app.model.receipt_worker.is_some());
        h.handle(Event::App(Message::CloseRequested));
        assert!(!h.should_exit());
        h.handle(Event::Continue);
        let command = h.handle(task.await);
        execute(&mut h, command).await;
        assert!(h.should_exit());
        assert!(!h.fixture.app.model.restoring());
        assert!(h.fixture.app.model.recovery_outcome.is_none());
        assert!(h.fixture.addons.join("Foo/old.lua").is_file());
    });
}

#[test]
fn recovery_wrapper_keeps_success_when_required_postinspection_fails() {
    async_std::task::block_on(async {
        let mut h = Acceptance::create_recovery();
        let preview = recovery_plan(&mut h).await;
        h.handle(Event::Hold(Point::PostInspection));
        let mut futures = confirm_recovery(&mut h, &preview).futures();
        let task = async_std::task::spawn(futures.remove(0));
        reached(&h.gate, Point::PostInspection).await;
        assert!(h.fixture.addons.join("Foo/old.lua").is_file());
        fs::write(
            h.fixture.addons.join(".patchloom-install/plan.yml"),
            b"invalid journal after commit",
        )
        .unwrap();
        h.handle(Event::App(Message::CloseRequested));
        assert!(!h.should_exit());
        h.handle(Event::Continue);
        let command = h.handle(task.await);
        execute(&mut h, command).await;
        assert!(h.should_exit());
        assert!(matches!(h.fixture.app.model.recovery_outcome, Some(Ok(_))));
        assert!(h.fixture.app.model.recovery_post_error.is_some());
        drop(h.view());
    });
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
