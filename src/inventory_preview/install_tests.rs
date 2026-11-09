//! Headless install lifecycle fixtures; 2026-11-06, original GPLv3 applies.
use super::*;
use std::fs;

mod acceptance {
    include!("visible_acceptance.rs");
    include!("visible_acceptance_tests.rs");
}
use acceptance::{drain, fixture, Fixture};

#[test]
fn real_clean_success_has_no_recovery_warning_but_retained_inspection_does() {
    async_std::task::block_on(async {
        let mut f = fixture().await;
        let bytes = checked(&mut f, "v2", &[("Bar/Bar.toc", "## Interface: 120105\n")]);
        let command = prepare(&mut f, bytes, false);
        drain(&mut f.app, command).await;
        let command = confirm(&mut f);
        drain(&mut f.app, command).await;
        let outcome = f.app.model.install.outcome.as_ref().unwrap();
        assert!(outcome.operation.is_ok());
        assert_eq!(outcome.recovery.as_ref().unwrap().phase, "none");
        assert!(
            outcome.recovery_warning().is_none(),
            "clean successful install must not render a retained-state warning"
        );
        let tx = f.addons.join(".patchloom-install");
        fs::create_dir(&tx).unwrap();
        fs::create_dir(tx.join("backup")).unwrap();
        fs::create_dir(tx.join("staged")).unwrap();
        fs::write(
            tx.join("plan.yml"),
            "version: 1\nprevious: []\nincoming:\n- Bar\n",
        )
        .unwrap();
        let inspection = ajour_core::fs::inspect_recovery(&f.addons).unwrap();
        assert_eq!(inspection.phase, "unmarked");
        let mut retained = (**outcome).clone();
        retained.recovery = Ok(Arc::new(inspection));
        assert!(retained.recovery_warning().is_some());
    });
}

