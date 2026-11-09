//! Explicit installation lifecycle; modified 2026-11-06, original GPLv3 applies.
include!("install_runtime.rs");

#[cfg(all(test, debug_assertions, feature = "fixture-tests"))]
#[path = "install_tests.rs"]
mod adversarial_tests;

#[cfg(all(test, debug_assertions, feature = "fixture-tests"))]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn explicit_fresh_command_previews_then_consumes_apply_and_publishes_receipt() {
        async_std::task::block_on(async {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path().join("wow");
            let addons = root.join("_retail_/Interface/AddOns");
            fs::create_dir_all(&addons).unwrap();
            fs::write(
                root.join(".build.info"),
                "Active!DEC:1|Product!STRING:0|Version!STRING:0\n1|wow|12.1.5.70575\n",
            )
            .unwrap();
            let (mut app, command) = Preview::new(Flags {
                root,
                native_check: false,
            });
            let message = command.futures().remove(0).await;
            assert!(app.dispatch(message).futures().is_empty());
            app.dispatch(Message::Select(Product::Retail));
            app.dispatch(Message::RepositoryChanged("fixture/addon".into()));
            let request = app.model.begin_release().unwrap();
            let (candidate, archive) = ajour_core::github_releases::fixture_support::package(
                app.model.installation().unwrap(),
                "fixture/addon",
                "v2",
                &[
                    ("Bar/Bar.toc", "## Interface: 120105\n## Version: v2\n"),
                    ("Bar/value.lua", "new"),
                ],
            )
            .unwrap();
            app.dispatch(Message::ReleaseCandidateDone(request, Arc::new(candidate)));
            let cache = temp.path().join("cache");
            fs::create_dir(&cache).unwrap();
            let command = app.dispatch_with_prepare(
                Message::Install(InstallMessage::Prepare(false)),
                |request| {
                    prepare_command_with(request, move |request| {
                        let installation = request
                            .inventory
                            .installations
                            .iter()
                            .find(|i| i.product == request.product)
                            .unwrap();
                        ajour_core::github_releases::fixture_support::prepare(
                            &request.candidate,
                            installation,
                            None,
                            &cache,
                            &archive,
                        )
                    })
                },
            );
            assert!(!addons.join("Bar").exists());
            let message = command.futures().remove(0).await;
            assert!(app.dispatch(message).futures().is_empty());
            let ready = app.model.install.ready.as_ref().unwrap();
            assert_eq!(ready.added, ["Bar"]);
            assert!(ready.replaced.is_empty() && ready.removed.is_empty());
            let preview = ready.request.clone();
            app.dispatch(Message::Install(InstallMessage::Ack(
                preview.clone(),
                "APPLY".into(),
            )));
            let command = app.dispatch(Message::Install(InstallMessage::Apply(preview.clone())));
            assert!(app.model.restoring());
            assert!(app
                .dispatch(Message::Install(InstallMessage::Apply(preview)))
                .futures()
                .is_empty());
            let message = command.futures().remove(0).await;
            assert!(app.dispatch(message).futures().is_empty());
            assert!(!app.model.restoring());
            assert_eq!(fs::read(addons.join("Bar/value.lua")).unwrap(), b"new");
            assert!(app.model.receipt.is_some());
            assert!(app
                .model
                .install
                .outcome
                .as_ref()
                .unwrap()
                .operation
                .is_ok());
        });
    }
}