#[test]
fn idle_close_never_admits_a_queued_install_and_last_drop_disposes_only_cache() {
    async_std::task::block_on(async {
        let mut f = fixture().await;
        let before = tree(&f.addons);
        let data = tree(&f.data);
        let bytes = checked(&mut f, "v2", &[("Bar/Bar.toc", "## Interface: 120105\n")]);
        let command = prepare(&mut f, bytes, false);
        drain(&mut f.app, command).await;
        let request = f.app.model.install.ready.as_ref().unwrap().request.clone();
        for event in [
            Message::CloseRequested,
            Message::Install(InstallMessage::Ack(request.clone(), "APPLY".into())),
            Message::Install(InstallMessage::Apply(request)),
            Message::CheckRelease,
        ] {
            assert!(f.app.dispatch(event).futures().is_empty());
            assert!(f.app.should_exit());
        }
        assert!(!f.app.model.restoring());
        assert_eq!(tree(&f.addons), before);
        // Model shutdown drops the last capability, not a user-folder handle.
        // Explicitly remove that model field here to observe its real Drop.
        drop(f.app.model.install.ready.take());
        async_std::future::timeout(std::time::Duration::from_secs(5), async {
            while fs::read_dir(&f.cache).unwrap().next().is_some() {
                async_std::task::sleep(std::time::Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(tree(&f.addons), before);
        assert_eq!(tree(&f.data), data);
    });
}

fn checked(f: &mut Fixture, version: &str, entries: &[(&str, &str)]) -> Vec<u8> {
    let command = f.app.dispatch(Message::CheckRelease);
    assert_eq!(command.futures().len(), 1); // do not poll real metadata HTTP
    let request = f.app.model.pending_release.clone().unwrap();
    let (candidate, bytes) = ajour_core::github_releases::fixture_support::package(
        f.app.model.installation().unwrap(),
        "fixture/addon",
        version,
        entries,
    )
    .unwrap();
    assert!(f
        .app
        .dispatch(Message::ReleaseCandidateDone(request, Arc::new(candidate)))
        .futures()
        .is_empty());
    bytes
}
fn prepare(f: &mut Fixture, bytes: Vec<u8>, update: bool) -> Command<Message> {
    let cache = f.cache.clone();
    f.app.dispatch_with_prepare(
        Message::Install(InstallMessage::Prepare(update)),
        move |r| {
            prepare_command_with(r, move |r| {
                let installation = r
                    .inventory
                    .installations
                    .iter()
                    .find(|i| i.product == r.product)
                    .unwrap();
                ajour_core::github_releases::fixture_support::prepare(
                    &r.candidate,
                    installation,
                    r.receipt.as_ref().map(|p| p.id.as_str()),
                    &cache,
                    &bytes,
                )
            })
        },
    )
}
fn confirm(f: &mut Fixture) -> Command<Message> {
    let r = f.app.model.install.ready.as_ref().unwrap().request.clone();
    assert!(f
        .app
        .dispatch(Message::Install(InstallMessage::Ack(
            r.clone(),
            "APPLY".into()
        )))
        .futures()
        .is_empty());
    f.app.dispatch(Message::Install(InstallMessage::Apply(r)))
}
pub(super) fn tree(root: &std::path::Path) -> std::collections::BTreeMap<PathBuf, Option<Vec<u8>>> {
    fn visit(
        base: &std::path::Path,
        p: &std::path::Path,
        out: &mut std::collections::BTreeMap<PathBuf, Option<Vec<u8>>>,
    ) {
        if p == base.join(".patchloom-install.lock") {
            return;
        }
        let m = fs::symlink_metadata(p).unwrap();
        assert!(!m.file_type().is_symlink());
        if m.is_dir() {
            out.insert(p.strip_prefix(base).unwrap().into(), None);
            for e in fs::read_dir(p).unwrap() {
                visit(base, &e.unwrap().path(), out);
            }
        } else {
            out.insert(
                p.strip_prefix(base).unwrap().into(),
                Some(fs::read(p).unwrap()),
            );
        }
    }
    let mut out = std::collections::BTreeMap::new();
    visit(root, root, &mut out);
    out
}

#[test]
fn receipt_update_runs_real_apply_and_publishes_exact_add_replace_remove() {
    async_std::task::block_on(async {
        let mut f = fixture().await;
        let data = tree(&f.data);
        let keep = tree(&f.addons.join("Keep"));
        let bytes = checked(
            &mut f,
            "v2",
            &[
                ("Foo/Foo.toc", "## Interface: 120105\n## Version: v2\n"),
                ("Gone/Gone.toc", "## Interface: 120105\n"),
            ],
        );
        let command = prepare(&mut f, bytes, false);
        drain(&mut f.app, command).await;
        let command = confirm(&mut f);
        drain(&mut f.app, command).await;
        let old = f.app.model.receipt_id.clone();
        let old_bytes = fs::read(
            f.addons
                .join(".patchloom-ownership")
                .join(format!("{old}.json")),
        )
        .unwrap();
        let bytes = checked(
            &mut f,
            "v3",
            &[
                ("Foo/Foo.toc", "## Interface: 120105\n## Version: v3\n"),
                ("Added/Added.toc", "## Interface: 120105\n"),
            ],
        );
        let command = prepare(&mut f, bytes, true);
        drain(&mut f.app, command).await;
        let ready = f.app.model.install.ready.as_ref().unwrap();
        assert_eq!(ready.added, ["Added"]);
        assert_eq!(ready.replaced, ["Foo"]);
        assert_eq!(ready.removed, ["Gone"]);
        assert!(f.addons.join("Gone").is_dir());
        assert!(!f.addons.join("Added").exists());
        let command = confirm(&mut f);
        drain(&mut f.app, command).await;
        assert!(f.addons.join("Added").is_dir());
        assert!(!f.addons.join("Gone").exists());
        assert_ne!(f.app.model.receipt_id, old);
        assert_eq!(
            f.app.model.receipt.as_ref().unwrap().folders,
            ["Added", "Foo"]
        );
        assert_eq!(
            fs::read(
                f.addons
                    .join(".patchloom-ownership")
                    .join(format!("{old}.json"))
            )
            .unwrap(),
            old_bytes
        );
        assert_eq!(tree(&f.data), data);
        assert_eq!(tree(&f.addons.join("Keep")), keep);
        assert!(fs::read_dir(&f.cache).unwrap().next().is_none());
        drop(f.app.view());
    });
}

#[test]
fn preparation_cancellation_and_scope_changes_wait_for_physical_disposal() {
    async_std::task::block_on(async {
        for change in 0..5 {
            let mut f = fixture().await;
            let before = tree(&f.addons);
            let data = tree(&f.data);
            let bytes = checked(&mut f, "v2", &[("Bar/Bar.toc", "## Interface: 120105\n")]);
            let command = prepare(&mut f, bytes, false);
            let request = f.app.model.install.job.as_ref().unwrap().clone();
            let invalidate = match change {
                0 => Message::Install(InstallMessage::Cancel(request.clone())),
                1 => Message::RepositoryChanged("different/addon".into()),
                2 => Message::ReceiptIdChanged("different".into()),
                3 => Message::RootInputChanged(
                    f.temp
                        .path()
                        .join("elsewhere")
                        .to_string_lossy()
                        .into_owned(),
                ),
                _ => Message::Rescan,
            };
            assert!(f.app.dispatch(invalidate).futures().is_empty());
            assert!(f.app.model.install.job.is_some());
            assert!(f.app.model.receipt_worker.is_some());
            assert!(f
                .app
                .dispatch(Message::Install(InstallMessage::Prepare(false)))
                .futures()
                .is_empty());
            let message = command.futures().remove(0).await;
            let disposal = f.app.dispatch(message);
            assert_eq!(
                f.app.model.install.job.as_ref().unwrap().phase,
                Phase::Dispose
            );
            assert!(!f.app.model.restoring());
            assert!(f.app.model.install.ready.is_none());
            drain(&mut f.app, disposal).await;
            assert!(f.app.model.install.job.is_none());
            assert!(f.app.model.receipt_worker.is_none());
            assert!(fs::read_dir(&f.cache).unwrap().next().is_none());
            assert_eq!(tree(&f.addons), before);
            assert_eq!(tree(&f.data), data);
        }
    });
}

#[test]
fn preview_cancel_and_mismatched_display_cannot_apply_or_leak_cache() {
    async_std::task::block_on(async {
        for corrupt in [false, true] {
            let mut f = fixture().await;
            let before = tree(&f.addons);
            let bytes = checked(&mut f, "v2", &[("Bar/Bar.toc", "## Interface: 120105\n")]);
            let command = prepare(&mut f, bytes, false);
            drain(&mut f.app, command).await;
            let r = f.app.model.install.ready.as_ref().unwrap().request.clone();
            let command = if corrupt {
                Arc::make_mut(f.app.model.install.ready.as_mut().unwrap()).added =
                    vec!["WrongDisplayedFolder".into()];
                confirm(&mut f)
            } else {
                f.app.dispatch(Message::Install(InstallMessage::Cancel(r)))
            };
            assert!(!f.app.model.restoring());
            drain(&mut f.app, command).await;
            assert!(fs::read_dir(&f.cache).unwrap().next().is_none());
            assert_eq!(tree(&f.addons), before);
            assert!(f.app.model.install.ready.is_none());
        }
    });
}

#[test]
fn cloned_and_foreign_completions_do_not_duplicate_or_release_capabilities() {
    async_std::task::block_on(async {
        let mut f = fixture().await;
        let bytes = checked(&mut f, "v2", &[("Bar/Bar.toc", "## Interface: 120105\n")]);
        let command = prepare(&mut f, bytes, false);
        let r = f.app.model.install.job.as_ref().unwrap().clone();
        let mut foreign = r.clone();
        foreign.token = Arc::new(());
        assert!(f
            .app
            .dispatch(Message::Install(InstallMessage::Done(
                foreign,
                ResultMessage::Disposed
            )))
            .futures()
            .is_empty());
        assert!(f.app.model.install.job.as_ref().unwrap().same(&r));
        let message = command.futures().remove(0).await;
        let duplicate = message.clone();
        assert!(f.app.dispatch(message).futures().is_empty());
        assert!(f.app.dispatch(duplicate).futures().is_empty());
        let preview = f.app.model.install.ready.as_ref().unwrap().request.clone();
        assert!(f
            .app
            .dispatch(Message::Install(InstallMessage::Ack(
                preview.clone(),
                "APPLYx".into()
            )))
            .futures()
            .is_empty());
        assert!(f
            .app
            .dispatch(Message::Install(InstallMessage::Apply(preview.clone())))
            .futures()
            .is_empty());
        let command = confirm(&mut f);
        assert!(f
            .app
            .dispatch(Message::Install(InstallMessage::Apply(preview)))
            .futures()
            .is_empty());
        drain(&mut f.app, command).await;
        assert!(f
            .app
            .model
            .install
            .outcome
            .as_ref()
            .unwrap()
            .operation
            .is_ok());
        assert_eq!(
            fs::read_dir(f.addons.join(".patchloom-ownership"))
                .unwrap()
                .count(),
            1
        );
    });
}

#[test]
fn close_defers_through_real_apply_and_post_reads_success_error_and_stale_delivery() {
    async_std::task::block_on(async {
        for (core_error, stale, post_error) in [
            (false, false, false),
            (false, true, false),
            (true, false, false),
            (true, true, false),
            (false, false, true),
        ] {
            let mut f = fixture().await;
            let keep = tree(&f.addons.join("Keep"));
            let data = tree(&f.data);
            let bytes = checked(
                &mut f,
                "v2",
                &[
                    ("Bar/Bar.toc", "## Interface: 120105\n"),
                    ("Bar/value.lua", "new"),
                ],
            );
            let command = prepare(&mut f, bytes, false);
            drain(&mut f.app, command).await;
            if core_error {
                fs::create_dir(f.addons.join("Bar")).unwrap();
                fs::write(
                    f.addons.join("Bar/Bar.toc"),
                    "## Interface: 120105\n## Version: unmanaged\n",
                )
                .unwrap();
            }
            let before = tree(&f.addons);
            let command = confirm(&mut f);
            let request = f.app.model.install.job.as_ref().unwrap().clone();
            assert!(f.app.model.restoring());
            for message in [
                Message::CloseRequested,
                Message::CloseRequested,
                Message::Rescan,
                Message::ReceiptIdChanged("wrong".into()),
                Message::RepositoryChanged("wrong/repo".into()),
                Message::Install(InstallMessage::Prepare(false)),
                Message::NativeCheckDone,
            ] {
                assert!(f.app.dispatch(message).futures().is_empty());
                assert!(!f.app.should_exit());
            }
            let mut foreign = request.clone();
            foreign.token = Arc::new(());
            assert!(f
                .app
                .dispatch(Message::Install(InstallMessage::Done(
                    foreign,
                    ResultMessage::Disposed
                )))
                .futures()
                .is_empty());
            assert!(f
                .app
                .dispatch(Message::Install(InstallMessage::Done(
                    request.clone(),
                    ResultMessage::Disposed
                )))
                .futures()
                .is_empty());
            assert!(!f.app.should_exit());
            if stale {
                f.app.model.generation = Arc::new(());
            }
            let mut message = command.futures().remove(0).await;
            if post_error {
                if let Message::Install(InstallMessage::Done(_, ResultMessage::Applied(result))) =
                    &mut message
                {
                    let value = Arc::make_mut(result);
                    assert!(value.operation.is_ok());
                    value.inventory =
                        Err("injected after real worker: inventory read failed".into());
                    value.receipt =
                        Some(Err("injected after real worker: receipt read failed".into()));
                    value.recovery = Err("injected after real worker: recovery read failed".into());
                } else {
                    panic!("expected completed real apply");
                }
            }
            let duplicate = message.clone();
            assert!(f.app.dispatch(message).futures().is_empty());
            assert!(f.app.should_exit());
            assert!(f.app.model.install.job.is_none());
            assert!(f.app.model.receipt_worker.is_none());
            assert!(f.app.dispatch(duplicate).futures().is_empty());
            assert!(f.app.should_exit());
            if stale {
                assert!(f.app.model.install.outcome.is_none());
            } else {
                let result = f.app.model.install.outcome.as_ref().unwrap();
                assert_eq!(result.operation.is_err(), core_error);
                if post_error {
                    assert!(result.operation.is_ok());
                    assert!(result.inventory.is_err());
                }
            }
            if core_error {
                assert_eq!(tree(&f.addons), before);
            } else {
                assert_eq!(fs::read(f.addons.join("Bar/value.lua")).unwrap(), b"new");
            }
            assert_eq!(tree(&f.addons.join("Keep")), keep);
            assert_eq!(tree(&f.data), data);
        }
    });
}

#[test]
fn update_requires_checked_matching_receipt_and_never_falls_back_to_fresh() {
    async_std::task::block_on(async {
        let mut f = fixture().await;
        checked(&mut f, "v2", &[("Keep/Keep.toc", "## Interface: 120105\n")]);
        let before = tree(&f.addons);
        assert!(f
            .app
            .dispatch(Message::Install(InstallMessage::Prepare(true)))
            .futures()
            .is_empty());
        f.app.dispatch(Message::ReceiptIdChanged("0".repeat(64)));
        let command = f.app.dispatch(Message::InspectReceipt);
        drain(&mut f.app, command).await;
        assert!(f.app.model.receipt.is_none());
        assert!(f
            .app
            .dispatch(Message::Install(InstallMessage::Prepare(true)))
            .futures()
            .is_empty());
        assert_eq!(tree(&f.addons), before);
        assert!(fs::read_dir(&f.cache).unwrap().next().is_none());
    });
}

#[test]
fn old_preview_events_cannot_confirm_a_new_download() {
    async_std::task::block_on(async {
        let mut f = fixture().await;
        let before = tree(&f.addons);
        let bytes = checked(&mut f, "v2", &[("Bar/Bar.toc", "## Interface: 120105\n")]);
        let command = prepare(&mut f, bytes.clone(), false);
        drain(&mut f.app, command).await;
        let old = f.app.model.install.ready.as_ref().unwrap().request.clone();
        let command = f
            .app
            .dispatch(Message::Install(InstallMessage::Cancel(old.clone())));
        drain(&mut f.app, command).await;
        let command = prepare(&mut f, bytes, false);
        drain(&mut f.app, command).await;
        for message in [
            InstallMessage::Ack(old.clone(), "APPLY".into()),
            InstallMessage::Page(old.clone(), usize::MAX),
            InstallMessage::Cancel(old.clone()),
            InstallMessage::Apply(old),
        ] {
            assert!(f
                .app
                .dispatch(Message::Install(message))
                .futures()
                .is_empty());
        }
        assert!(f.app.model.install.ready.is_some());
        assert!(f.app.model.install.ack.is_empty());
        assert_eq!(f.app.model.install.page, 0);
        let current = f.app.model.install.ready.as_ref().unwrap().request.clone();
        let command = f
            .app
            .dispatch(Message::Install(InstallMessage::Cancel(current)));
        drain(&mut f.app, command).await;
        assert_eq!(tree(&f.addons), before);
        assert!(fs::read_dir(&f.cache).unwrap().next().is_none());
    });
}

#[test]
fn stale_matching_rate_limit_preserves_download_cooldown_without_releasing_foreign_work() {
    async_std::task::block_on(async {
        let mut f = fixture().await;
        checked(&mut f, "v2", &[("Bar/Bar.toc", "## Interface: 120105\n")]);
        let request = f.app.model.begin_prepare_install(false).unwrap();
        let deadline = RetryDeadline::At(Instant::now() + std::time::Duration::from_secs(3600));
        let mut foreign = request.clone();
        foreign.token = Arc::new(());
        assert!(f
            .app
            .dispatch(Message::Install(InstallMessage::Done(
                foreign,
                ResultMessage::Prepared(Err(ReleaseFailure::RateLimited(deadline)))
            )))
            .futures()
            .is_empty());
        assert!(f.app.model.release_retry_at.is_none());
        assert!(f.app.model.install.job.is_some());
        f.app.dispatch(Message::ReceiptIdChanged("changed".into()));
        assert!(f
            .app
            .dispatch(Message::Install(InstallMessage::Done(
                request,
                ResultMessage::Prepared(Err(ReleaseFailure::RateLimited(deadline)))
            )))
            .futures()
            .is_empty());
        assert_eq!(f.app.model.release_retry_at, Some(deadline));
        assert!(f.app.model.install.job.is_none());
        assert!(f.app.dispatch(Message::CheckRelease).futures().is_empty());
        assert!(f
            .app
            .dispatch(Message::Install(InstallMessage::Prepare(false)))
            .futures()
            .is_empty());
        assert!(fs::read_dir(&f.cache).unwrap().next().is_none());
    });
}

#[test]
fn preview_pages_preserve_complete_unicode_folder_names_and_scope() {
    async_std::task::block_on(async {
        let mut f = fixture().await;
        let names: Vec<String> = (0..FOLDERS_SHOWN + 2)
            .map(|n| format!("{}{:04}Tail", "α".repeat(90), n))
            .collect();
        let entries: Vec<(String, String)> = names
            .iter()
            .map(|n| (format!("{n}/{n}.toc"), "## Interface: 120105\n".into()))
            .collect();
        let references: Vec<(&str, &str)> = entries
            .iter()
            .map(|(a, b)| (a.as_str(), b.as_str()))
            .collect();
        let bytes = checked(&mut f, "v2", &references);
        let command = prepare(&mut f, bytes, false);
        drain(&mut f.app, command).await;
        let ready = f.app.model.install.ready.as_ref().unwrap();
        assert_eq!(ready.added, names);
        assert_eq!(install_pages(ready), 2);
        let r = ready.request.clone();
        let escaped: String = names
            .last()
            .unwrap()
            .chars()
            .flat_map(char::escape_default)
            .collect();
        assert!(recovery_folder_page(&ready.added, 1).contains(&escaped));
        assert!(f
            .app
            .dispatch(Message::Install(InstallMessage::Page(
                r.clone(),
                usize::MAX
            )))
            .futures()
            .is_empty());
        assert_eq!(f.app.model.install.page, 1);
        drop(f.app.view());
        let command = f.app.dispatch(Message::Install(InstallMessage::Cancel(r)));
        drain(&mut f.app, command).await;
    });
}
