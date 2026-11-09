//! Patchloom inventory and explicit recovery, 2026-10-28; original GPLv3 applies.
//! Inventory and recovery inspection do not change files. Restoring originals
//! requires a validated preview and fresh confirmation for the selected scope.
mod install;
include!("inventory_preview/runtime.rs");

impl Model {
    fn refresh(&mut self) {
        if let Some(request) = self.begin_scan() {
            let result = inventory_snapshot(&request);
            self.complete_scan(request, result);
        }
    }
    fn needs_review(&self) -> bool {
        self.error.is_some()
            || self.inventory.as_ref().is_none_or(|report| {
                !report.issues.is_empty()
                    || report
                        .installations
                        .iter()
                        .any(|i| i.addons.iter().any(|a| a.issue.is_some()))
            })
    }
}

// Added 2026-10-28; original GPLv3 applies.
// Runs the actual Application constructor and local completion dispatch.
// Does not start a native application, executor, subscription or clipboard.
// Modified 2026-10-28; original GPLv3 applies.
// Runs the actual Application constructor and local completion dispatch.
// Does not start a native application, executor, subscription or clipboard.
fn check_startup(root: PathBuf, explicit_root: bool) -> crate::Result<()> {
    let empty_root = root.as_os_str().is_empty();
    let (mut app, command) = Preview::new(Flags {
        root,
        native_check: false,
    });
    let mut futures = command.futures();
    let initial_commands = futures.len();
    let expected_commands = if empty_root { 0 } else { 1 };

    let no_recovery_work = |model: &Model| {
        model.recovery_job.is_none()
            && model.pending_recovery.is_none()
            && model.recovery_inspection.is_none()
            && model.recovery_preview.is_none()
            && model.recovery_ack.is_empty()
            && model.recovery_page == 0
            && model.recovery_error.is_none()
            && model.recovery_outcome.is_none()
            && model.recovery_post_error.is_none()
    };

    if initial_commands != expected_commands
        || app.model.receipt_worker.is_some()
        || app.model.release_worker.is_some()
        || app.model.pending_release.is_some()
        || !no_recovery_work(&app.model)
    {
        anyhow::bail!("Unexpected work during safe startup");
    }

    if empty_root && explicit_root {
        // An explicitly supplied empty path must not discover the working
        // directory or count as a successful rootless landing.
        if !app.dispatch(Message::OpenRoot).futures().is_empty() {
            anyhow::bail!("Unexpected work for an empty WoW path");
        }
    }

    if initial_commands == 1 {
        let message = async_std::task::block_on(futures.remove(0));
        match message {
            Message::InventoryDone(request, result) => {
                let next = app.dispatch(Message::InventoryDone(request, result));
                if !next.futures().is_empty() {
                    anyhow::bail!("Unexpected follow-up work during safe startup");
                }
            }
            _ => anyhow::bail!("Unexpected completion during safe startup"),
        }
    }

    drop(app.view());
    if !no_recovery_work(&app.model)
        || app.model.receipt_worker.is_some()
        || app.model.release_worker.is_some()
        || app.model.pending_release.is_some()
    {
        anyhow::bail!("Unexpected recovery or provider work after startup");
    }

    serde_json::to_writer(
        std::io::stdout(),
        &serde_json::json!({
            "entry": "safe-startup",
            "initial_commands": initial_commands,
            "model": &app.model,
        }),
    )?;
    if explicit_root && app.model.needs_review() {
        anyhow::bail!("Some inventory items need review");
    }
    Ok(())
}

pub fn run(opts: &crate::cli::Opts) -> crate::Result<()> {
    if !cfg!(feature = "no-self-update") {
        anyhow::bail!("This development build requires the no-self-update feature");
    }
    if opts.command.is_some() || opts.data_directory.is_some() || opts.self_update_temp.is_some() {
        anyhow::bail!(
            "Preview cannot be combined with legacy commands, --data or --self-update-temp"
        );
    }
    if (opts.preview_check && opts.preview_window_check)
        || (opts.startup_check && (opts.preview_check || opts.preview_window_check))
    {
        anyhow::bail!("Choose one preview check mode");
    }
    if (opts.preview_check || opts.preview_window_check) && opts.preview.is_none() {
        anyhow::bail!("An explicit copied WoW path is required");
    }

    let root = opts.preview.clone().unwrap_or_default();
    if opts.startup_check {
        return check_startup(root, opts.preview.is_some());
    }
    if opts.preview_check {
        // Complete the headless snapshot without starting an executor.
        let mut model = Model::new(root);
        model.refresh();
        let mut app = Preview::from_model(model, false);
        // Construct the same view as the native entry; no window or executor is started.
        drop(app.view());
        serde_json::to_writer(std::io::stdout(), &app.model)?;
        if app.model.needs_review() {
            anyhow::bail!("Some inventory items need review");
        }
        return Ok(());
    }
    let settings = preview_settings(
        root,
        opts.preview_window_check,
        opts.antialiasing.unwrap_or(false),
    );
    Preview::run(settings).map_err(|e| anyhow::anyhow!(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn native_close_messages(
        app: &Preview,
        events: Vec<(iced_native::Event, iced_native::event::Status)>,
    ) -> Vec<Message> {
        use futures::StreamExt;
        let mut recipes = app.subscription().recipes();
        assert_eq!(recipes.len(), 1 + usize::from(app.native_check));
        async_std::task::block_on(
            recipes
                .remove(0)
                .stream(futures::stream::iter(events).boxed())
                .collect(),
        )
    }

    fn assert_passive_close_subscription(app: &Preview) {
        assert!(native_close_messages(
            app,
            vec![(
                iced_native::Event::Window(iced_native::window::Event::Focused),
                iced_native::event::Status::Ignored
            )]
        )
        .is_empty());
    }

    #[test]
    fn native_close_subscription_delivers_close_events_before_shutdown() {
        let mut app = Preview::from_model(Model::new(PathBuf::new()), false);
        let settings = preview_settings(PathBuf::new(), false, false);
        assert!(!settings.exit_on_close_request);
        assert_eq!(settings.window.size, (960, 680));
        let messages = native_close_messages(
            &app,
            vec![
                (
                    iced_native::Event::Window(iced_native::window::Event::Focused),
                    iced_native::event::Status::Ignored,
                ),
                (
                    iced_native::Event::Window(iced_native::window::Event::CloseRequested),
                    iced_native::event::Status::Ignored,
                ),
                (
                    iced_native::Event::Window(iced_native::window::Event::CloseRequested),
                    iced_native::event::Status::Captured,
                ),
            ],
        );
        assert_eq!(messages.len(), 2);
        for message in messages {
            assert!(matches!(&message, Message::CloseRequested));
            assert!(app.dispatch(message).futures().is_empty());
            assert!(app.should_exit());
        }
        for queued in [
            Message::Rescan,
            Message::CheckRelease,
            Message::InspectRecovery,
        ] {
            assert!(app.dispatch(queued).futures().is_empty());
        }
        assert!(app.model.recovery_job.is_none());
        assert!(app.model.release_worker.is_none());
    }

    #[test]
    fn native_close_preserves_real_hidden_check_timer() {
        use futures::StreamExt;
        let settings = preview_settings(PathBuf::new(), true, true);
        assert!(!settings.exit_on_close_request);
        assert!(settings.flags.native_check);
        assert!(settings.antialiasing);
        let mut app = Preview::from_model(Model::new(PathBuf::new()), true);
        assert_eq!(app.mode(), iced::window::Mode::Hidden);
        let mut recipes = app.subscription().recipes();
        assert_eq!(recipes.len(), 2);
        let mut timer = recipes
            .pop()
            .unwrap()
            .stream(futures::stream::empty().boxed());
        let message = async_std::task::block_on(async_std::future::timeout(
            std::time::Duration::from_secs(2),
            timer.next(),
        ))
        .unwrap()
        .unwrap();
        assert!(matches!(&message, Message::NativeCheckDone));
        assert!(app.dispatch(message).futures().is_empty());
        assert!(app.should_exit());
    }

    fn deliver(model: &mut Model, command: Command<Message>) -> bool {
        let mut futures = command.futures();
        assert_eq!(futures.len(), 1);
        match async_std::task::block_on(futures.remove(0)) {
            Message::InventoryDone(request, result) => model.complete_scan(request, result),
            Message::ReceiptDone(request, result) => model.complete_receipt(request, result),
            other => panic!("Unexpected worker message: {:?}", other),
        }
    }

    fn scanned_preview(root: PathBuf, native_check: bool) -> Preview {
        let (mut app, command) = Preview::new(Flags { root, native_check });
        assert!(deliver(&mut app.model, command));
        app.synchronize_choices();
        app
    }
    #[test]
    fn selection_never_falls_back_to_another_product_or_survives_refresh() {
        let temp = tempfile::tempdir().unwrap();
        let mut model = Model {
            root: temp.path().join("missing-root"),
            inventory: Some(Arc::new(Discovery {
                root: PathBuf::new(),
                installations: vec![],
                issues: vec!["unknown product".into()],
            })),
            error: None,
            selected: None,
            ..Model::new(PathBuf::new())
        };
        assert!(model.needs_review());
        model.select(Product::Retail);
        assert!(model.installation().is_none());
        model.selected = Some(Product::Retail);
        model.refresh();
        assert!(model.selected.is_none());
        assert!(model.error.is_some());
        assert!(model.inventory.is_none());
        let (app, command) = Preview::new(Flags {
            root: temp.path().join("another-missing-root"),
            native_check: false,
        });
        assert!(app.model.error.is_none());
        assert!(app.model.inventory.is_none());
        assert!(app.model.scan.is_some());
        assert_eq!(command.futures().len(), 1);
        assert_passive_close_subscription(&app);
    }

    #[test]
    fn real_products_selection_refresh_and_view_schedule_no_legacy_work() {
        let temp = tempfile::tempdir().unwrap();
        for folder in ["_retail_", "_classic_"] {
            std::fs::create_dir(temp.path().join(folder)).unwrap();
        }
        let info = temp.path().join(".build.info");
        std::fs::write(&info,"Active!DEC:1|Product!STRING:0|Version!STRING:0\n1|wow|12.1.5.70575\n1|wow_classic|5.5.4.70579\n").unwrap();
        let mut app = scanned_preview(temp.path().into(), false);
        assert_passive_close_subscription(&app);
        assert!(!app.model.needs_review());
        assert!(app.model.installation().is_none());
        app.model.select(Product::ClassicProgression);
        assert_eq!(
            app.model.installation().unwrap().product,
            Product::ClassicProgression
        );
        app.model.select(Product::Anniversary);
        assert_eq!(
            app.model.installation().unwrap().product,
            Product::ClassicProgression
        );
        drop(app.view());
        std::fs::write(
            &info,
            "Active!DEC:1|Product!STRING:0|Version!STRING:0\n1|wow_unknown|13.0.0.1\n",
        )
        .unwrap();
        app.model.refresh();
        app.synchronize_choices();
        assert!(app.model.needs_review());
        assert!(app.model.installation().is_none());
        assert!(app.choices.is_empty());
        drop(app.view());
        assert_passive_close_subscription(&app);
        let (smoke, command) = Preview::new(Flags {
            root: temp.path().into(),
            native_check: true,
        });
        assert_eq!(command.futures().len(), 1);
        assert_eq!(smoke.mode(), iced::window::Mode::Hidden);
    }

    fn receipt_model() -> (tempfile::TempDir, Model) {
        let temp = tempfile::tempdir().unwrap();
        for directory in ["_retail_", "_classic_"] {
            std::fs::create_dir_all(temp.path().join(directory).join("Interface").join("AddOns"))
                .unwrap();
        }
        std::fs::write(
            temp.path().join(".build.info"),
            "Active!DEC:1|Product!STRING:0|Version!STRING:0\n1|wow|12.1.5.70575\n1|wow_classic|5.5.4.70579\n",
        )
        .unwrap();
        let mut model = Model::new(temp.path().into());
        model.refresh();
        model.select(Product::Retail);
        model.set_receipt_id("a".repeat(64));
        (temp, model)
    }

    fn matching_receipt(request: &ReceiptRequest) -> Arc<ajour_core::fs::OwnershipPreview> {
        Arc::new(ajour_core::fs::OwnershipPreview {
            id: request.id.clone(),
            root: request.addons.clone(),
            source: serde_json::json!({
                "repository": "fixture/addon",
                "product": request.product,
            }),
            folders: vec!["Foo".into()],
        })
    }

    #[test]
    fn receipt_result_checks_product_and_preserves_reported_fields() {
        let (_temp, mut model) = receipt_model();
        let request = model.begin_receipt().unwrap();
        assert_eq!(
            request.addons,
            model
                .installation()
                .unwrap()
                .directory
                .join("Interface")
                .join("AddOns")
        );
        assert!(model.complete_receipt(request.clone(), Ok(matching_receipt(&request))));
        let preview = model.receipt.as_ref().unwrap();
        assert_eq!(preview.folders, vec!["Foo"]);
        assert_eq!(preview.source["repository"], "fixture/addon");
        assert!(model.receipt_error.is_none());
        let retry = model.begin_receipt().unwrap();
        let wrong = ajour_core::fs::OwnershipPreview {
            id: retry.id.clone(),
            root: retry.addons.clone(),
            source: serde_json::json!({"product": "ClassicEra"}),
            folders: vec!["Foo".into()],
        };
        assert!(model.complete_receipt(retry, Ok(Arc::new(wrong))));
        assert!(model.receipt.is_none());
        assert_eq!(
            model.receipt_error.as_deref(),
            Some("Ownership source/product mismatch")
        );
    }

    #[test]
    fn pending_receipts_are_invalidated_by_input_selection_rescan_and_root_changes() {
        for change in 0..4 {
            let (temp, mut model) = receipt_model();
            let request = model.begin_receipt().unwrap();
            for _ in 0..8 {
                assert!(model.begin_receipt().is_none());
            }
            match change {
                0 => {
                    model.set_receipt_id("b".repeat(64));
                    model.set_receipt_id(request.id.clone());
                }
                1 => {
                    model.select(Product::ClassicProgression);
                    model.select(Product::Retail);
                }
                2 => {
                    model.refresh();
                    model.select(Product::Retail);
                }
                _ => {
                    model.set_root(model.root.join("another-root"));
                    assert!(model.begin_scan().is_none());
                }
            }
            assert!(model.receipt.is_none());
            assert!(model.receipt_error.is_none());
            assert!(model.pending_receipt.is_none());
            assert!(model.receipt_worker.is_some());
            assert!(model.begin_receipt().is_none());
            let mut unrelated = request.clone();
            unrelated.token = Arc::new(());
            assert!(!model.complete_receipt(unrelated, Err("unrelated".into())));
            assert!(model.receipt_worker.is_some());
            assert!(!deliver(&mut model, receipt_command(request.clone())));
            assert!(model.receipt_worker.is_none());
            assert!(model.receipt.is_none());
            assert!(!model.complete_receipt(request.clone(), Ok(matching_receipt(&request))));
            if model.scan_requested {
                let scan = model.start_scan().unwrap();
                assert!(!model.complete_receipt(request, Err("duplicate".into())));
                assert!(model.scan.is_some());
                assert!(deliver(&mut model, scan_command(scan)));
            }
            model.set_root(temp.path().into());
            model.refresh();
            model.select(Product::Retail);
            assert!(model.begin_receipt().is_some());
        }
    }

    #[test]
    fn completed_receipts_are_cleared_before_new_work() {
        for change in 0..4 {
            let (_temp, mut model) = receipt_model();
            let request = model.begin_receipt().unwrap();
            assert!(model.complete_receipt(request.clone(), Ok(matching_receipt(&request))));
            match change {
                0 => model.set_receipt_id("b".repeat(64)),
                1 => model.select(Product::ClassicProgression),
                2 => {
                    assert!(model.begin_scan().is_some());
                }
                _ => {
                    model.set_root(model.root.join("another-root"));
                    assert!(model.begin_scan().is_some());
                }
            }
            assert!(model.receipt.is_none());
            assert!(model.receipt_error.is_none());
        }
    }

    #[test]
    fn receipt_capacity_rejects_duplicates_and_errors_allow_retry() {
        let (_temp, mut model) = receipt_model();
        let failure = model.begin_receipt().unwrap();
        assert!(model.begin_receipt().is_none());
        assert!(deliver(&mut model, receipt_command(failure.clone())));
        assert!(model.receipt.is_none());
        assert!(model.receipt_error.is_some());
        assert!(model.receipt_worker.is_none());
        let retry = model.begin_receipt().unwrap();
        assert!(model.receipt_error.is_none());
        assert!(!model.complete_receipt(failure, Err("late failure".into())));
        assert!(model
            .receipt_worker
            .as_ref()
            .is_some_and(|token| Arc::ptr_eq(token, &retry.token)));
        assert!(model.begin_receipt().is_none());
        assert!(model.complete_receipt(retry.clone(), Ok(matching_receipt(&retry))));
        assert!(!model.complete_receipt(retry, Err("duplicate".into())));
        assert!(model.receipt.is_some());
        assert!(model.receipt_worker.is_none());
    }

    #[test]
    fn inventory_completions_reject_old_requests_and_changed_roots() {
        let (_temp, mut model) = receipt_model();
        let original = model.root.clone();
        let older = model.begin_scan().unwrap();
        for _ in 0..8 {
            assert!(model.begin_scan().is_none());
            assert!(model.begin_receipt().is_none());
        }
        assert!(!deliver(&mut model, scan_command(older.clone())));
        assert!(model.scan.is_none());
        let newer = model.start_scan().unwrap();
        assert!(!model.complete_scan(older, Err("late failure".into())));
        assert!(model
            .scan
            .as_ref()
            .is_some_and(|token| Arc::ptr_eq(token, &newer.token)));
        assert!(deliver(&mut model, scan_command(newer.clone())));
        assert!(model.inventory.is_some());
        assert!(!model.complete_scan(newer, Err("duplicate".into())));
        let changed = model.begin_scan().unwrap();
        model.set_root(model.root.join("different-root"));
        assert!(!deliver(&mut model, scan_command(changed)));
        assert!(model.scan.is_none());
        let failing = model.begin_scan().unwrap();
        assert!(deliver(&mut model, scan_command(failing)));
        assert!(model.error.is_some());
        assert!(model.inventory.is_none());
        model.set_root(original);
        let retry = model.begin_scan().unwrap();
        assert!(deliver(&mut model, scan_command(retry)));
        assert!(model.error.is_none());
        assert!(model.inventory.is_some());
    }

    #[test]
    fn receipt_root_guard_rejects_a_changed_root_before_another_scan() {
        let (_temp, mut model) = receipt_model();
        let request = model.begin_receipt().unwrap();
        model.set_root(model.root.join("different-root"));
        assert!(!model.complete_receipt(request.clone(), Ok(matching_receipt(&request))));
        assert!(model.receipt.is_none());
    }

    #[test]
    fn receipt_display_is_unicode_safe_and_bounded() {
        assert_eq!(clip("界界界界", 3), "界界界...");
        assert_eq!(clip("界界界", 3), "界界界");
        assert_eq!(clip("", 3), "");
        // Large, nested, arbitrary source: only repository and product are shown.
        let big = serde_json::Value::Array(vec![serde_json::Value::from("x".repeat(1000)); 1000]);
        let source = serde_json::json!({
            "repository": "界".repeat(5000),
            "product": "Retail",
            "extra": {"nested": big},
        });
        let text = source_summary(&source);
        assert!(text.chars().count() < 200, "{}", text.chars().count());
        assert!(text.contains("(Retail)") && !text.contains("xxxx"));
        assert!(text.contains(&"界".repeat(SOURCE_FIELD_CHARS)));
        // Missing or non-string fields are reported, not hidden.
        assert_eq!(
            source_summary(&serde_json::json!({"repository": 7})),
            "Recorded source: not recorded (not recorded)"
        );
        assert_eq!(
            source_summary(&serde_json::Value::Null),
            "Recorded source: not recorded (not recorded)"
        );
        // Folder list: count and per-name length are both capped.
        let folders: Vec<String> = (0..10_000)
            .map(|i| format!("界{i}{}", "z".repeat(500)))
            .collect();
        let text = folder_summary(&folders);
        assert!(text.chars().count() < FOLDERS_SHOWN * (FOLDER_CHARS + 5) + 40);
        assert!(text.ends_with(", and 9980 more"));
        assert_eq!(folder_summary(&["Foo".into(), "Bar".into()]), "Foo, Bar");
    }

    // GUI: append inside the existing #[cfg(test)] mod tests containing
    // deliver() and scanned_preview(). No existing code changes.
    // Added 2026-10-28; original GPLv3 applies.

    #[test]
    #[ignore = "child of the explicit real-writer bridge parent; never run standalone"]
    fn real_writer_receipt_reaches_gui_completion_child() {
        use std::io::Write;

        let root = PathBuf::from(
            std::env::var_os("PATCHLOOM_GUI_BRIDGE_ROOT")
                .expect("Run through the core bridge parent"),
        );
        let id = std::env::var("PATCHLOOM_GUI_BRIDGE_ID")
            .expect("Missing installer-returned receipt ID");
        let source: serde_json::Value = serde_json::from_str(
            &std::env::var("PATCHLOOM_GUI_BRIDGE_SOURCE")
                .expect("Missing writer-source expectation"),
        )
        .unwrap();
        let ack = root.parent().unwrap().join("gui-bridge-complete");
        assert!(!ack.exists(), "Completion acknowledgement must be fresh");
        assert_eq!(source["repository"], "fixture/addon");
        assert_eq!(source["product"], "Retail");

        // Real discovery through the existing scan Command helper.
        let mut app = scanned_preview(root.clone(), false);
        assert!(app.model.error.is_none());
        assert!(!app.model.needs_review());
        app.model.select(Product::Retail);
        app.model.set_receipt_id(id.clone());

        let request = app.model.begin_receipt().unwrap();
        let addons = root.join("_retail_").join("Interface").join("AddOns");
        assert_eq!(request.root, root);
        assert_eq!(request.addons, addons);
        assert_eq!(request.product, Product::Retail);
        assert_eq!(request.id, id);
        assert!(app.model.pending_receipt.is_some());
        assert!(app.model.receipt_worker.is_some());

        // Existing real Command future -> spawn_blocking -> receipt_snapshot
        // -> inspect_ownership -> Message::ReceiptDone -> complete_receipt.
        assert!(deliver(&mut app.model, receipt_command(request)));

        assert!(app.model.receipt_error.is_none());
        assert!(app.model.pending_receipt.is_none());
        assert!(app.model.receipt_worker.is_none());
        let preview = app
            .model
            .receipt
            .as_ref()
            .expect("GUI success receipt missing");
        assert_eq!(preview.id, id);
        assert_eq!(preview.root, addons.canonicalize().unwrap());
        assert_eq!(preview.source, source);
        assert_eq!(preview.folders, vec!["Foo".to_owned()]);
        assert_eq!(folder_summary(&preview.folders), "Foo");
        assert_eq!(
            source_summary(&preview.source),
            "Recorded source: fixture/addon (Retail)",
        );

        // Construct the existing success view without a window or native executor.
        drop(app.view());

        // Outside the copied WoW tree; never replace an existing acknowledgement.
        let mut completed = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&ack)
            .unwrap();
        completed
            .write_all(b"writer-receipt/gui-completion/v1\n")
            .unwrap();
    }

    // Added 2026-10-28; original GPLv3 applies.
    #[test]
    fn receipt_view_stays_busy_after_id_or_product_invalidation() {
        for change_product in [false, true] {
            let (_temp, model) = receipt_model();
            let mut app = Preview::from_model(model, false);
            assert!(app.model.can_inspect_receipt());
            assert_eq!(app.model.receipt_status(), None);

            let request = app.model.begin_receipt().unwrap();
            assert!(!app.model.can_inspect_receipt());
            assert_eq!(app.model.receipt_status(), Some("Checking receipt..."));

            if change_product {
                app.model.select(Product::ClassicProgression);
            } else {
                app.model.set_receipt_id("b".repeat(64));
            }

            assert!(app.model.pending_receipt.is_none());
            assert!(app
                .model
                .receipt_worker
                .as_ref()
                .is_some_and(|token| Arc::ptr_eq(token, &request.token)));
            assert!(!app.model.can_inspect_receipt());
            assert_eq!(app.model.receipt_status(), Some("Checking receipt..."));
            assert!(app.model.begin_receipt().is_none());
            drop(app.view());

            // The invalidated result releases capacity but cannot populate
            // the newly selected receipt/product.
            assert!(!deliver(&mut app.model, receipt_command(request)));
            assert!(app.model.receipt_worker.is_none());
            assert!(app.model.receipt.is_none());
            assert!(app.model.receipt_error.is_none());
            assert_eq!(app.model.receipt_status(), None);
            assert!(app.model.can_inspect_receipt());
            drop(app.view());

            app.model.set_receipt_id(String::new());
            assert!(!app.model.can_inspect_receipt());
        }
    }

    #[test]
    fn scan_view_shows_queued_then_running_then_completed_status() {
        let (_temp, model) = receipt_model();
        let mut app = Preview::from_model(model, false);
        let receipt = app.model.begin_receipt().unwrap();

        assert!(app.model.begin_scan().is_none());
        app.synchronize_choices();
        assert!(app.model.inventory.is_none());
        assert!(app.model.scan.is_none());
        assert!(app.model.scan_requested);
        assert_eq!(
            app.model.scan_status(),
            Some("Waiting to read addon folders...")
        );
        assert_eq!(app.model.receipt_status(), Some("Checking receipt..."));
        assert!(!app.model.can_inspect_receipt());
        drop(app.view());

        assert!(!deliver(&mut app.model, receipt_command(receipt)));
        assert_eq!(app.model.receipt_status(), None);
        assert_eq!(
            app.model.scan_status(),
            Some("Waiting to read addon folders...")
        );
        assert!(!app.model.can_inspect_receipt());

        let scan = app.model.start_scan().unwrap();
        assert!(!app.model.scan_requested);
        assert_eq!(app.model.scan_status(), Some("Reading addon folders..."));
        assert!(!app.model.can_inspect_receipt());
        drop(app.view());

        assert!(deliver(&mut app.model, scan_command(scan)));
        app.synchronize_choices();
        assert_eq!(app.model.scan_status(), None);
        assert_eq!(app.model.receipt_status(), None);
        assert!(app.model.inventory.is_some());
        assert!(!app.model.can_inspect_receipt());
        app.model.select(Product::Retail);
        assert!(app.model.can_inspect_receipt());
        drop(app.view());
    }

    #[test]
    fn receipt_display_escapes_controls_within_existing_output_budgets() {
        assert_eq!(clip("界é🙂", 3), "界é🙂");
        assert_eq!(clip("界é🙂x", 3), "界é🙂...");
        assert_eq!(clip("", 0), "");
        assert_eq!(clip("x", 0), "...");
        assert_eq!(clip("A\n\u{202e}界", 40), r"A\u{a}\u{202e}界");

        // Never split an escape to fill the remaining budget.
        assert_eq!(clip("\u{202e}", 7), "...");
        assert_eq!(clip("\u{202e}", 8), r"\u{202e}");
        assert_eq!(clip("\u{202e}x", 8), r"\u{202e}...");

        let controls = [
            '\0', '\t', '\n', '\r', '\u{001b}', '\u{0085}', '\u{061c}', '\u{200e}', '\u{200f}',
            '\u{2028}', '\u{2029}', '\u{202a}', '\u{202b}', '\u{202c}', '\u{202d}', '\u{202e}',
            '\u{2066}', '\u{2067}', '\u{2068}', '\u{2069}',
        ];
        for ch in controls {
            let raw = ch.to_string();
            assert_eq!(clip(&raw, 20), ch.escape_unicode().to_string());
        }

        let raw = "界\n\u{202e}".repeat(1000);
        for limit in [SOURCE_FIELD_CHARS, FOLDER_CHARS, ERROR_CHARS] {
            let displayed = clip(&raw, limit);
            assert!(displayed.chars().count() <= limit + 3);
            assert!(displayed.starts_with("界"));
            assert!(displayed.ends_with("..."));
            assert!(!displayed.chars().any(char::is_control));
            assert!(!displayed.contains('\u{202e}'));
        }

        let source = serde_json::json!({
            "repository": raw.clone(),
            "product": "Retail",
        });
        let source_before = source.clone();
        let folders = vec![raw.clone()];
        let source_text = source_summary(&source);
        let folder_text = folder_summary(&folders);
        assert!(source_text.contains(r"\u{a}"));
        assert!(source_text.contains(r"\u{202e}"));
        assert!(source_text.ends_with(" (Retail)"));
        assert!(folder_text.chars().count() <= FOLDER_CHARS + 3);
        assert!(!folder_text.contains('\n'));
        assert!(!folder_text.contains('\u{202e}'));
        assert_eq!(source, source_before);
        assert_eq!(folders[0], raw);
    }

    // Added 2026-10-28; original GPLv3 applies.
    // Provider results below are injected model results, not HTTP responses.
    fn release_fixture() -> (tempfile::TempDir, Model) {
        let (temp, mut model) = receipt_model();
        model.set_repository("fixture/addon".into());
        (temp, model)
    }

    fn assert_no_release_work(model: &Model) {
        assert!(model.release_worker.is_none());
        assert!(model.pending_release.is_none());
        assert!(model.release_result.is_none());
        assert_eq!(model.release_status(), None);
    }

    #[test]
    fn release_work_is_not_started_by_construction_headless_or_local_actions() {
        let (temp, _) = receipt_model();
        let (mut app, command) = Preview::new(Flags {
            root: temp.path().into(),
            native_check: false,
        });
        assert_no_release_work(&app.model);

        // Existing deliver checks exactly one future and accepts only the
        // actual inventory/receipt messages. Drive only the startup scan.
        assert!(deliver(&mut app.model, command));
        app.synchronize_choices();
        assert_no_release_work(&app.model);

        app.model.select(Product::Retail);
        app.model.set_repository("fixture/addon".into());
        assert!(app.model.can_check_release());
        assert_no_release_work(&app.model);
        app.model.select(Product::ClassicProgression);
        assert_no_release_work(&app.model);
        app.model.set_receipt_id("b".repeat(64));
        assert_no_release_work(&app.model);
        drop(app.view());
        assert_no_release_work(&app.model);

        let scan = app.model.begin_scan().unwrap();
        app.synchronize_choices();
        assert_no_release_work(&app.model);
        assert!(deliver(&mut app.model, scan_command(scan)));
        app.synchronize_choices();
        assert_no_release_work(&app.model);

        // Follow the supplied headless branch's model/view/serialization
        // operations. This does not invoke run() or construct CLI options.
        let mut headless = Model::new(temp.path().into());
        headless.refresh();
        let mut headless = Preview::from_model(headless, false);
        drop(headless.view());
        let report = serde_json::to_value(&headless.model).unwrap();
        assert!(!headless.model.needs_review());
        assert!(report.get("repository").is_none());
        assert!(report.get("release_result").is_none());
        assert_no_release_work(&headless.model);
    }

    #[test]
    fn release_admission_busy_success_error_and_retry_are_explicit() {
        let (_temp, mut model) = release_fixture();
        let first = model.begin_release().unwrap();
        assert_eq!(model.release_status(), Some("Checking GitHub release..."));
        assert!(!model.can_check_release());
        for _ in 0..8 {
            assert!(model.begin_release().is_none());
        }

        // Construct and drop the actual provider future without polling it.
        // This checks Command shape, not HTTP transport or resolver success.
        assert_eq!(release_command(first.clone()).futures().len(), 1);

        assert!(model.complete_release(first.clone(), Ok("v2".into())));
        assert_eq!(model.release_status(), None);
        assert!(model.release_worker.is_none());
        assert!(model.pending_release.is_none());
        assert!(model.can_check_release());
        let summary = model.release_result.as_ref().unwrap().as_ref().unwrap();
        assert_eq!(summary.repository, "fixture/addon");
        assert_eq!(summary.product, Product::Retail);
        assert_eq!(summary.version, "v2");

        // A completed result remains visible until explicit admission or
        // invalidation. Rendering and unchanged input do not retry.
        model.set_repository("fixture/addon".into());
        let mut app = Preview::from_model(model, false);
        drop(app.view());
        assert!(app.model.release_worker.is_none());
        assert!(app.model.release_result.as_ref().unwrap().is_ok());

        let failed = app.model.begin_release().unwrap();
        assert!(app.model.release_result.is_none());
        assert!(app.model.complete_release(failed, Err("HTTP 429".into())));
        assert_eq!(
            app.model
                .release_result
                .as_ref()
                .unwrap()
                .as_ref()
                .unwrap_err(),
            "HTTP 429"
        );
        assert!(app.model.release_worker.is_none());
        assert!(app.model.can_check_release());
        drop(app.view());
        assert!(app.model.release_worker.is_none());
        assert!(app.model.pending_release.is_none());

        let retry = app.model.begin_release().unwrap();
        assert!(app.model.release_result.is_none());
        assert!(!app
            .model
            .complete_release(first, Err("old duplicate".into())));
        assert!(app
            .model
            .release_worker
            .as_ref()
            .is_some_and(|token| Arc::ptr_eq(token, &retry.token)));
        assert!(app.model.complete_release(retry, Ok("v3".into())));
        assert_eq!(
            app.model
                .release_result
                .as_ref()
                .unwrap()
                .as_ref()
                .unwrap()
                .version,
            "v3"
        );
    }

    #[test]
    fn release_invalidated_before_completion_cannot_publish() {
        for change in 0..4 {
            let (_temp, mut model) = release_fixture();
            let request = model.begin_release().unwrap();
            match change {
                0 => model.set_repository("other/addon".into()),
                1 => model.select(Product::ClassicProgression),
                2 => {
                    model.set_root(model.root.join("different-root"));
                    assert!(model.pending_release.is_none());
                }
                _ => {
                    // A local rescan can complete while provider capacity
                    // stays reserved. It never queues another provider job.
                    model.refresh();
                    model.select(Product::Retail);
                    assert!(!Arc::ptr_eq(
                        model.inventory.as_ref().unwrap(),
                        &request.inventory
                    ));
                }
            }
            assert!(model
                .release_worker
                .as_ref()
                .is_some_and(|token| Arc::ptr_eq(token, &request.token)));
            assert_eq!(model.release_status(), Some("Checking GitHub release..."));
            assert!(model.begin_release().is_none());
            assert!(!model.complete_release(request, Ok("stale".into())));
            assert_no_release_work(&model);
        }
    }

    #[test]
    fn release_results_are_cleared_by_input_product_and_rescan_transitions() {
        for failure in [false, true] {
            for change in 0..4 {
                let (_temp, mut model) = release_fixture();
                let request = model.begin_release().unwrap();
                let result = if failure {
                    Err("offline".into())
                } else {
                    Ok("v2".into())
                };
                assert!(model.complete_release(request, result));
                assert!(model.release_result.is_some());

                match change {
                    0 => model.set_repository("other/addon".into()),
                    1 => model.select(Product::ClassicProgression),
                    2 => {
                        model.set_root(model.root.join("different-root"));
                        model.refresh();
                        assert!(model.error.is_some());
                    }
                    _ => model.refresh(),
                }
                assert_no_release_work(&model);
            }
        }
    }

    #[test]
    fn release_aba_and_old_completions_cannot_release_a_new_worker() {
        for change in 0..4 {
            let (_temp, mut model) = release_fixture();
            let original_root = model.root.clone();
            let old = model.begin_release().unwrap();

            match change {
                0 => {
                    model.set_repository("other/addon".into());
                    model.set_repository(old.repository.clone());
                }
                1 => {
                    model.select(Product::ClassicProgression);
                    model.select(Product::Retail);
                }
                2 => {
                    model.refresh();
                    model.select(Product::Retail);
                }
                _ => {
                    model.set_root(original_root.join("different-root"));
                    model.refresh();
                    model.set_root(original_root);
                    model.refresh();
                    model.select(Product::Retail);
                }
            }

            assert_eq!(model.repository, old.repository);
            assert_eq!(model.selected, Some(old.product));
            assert_eq!(model.root, old.root);
            assert!(model.pending_release.is_none());
            assert!(model.begin_release().is_none());

            let mut unrelated = old.clone();
            unrelated.token = Arc::new(());
            assert!(!model.complete_release(unrelated, Ok("unrelated".into())));
            assert!(model
                .release_worker
                .as_ref()
                .is_some_and(|token| Arc::ptr_eq(token, &old.token)));

            assert!(!model.complete_release(old.clone(), Ok("ABA stale".into())));
            assert_no_release_work(&model);

            let current = model.begin_release().unwrap();
            assert!(!Arc::ptr_eq(&old.token, &current.token));
            assert!(!model.complete_release(old.clone(), Err("late".into())));
            assert!(model
                .release_worker
                .as_ref()
                .is_some_and(|token| Arc::ptr_eq(token, &current.token)));
            assert!(model
                .pending_release
                .as_ref()
                .is_some_and(|pending| Arc::ptr_eq(&pending.token, &current.token)));

            assert!(model.complete_release(current.clone(), Ok("current".into())));
            assert!(!model.complete_release(current, Err("duplicate".into())));
            assert!(!model.complete_release(old, Ok("older duplicate".into())));
            assert_eq!(
                model
                    .release_result
                    .as_ref()
                    .unwrap()
                    .as_ref()
                    .unwrap()
                    .version,
                "current"
            );
            assert!(model.release_worker.is_none());
        }
    }

    #[test]
    fn release_validation_uses_raw_repository_without_url_or_whitespace_fallback() {
        let (_temp, mut model) = release_fixture();
        let report = model.inventory.as_ref().unwrap().clone();
        for raw in [
            "",
            "owner",
            "/repo",
            "owner/",
            "owner/repo/extra",
            "owner//repo",
            "./repo",
            "../repo",
            "owner/.",
            "owner/..",
            " owner/repo",
            "owner/repo ",
            "owner/repo\n",
            "owner/\u{202e}repo",
            "ownér/repo",
            "https://github.com/owner/repo",
            "owner/repo?token=x",
            "owner/repo#fragment",
        ] {
            assert!(!valid_repository(raw), "{:?}", raw);
            model.set_repository(raw.into());
            assert_eq!(model.repository, raw);
            assert!(!model.can_check_release());
            assert!(model.begin_release().is_none());
            assert_no_release_work(&model);
            assert!(Arc::ptr_eq(model.inventory.as_ref().unwrap(), &report));
        }

        let maximum = format!("{}/{}", "a".repeat(100), "b".repeat(100));
        assert!(valid_repository(&maximum));
        let too_long = format!("{}/repo", "a".repeat(101));
        assert!(!valid_repository(&too_long));
        model.set_repository(too_long.clone());
        assert_eq!(model.repository, too_long);
        assert!(model.begin_release().is_none());

        for raw in ["fixture/addon", "Owner-1/repo_2.v3", &maximum] {
            assert!(valid_repository(raw));
            model.set_repository(raw.into());
            assert!(model.can_check_release());
            assert_no_release_work(&model);
        }

        model.selected = None;
        assert!(!model.can_check_release());
        assert!(model.begin_release().is_none());
    }

    #[test]
    fn release_summaries_are_bounded_control_safe_and_do_not_change_input() {
        let (_temp, mut model) = release_fixture();
        let raw = format!("界\n\u{202e}{}", "z".repeat(10_000));
        let repository = model.repository.clone();
        let request = model.begin_release().unwrap();
        assert!(model.complete_release(request, Ok(raw.clone())));
        let summary = model.release_result.as_ref().unwrap().as_ref().unwrap();
        assert_eq!(summary.version, clip(&raw, SOURCE_FIELD_CHARS));
        assert!(summary.version.chars().count() <= SOURCE_FIELD_CHARS + 3);
        assert!(summary.version.contains(r"\u{a}"));
        assert!(summary.version.contains(r"\u{202e}"));
        assert!(!summary.version.chars().any(char::is_control));
        assert!(!summary.version.contains('\u{202e}'));
        assert_eq!(summary.repository, repository);
        assert_eq!(model.repository, repository);

        let retry = model.begin_release().unwrap();
        assert!(model.complete_release(retry, Err(raw.clone())));
        let error = model.release_result.as_ref().unwrap().as_ref().unwrap_err();
        assert_eq!(error, &clip(&raw, ERROR_CHARS));
        assert!(error.chars().count() <= ERROR_CHARS + 3);
        assert!(!error.chars().any(char::is_control));
        assert!(!error.contains('\u{202e}'));
        assert_eq!(model.repository, repository);
        assert!(raw.contains('\n') && raw.contains('\u{202e}'));
    }

    #[test]
    fn release_completion_preserves_inventory_receipt_and_fixture_bytes() {
        let (temp, mut model) = release_fixture();
        let app_data = tempfile::tempdir().unwrap();
        let config = app_data.path().join("config-sentinel.json");
        std::fs::write(&config, b"{\"fixture\":\"unchanged\"}\n").unwrap();
        let addon = temp.path().join("_retail_/Interface/AddOns/Unowned");
        std::fs::create_dir_all(&addon).unwrap();
        let user_file = addon.join("keep.lua");
        std::fs::write(&user_file, b"fixture user bytes\n").unwrap();

        // Synthetic receipt only for this model-isolation assertion.
        // The existing real-writer paired bridge is preserved separately.
        let receipt_request = model.begin_receipt().unwrap();
        let receipt = matching_receipt(&receipt_request);
        assert!(model.complete_receipt(receipt_request, Ok(receipt.clone())));
        let inventory = model.inventory.as_ref().unwrap().clone();
        let inventory_bytes = serde_json::to_vec(inventory.as_ref()).unwrap();
        let receipt_id = model.receipt_id.clone();
        let paths = [temp.path().join(".build.info"), user_file, config];
        let before: Vec<Vec<u8>> = paths
            .iter()
            .map(|path| std::fs::read(path).unwrap())
            .collect();

        for result in [Err("offline / HTTP 403".into()), Ok("v2".into())] {
            let request = model.begin_release().unwrap();
            // Independent provider admission does not consume snapshot
            // capacity or invalidate an already displayed receipt.
            assert!(model.scan.is_none());
            assert!(model.receipt_worker.is_none());
            assert!(model.can_inspect_receipt());
            assert!(model.complete_release(request, result));
            assert!(Arc::ptr_eq(model.inventory.as_ref().unwrap(), &inventory));
            assert_eq!(
                serde_json::to_vec(model.inventory.as_deref().unwrap()).unwrap(),
                inventory_bytes
            );
            assert!(Arc::ptr_eq(model.receipt.as_ref().unwrap(), &receipt));
            assert_eq!(model.receipt_id, receipt_id);
            assert!(model.error.is_none());
            assert!(model.receipt_error.is_none());
            for (path, bytes) in paths.iter().zip(&before) {
                assert_eq!(&std::fs::read(path).unwrap(), bytes);
            }
        }

        // Receipt inspection can still be admitted after provider completion.
        let next_receipt = model.begin_receipt().unwrap();
        assert!(model.complete_receipt(next_receipt.clone(), Ok(matching_receipt(&next_receipt))));
    }

    // Added 2026-10-28; original GPLv3 applies.
    #[cfg(test)]
    mod rate_error_boundary_tests {
        use super::*;
        use ajour_core::error::DownloadError;
        use isahc::http::StatusCode;
        use std::time::Duration;

        #[test]
        fn question_mark_boxing_preserves_rate_type_and_wait_for_gui_conversion() {
            type Boxed = Box<dyn std::error::Error + Send + Sync>;

            for code in [StatusCode::TOO_MANY_REQUESTS, StatusCode::FORBIDDEN] {
                // Exercise the same concrete-error-to-box conversion used by `?`.
                let result: Result<(), Boxed> = (|| {
                    Err::<(), DownloadError>(DownloadError::RateLimited {
                        code,
                        retry_after: Duration::from_secs(120),
                    })?;
                    Ok(())
                })();
                let error = result.unwrap_err();

                match error.downcast_ref::<DownloadError>() {
                    Some(DownloadError::RateLimited {
                        code: actual,
                        retry_after,
                    }) => {
                        assert_eq!(*actual, code);
                        assert_eq!(*retry_after, Duration::from_secs(120));
                    }
                    other => panic!("typed rate metadata lost: {:?}", other),
                }

                // Bracket the real clock read; no sleep or exact timing assumption.
                let before = Instant::now();
                let failure = release_failure(error);
                let after = Instant::now();
                match failure {
                    ReleaseFailure::RateLimited(RetryDeadline::At(deadline)) => {
                        assert!(deadline >= before + Duration::from_secs(120));
                        assert!(deadline <= after + Duration::from_secs(120));
                    }
                    other => panic!("GUI lost typed rate outcome: {:?}", other),
                }
            }
        }

        #[test]
        fn status_or_rate_words_in_an_untyped_error_do_not_start_a_wait() {
            for code in [StatusCode::FORBIDDEN, StatusCode::TOO_MANY_REQUESTS] {
                let error = DownloadError::InvalidStatusCode {
                    code,
                    url: "RateLimited retry-after 3600".into(),
                };
                assert!(matches!(
                    release_failure(Box::new(error)),
                    ReleaseFailure::Other(_),
                ));
            }

            let error = std::io::Error::other("Rate limit response (429); retry-after 3600");
            assert!(matches!(
                release_failure(Box::new(error)),
                ReleaseFailure::Other(_),
            ));
        }

        #[test]
        fn checked_deadlines_preserve_long_waits_or_fail_closed() {
            let now = Instant::now();
            for wait in [
                Duration::from_secs(86_401),
                Duration::from_secs(172_800),
                Duration::from_secs(u64::MAX),
            ] {
                let actual = RetryDeadline::from_wait(now, wait);
                match now.checked_add(wait) {
                    Some(deadline) => {
                        assert_eq!(actual, RetryDeadline::At(deadline));
                        assert!(!actual.allows(now));
                        assert!(actual.allows(deadline));
                    }
                    None => {
                        assert_eq!(actual, RetryDeadline::Blocked);
                        assert!(!actual.allows(now));
                    }
                }
            }
            assert_eq!(
                RetryDeadline::from_wait(now, Duration::MAX),
                RetryDeadline::Blocked,
            );
        }

        #[test]
        fn deadline_merge_never_shortens_or_unblocks_a_wait() {
            let now = Instant::now();
            let short = RetryDeadline::At(now);
            let long = RetryDeadline::from_wait(now, Duration::from_secs(172_800));
            assert_eq!(short.max(long), long);
            assert_eq!(long.max(short), long);
            assert_eq!(long.max(RetryDeadline::Blocked), RetryDeadline::Blocked);
            assert_eq!(RetryDeadline::Blocked.max(long), RetryDeadline::Blocked);
            assert!(!RetryDeadline::Blocked.allows(now));
            assert!(short.allows(now));
        }

        #[test]
        fn boxed_unrepresentable_rate_wait_remains_typed_and_blocked() {
            let failure = release_failure(Box::new(DownloadError::RateLimited {
                code: StatusCode::TOO_MANY_REQUESTS,
                retry_after: Duration::MAX,
            }));
            assert!(matches!(
                failure,
                ReleaseFailure::RateLimited(RetryDeadline::Blocked),
            ));
        }
    }

    // Added 2026-10-28; original GPLv3 applies.
    mod rate_dispatch_tests {
        use super::*;
        use ajour_core::error::DownloadError;
        use isahc::http::StatusCode;
        use std::time::Duration;

        fn no_command(command: Command<Message>) {
            assert!(command.futures().is_empty());
        }

        fn app_fixture() -> (tempfile::TempDir, Preview) {
            let (temp, model) = release_fixture();
            (temp, Preview::from_model(model, false))
        }

        fn owns(app: &Preview, request: &ReleaseRequest) {
            assert!(app
                .model
                .release_worker
                .as_ref()
                .is_some_and(|token| Arc::ptr_eq(token, &request.token)));
            assert!(app
                .model
                .pending_release
                .as_ref()
                .is_some_and(|pending| Arc::ptr_eq(&pending.token, &request.token)));
        }

        // Exercise actual CheckRelease dispatch and actual release_command
        // construction. Never poll a provider future.
        fn click_once(app: &mut Preview) -> ReleaseRequest {
            let futures = app.dispatch(Message::CheckRelease).futures();
            assert_eq!(futures.len(), 1);
            let request = app.model.pending_release.as_ref().unwrap().clone();
            owns(app, &request);
            drop(futures);
            request
        }

        // Poll only real local inventory/receipt commands, using the same
        // supplied futures API as deliver(). Route results through dispatch.
        fn local_message(command: Command<Message>) -> Message {
            let mut futures = command.futures();
            assert_eq!(futures.len(), 1);
            let message = async_std::task::block_on(futures.remove(0));
            assert!(matches!(
                &message,
                Message::InventoryDone(..) | Message::ReceiptDone(..)
            ));
            message
        }

        fn finish_local(app: &mut Preview, command: Command<Message>) {
            no_command(app.dispatch(local_message(command)));
        }

        // Synthetic typed provider failure -> real error conversion -> real
        // completion handler. This does not execute release_command's mapper.
        fn rate_completion(
            app: &mut Preview,
            request: ReleaseRequest,
            code: StatusCode,
            wait: Duration,
        ) -> RetryDeadline {
            let deadline = match release_failure(Box::new(DownloadError::RateLimited {
                code,
                retry_after: wait,
            })) {
                ReleaseFailure::RateLimited(deadline) => deadline,
                other => panic!("Lost typed rate error: {:?}", other),
            };
            no_command(app.dispatch(Message::ReleaseRateLimited(request, deadline)));
            deadline
        }

        fn invalidate(app: &mut Preview, change: usize) {
            match change {
                0 => {
                    let original = app.model.repository.clone();
                    no_command(app.dispatch(Message::RepositoryChanged("other/addon".into())));
                    no_command(app.dispatch(Message::RepositoryChanged(original)));
                }
                1 => {
                    no_command(app.dispatch(Message::Select(Product::ClassicProgression)));
                    no_command(app.dispatch(Message::Select(Product::Retail)));
                }
                2 => {
                    let command = app.dispatch(Message::Rescan);
                    finish_local(app, command);
                    no_command(app.dispatch(Message::Select(Product::Retail)));
                }
                3 => {
                    let original = app.model.root.clone();
                    let missing = original.join("missing-fixture-root");
                    no_command(app.dispatch(Message::RootInputChanged(
                        missing.to_str().expect("UTF-8 fixture path").into(),
                    )));
                    let command = app.dispatch(Message::OpenRoot);
                    finish_local(app, command);
                    assert!(app.model.error.is_some());
                    no_command(app.dispatch(Message::RootInputChanged(
                        original.to_str().expect("UTF-8 fixture path").into(),
                    )));
                    let command = app.dispatch(Message::OpenRoot);
                    finish_local(app, command);
                    no_command(app.dispatch(Message::Select(Product::Retail)));
                }
                _ => {
                    let original = app.model.root.clone();
                    let away = original.join("draft-only-root");
                    no_command(app.dispatch(Message::RootInputChanged(
                        away.to_str().expect("UTF-8 fixture path").into(),
                    )));
                    no_command(app.dispatch(Message::RootInputChanged(
                        original.to_str().expect("UTF-8 fixture path").into(),
                    )));
                    assert!(app.model.pending_release.is_none());
                    let command = app.dispatch(Message::OpenRoot);
                    finish_local(app, command);
                    no_command(app.dispatch(Message::Select(Product::Retail)));
                }
            }
        }

        #[test]
        fn typed_long_and_unrepresentable_completions_block_actual_manual_dispatch() {
            for code in [StatusCode::TOO_MANY_REQUESTS, StatusCode::FORBIDDEN] {
                for wait in [Duration::from_secs(172_800), Duration::MAX] {
                    let (_temp, mut app) = app_fixture();
                    let inventory = app.model.inventory.as_ref().unwrap().clone();
                    let receipt_id = app.model.receipt_id.clone();
                    let request = click_once(&mut app);
                    let before = Instant::now();
                    let deadline = rate_completion(&mut app, request, code, wait);
                    let after = Instant::now();

                    match deadline {
                        RetryDeadline::At(at) => {
                            assert_ne!(wait, Duration::MAX);
                            assert!(at >= before.checked_add(wait).unwrap());
                            assert!(at <= after.checked_add(wait).unwrap());
                            assert!(at.duration_since(before) > Duration::from_secs(86_400));
                        }
                        RetryDeadline::Blocked => {
                            assert!(
                                wait == Duration::MAX
                                    || before.checked_add(wait).is_none()
                                    || after.checked_add(wait).is_none()
                            );
                        }
                    }
                    assert_eq!(app.model.release_retry_at, Some(deadline));
                    assert!(app.model.release_result.as_ref().unwrap().is_err());
                    assert!(app.model.release_worker.is_none());
                    assert!(app.model.pending_release.is_none());
                    assert!(Arc::ptr_eq(
                        app.model.inventory.as_ref().unwrap(),
                        &inventory
                    ));
                    assert_eq!(app.model.receipt_id, receipt_id);

                    for _ in 0..4 {
                        no_command(app.dispatch(Message::CheckRelease));
                        assert_eq!(app.model.release_retry_at, Some(deadline));
                        assert!(app.model.release_worker.is_none());
                        assert!(app.model.pending_release.is_none());
                    }
                    drop(app.view());
                    assert_passive_close_subscription(&app);
                    let report = serde_json::to_value(&app.model).unwrap();
                    assert!(report.get("release_retry_at").is_none());
                }
            }
        }

        #[test]
        fn stale_physical_rate_completion_establishes_wait_across_invalidations() {
            for wait in [Duration::from_secs(172_800), Duration::MAX] {
                for change in 0..5 {
                    let (_temp, mut app) = app_fixture();
                    let request = click_once(&mut app);
                    invalidate(&mut app, change);
                    assert!(app
                        .model
                        .release_worker
                        .as_ref()
                        .is_some_and(|token| Arc::ptr_eq(token, &request.token)));
                    assert!(app.model.pending_release.is_none());
                    no_command(app.dispatch(Message::CheckRelease));

                    let deadline = rate_completion(
                        &mut app,
                        request.clone(),
                        StatusCode::TOO_MANY_REQUESTS,
                        wait,
                    );
                    assert!(app.model.release_worker.is_none());
                    assert!(app.model.pending_release.is_none());
                    assert!(app.model.release_result.is_none());
                    assert_eq!(app.model.release_retry_at, Some(deadline));

                    // Later input/product/discovery changes also retain it.
                    invalidate(&mut app, change);
                    assert!(app.model.release_inputs_ready());
                    assert_eq!(app.model.release_retry_at, Some(deadline));
                    no_command(app.dispatch(Message::CheckRelease));

                    no_command(
                        app.dispatch(Message::ReleaseRateLimited(request, RetryDeadline::Blocked)),
                    );
                    assert_eq!(app.model.release_retry_at, Some(deadline));
                    assert!(app.model.release_worker.is_none());
                    assert!(app.model.release_result.is_none());
                }
            }
        }

        #[test]
        fn foreign_duplicate_and_old_aba_responses_cannot_mutate_wait_or_capacity() {
            let (_temp, mut app) = app_fixture();
            let old = click_once(&mut app);
            invalidate(&mut app, 0);
            let mut foreign = old.clone();
            foreign.token = Arc::new(());

            no_command(app.dispatch(Message::ReleaseRateLimited(foreign, RetryDeadline::Blocked)));
            assert!(app.model.release_retry_at.is_none());
            assert!(app
                .model
                .release_worker
                .as_ref()
                .is_some_and(|token| Arc::ptr_eq(token, &old.token)));
            assert!(app.model.pending_release.is_none());

            let deadline = rate_completion(
                &mut app,
                old.clone(),
                StatusCode::FORBIDDEN,
                Duration::from_secs(172_800),
            );
            assert!(app.model.release_result.is_none());
            assert!(app.model.release_worker.is_none());
            no_command(app.dispatch(Message::ReleaseRateLimited(
                old.clone(),
                RetryDeadline::Blocked,
            )));
            assert_eq!(app.model.release_retry_at, Some(deadline));

            // Controlled expired fixture state; no sleep or expiry callback.
            let expired = RetryDeadline::At(Instant::now());
            app.model.release_retry_at = Some(expired);
            let current = click_once(&mut app);
            assert!(!Arc::ptr_eq(&old.token, &current.token));
            let mut foreign = current.clone();
            foreign.token = Arc::new(());

            for message in [
                Message::ReleaseRateLimited(old.clone(), RetryDeadline::Blocked),
                Message::ReleaseDone(old.clone(), Ok("old".into())),
                Message::ReleaseRateLimited(foreign.clone(), RetryDeadline::Blocked),
                Message::ReleaseDone(foreign, Err("foreign".into())),
            ] {
                no_command(app.dispatch(message));
                owns(&app, &current);
                assert_eq!(app.model.release_retry_at, Some(expired));
                assert!(app.model.release_result.is_none());
            }

            no_command(app.dispatch(Message::ReleaseDone(current.clone(), Ok("current".into()))));
            assert!(app.model.release_worker.is_none());
            for message in [
                Message::ReleaseRateLimited(current, RetryDeadline::Blocked),
                Message::ReleaseRateLimited(old, RetryDeadline::Blocked),
            ] {
                no_command(app.dispatch(message));
                assert_eq!(app.model.release_retry_at, Some(expired));
                assert!(app.model.release_worker.is_none());
                assert!(app.model.pending_release.is_none());
                assert_eq!(
                    app.model
                        .release_result
                        .as_ref()
                        .unwrap()
                        .as_ref()
                        .unwrap()
                        .version,
                    "current",
                );
            }
        }

        #[test]
        fn expiry_is_idle_until_explicit_click_and_each_admission_is_single() {
            let (_temp, mut app) = app_fixture();
            let request = click_once(&mut app);
            rate_completion(
                &mut app,
                request,
                StatusCode::TOO_MANY_REQUESTS,
                Duration::from_secs(172_800),
            );
            no_command(app.dispatch(Message::CheckRelease));

            // Simulate elapsed time by supplying an already-expired deadline.
            // This deliberately does not claim a real two-day clock advance.
            app.model.release_retry_at = Some(RetryDeadline::At(Instant::now()));
            drop(app.view());
            assert_passive_close_subscription(&app);
            let repository = app.model.repository.clone();
            no_command(app.dispatch(Message::RepositoryChanged(repository)));
            assert!(app.model.release_worker.is_none());
            assert!(app.model.pending_release.is_none());

            let request = click_once(&mut app);
            for _ in 0..4 {
                no_command(app.dispatch(Message::CheckRelease));
                owns(&app, &request);
            }
            no_command(app.dispatch(Message::ReleaseDone(request, Ok("v2".into()))));
            assert!(app.model.release_worker.is_none());
            drop(app.view());
            assert!(app.model.release_worker.is_none());
            let next = click_once(&mut app);
            owns(&app, &next);
        }

        #[test]
        fn native_timer_messages_never_schedule_or_release_provider_work() {
            for state in 0..4 {
                let (_temp, mut app) = app_fixture();
                assert_passive_close_subscription(&app);
                let request = click_once(&mut app);
                match state {
                    0 => {
                        rate_completion(
                            &mut app,
                            request,
                            StatusCode::TOO_MANY_REQUESTS,
                            Duration::from_secs(172_800),
                        );
                    }
                    1 => {
                        rate_completion(
                            &mut app,
                            request,
                            StatusCode::TOO_MANY_REQUESTS,
                            Duration::MAX,
                        );
                    }
                    2 => {
                        rate_completion(
                            &mut app,
                            request,
                            StatusCode::TOO_MANY_REQUESTS,
                            Duration::from_secs(172_800),
                        );
                        app.model.release_retry_at = Some(RetryDeadline::At(Instant::now()));
                    }
                    _ => {}
                }
                let deadline = app.model.release_retry_at;
                let worker = app.model.release_worker.clone();
                let pending = app.model.pending_release.clone();
                app.native_check = true;
                assert_passive_close_subscription(&app);

                for _ in 0..3 {
                    no_command(app.dispatch(Message::NativeCheckDone));
                    assert!(app.should_exit());
                    assert_eq!(app.model.release_retry_at, deadline);
                    match (&worker, &app.model.release_worker) {
                        (Some(before), Some(after)) => assert!(Arc::ptr_eq(before, after)),
                        (None, None) => {}
                        other => panic!("Timer changed provider capacity: {:?}", other),
                    }
                    match &pending {
                        Some(request) => owns(&app, request),
                        None => assert!(app.model.pending_release.is_none()),
                    }
                }
            }
        }

        #[test]
        fn local_commands_and_queued_rescan_work_while_release_is_busy_or_blocked() {
            let (temp, mut app) = app_fixture();
            let app_data = tempfile::tempdir().unwrap();
            let config = app_data.path().join("config-sentinel.json");
            let user_file = temp.path().join("_retail_/Interface/AddOns/keep.lua");
            std::fs::write(&config, b"fixture config\n").unwrap();
            std::fs::write(&user_file, b"fixture user bytes\n").unwrap();
            let paths = [temp.path().join(".build.info"), user_file, config];
            let before: Vec<_> = paths
                .iter()
                .map(|path| std::fs::read(path).unwrap())
                .collect();

            let request = click_once(&mut app);
            let receipt = app.dispatch(Message::InspectReceipt);
            assert!(app.model.receipt_worker.is_some());
            finish_local(&mut app, receipt);
            assert!(app.model.receipt_error.is_some());
            owns(&app, &request);

            let scan = app.dispatch(Message::Rescan);
            finish_local(&mut app, scan);
            no_command(app.dispatch(Message::Select(Product::Retail)));
            assert!(app
                .model
                .release_worker
                .as_ref()
                .is_some_and(|token| Arc::ptr_eq(token, &request.token)));

            rate_completion(
                &mut app,
                request,
                StatusCode::TOO_MANY_REQUESTS,
                Duration::MAX,
            );
            assert_eq!(app.model.release_retry_at, Some(RetryDeadline::Blocked));
            assert!(app.model.release_result.is_none());
            assert!(app.model.can_inspect_receipt());

            let receipt = app.dispatch(Message::InspectReceipt);
            no_command(app.dispatch(Message::Rescan));
            assert!(app.model.scan_requested);
            assert!(app.model.scan.is_none());
            no_command(app.dispatch(Message::NativeCheckDone));
            assert!(!app.should_exit());
            no_command(app.dispatch(Message::CheckRelease));

            // The real receipt completion releases snapshot capacity and the
            // unchanged handler schedules exactly the queued inventory scan.
            let scan = app.dispatch(local_message(receipt));
            assert!(app.model.receipt_worker.is_none());
            assert!(app.model.scan.is_some());
            no_command(app.dispatch(Message::NativeCheckDone));
            assert!(!app.should_exit());
            finish_local(&mut app, scan);
            no_command(app.dispatch(Message::Select(Product::Retail)));

            assert!(app.model.inventory.is_some());
            assert!(app.model.can_inspect_receipt());
            assert_eq!(app.model.release_retry_at, Some(RetryDeadline::Blocked));
            no_command(app.dispatch(Message::CheckRelease));
            assert!(app.model.release_worker.is_none());
            drop(app.view());
            for (path, bytes) in paths.iter().zip(&before) {
                assert_eq!(&std::fs::read(path).unwrap(), bytes);
            }
        }
    }

    // Added 2026-10-28; original GPLv3 applies.
    mod recovery_dispatch_tests {
        use super::*;
        use ajour_core::fs::{
            preflight_restore_originals, restore_originals_checked,
            RecoveryRestoreOutcome as Outcome,
        };
        use std::{collections::BTreeMap, fs, path::Path};

        #[derive(Debug, PartialEq, Eq)]
        enum Entry {
            Directory,
            File(Vec<u8>),
            Link(PathBuf),
        }

        // Complete fixture membership/types/bytes, including empty directories.
        // Never follows links. Call only before work or after its future finishes,
        // and never while this test holds the root mutex.
        fn tree(root: &Path) -> BTreeMap<PathBuf, Entry> {
            fn visit(root: &Path, path: &Path, entries: &mut BTreeMap<PathBuf, Entry>) {
                let metadata = fs::symlink_metadata(path).unwrap();
                let link = metadata.file_type().is_symlink();
                #[cfg(windows)]
                let link = {
                    use std::os::windows::fs::MetadataExt;
                    link || metadata.file_attributes() & 0x400 != 0
                };
                let entry = if link {
                    Entry::Link(fs::read_link(path).unwrap())
                } else if metadata.is_dir() {
                    Entry::Directory
                } else {
                    assert!(metadata.is_file(), "unexpected fixture entry type");
                    Entry::File(fs::read(path).unwrap())
                };
                let directory = matches!(&entry, Entry::Directory);
                entries.insert(path.strip_prefix(root).unwrap().into(), entry);
                if directory {
                    for child in fs::read_dir(path).unwrap() {
                        visit(root, &child.unwrap().path(), entries);
                    }
                }
            }

            let mut entries = BTreeMap::new();
            visit(root, root, &mut entries);
            entries
        }

        fn no_command(command: Command<Message>) {
            assert!(command.futures().is_empty());
        }

        // Call only with local constructor/scan/receipt/recovery commands.
        // Never pass CheckRelease: no provider future is polled by these tests.
        fn local_message(command: Command<Message>) -> Message {
            let mut futures = command.futures();
            assert_eq!(futures.len(), 1);
            let message = async_std::task::block_on(futures.remove(0));
            assert!(matches!(
                &message,
                Message::InventoryDone(..) | Message::ReceiptDone(..) | Message::RecoveryDone(..)
            ));
            message
        }

        fn finish(app: &mut Preview, command: Command<Message>) {
            let message = local_message(command);
            no_command(app.dispatch(message));
        }

        fn owns(app: &Preview, request: &RecoveryRequest) {
            assert!(app
                .model
                .recovery_job
                .as_ref()
                .is_some_and(|job| job.same_request(request)));
            assert!(app
                .model
                .receipt_worker
                .as_ref()
                .is_some_and(|token| Arc::ptr_eq(token, &request.token)));
        }

        fn fixture(interrupted: bool) -> (tempfile::TempDir, tempfile::TempDir, Preview, PathBuf) {
            let (wow, model) = release_fixture();
            let data = tempfile::tempdir().unwrap();
            fs::create_dir(data.path().join("config")).unwrap();
            fs::write(
                data.path().join("config/settings.json"),
                b"{\"fixture\":\"preserve\"}\0\n",
            )
            .unwrap();

            // Selected WoW root and canonical AddOns root are different scopes.
            let addons = model
                .installation()
                .unwrap()
                .directory
                .join("Interface")
                .join("AddOns")
                .canonicalize()
                .unwrap();

            for directory in [
                wow.path().join("_retail_/WTF/Account/SavedVariables"),
                addons.join("WTF"),
                addons.join("SavedVariables"),
                addons.join("Unrelated"),
            ] {
                fs::create_dir_all(&directory).unwrap();
                fs::write(directory.join("keep.lua"), b"unrelated settings\0\n").unwrap();
            }

            if interrupted {
                let transaction = addons.join(".patchloom-install");
                fs::create_dir_all(transaction.join("backup/Bar")).unwrap();
                fs::create_dir(transaction.join("backup/Foo")).unwrap();
                fs::create_dir(transaction.join("staged")).unwrap();
                // Same accepted journal format as the supplied CLI/core fixtures.
                fs::write(
                    transaction.join("plan.yml"),
                    b"version: 1\nprevious: [Bar, Foo]\nincoming: [Foo, New]\n",
                )
                .unwrap();
                fs::write(transaction.join("ready"), b"").unwrap();
                fs::write(transaction.join("backup/Bar/old.lua"), b"original-bar").unwrap();
                fs::write(transaction.join("backup/Foo/old.lua"), b"original").unwrap();
                for (name, bytes) in [
                    ("Foo", b"replacement".as_slice()),
                    ("New", b"additional".as_slice()),
                ] {
                    fs::create_dir(addons.join(name)).unwrap();
                    fs::write(addons.join(name).join("new.lua"), bytes).unwrap();
                }
                // Nonempty mutex bytes are included in test inventories, but
                // are deliberately not approval authority.
                fs::write(
                    addons.join(".patchloom-install.lock"),
                    b"fixture mutex bytes\n",
                )
                .unwrap();
            }

            (wow, data, Preview::from_model(model, false), addons)
        }

        fn plan(app: &mut Preview) -> RecoveryPreview {
            let command = app.dispatch(Message::InspectRecovery);
            let request = app.model.pending_recovery.as_ref().unwrap().clone();
            owns(app, &request);
            assert!(matches!(&request.action, RecoveryAction::Inspect));
            assert!(app.model.scan.is_none());
            finish(app, command);
            assert!(app.model.recovery_error.is_none());
            assert!(app.model.recovery_inspection.is_some());

            let command = app.dispatch(Message::PreviewRecovery);
            let request = app.model.pending_recovery.as_ref().unwrap().clone();
            owns(app, &request);
            assert!(matches!(&request.action, RecoveryAction::Preview));
            finish(app, command);
            assert!(app.model.recovery_error.is_none());
            assert!(app.model.pending_recovery.is_none());
            assert!(app.model.recovery_job.is_none());
            assert!(app.model.receipt_worker.is_none());
            assert!(app.model.recovery_ack.is_empty());
            app.model.recovery_preview.as_ref().unwrap().clone()
        }

        fn restore(app: &mut Preview, preview: &RecoveryPreview) -> Command<Message> {
            no_command(app.dispatch(Message::RecoveryAckChanged(
                preview.clone(),
                "RESTORE".into(),
            )));
            assert!(app.model.can_restore_recovery(preview));
            let command = app.dispatch(Message::RestoreRecovery(preview.clone()));
            let request = app.model.pending_recovery.as_ref().unwrap();
            owns(app, request);
            assert!(matches!(&request.action, RecoveryAction::Restore(_)));
            assert!(app.model.restoring());
            assert!(app.model.inventory.is_none());
            assert!(app.model.recovery_preview.is_none());
            assert!(app.model.recovery_ack.is_empty());
            assert!(app.model.scan.is_none());
            assert!(!app.model.scan_requested);
            command
        }

        #[test]
        fn inspect_preflight_core_cancel_and_preview_cancel_preserve_complete_trees() {
            let (wow, data, mut app, addons) = fixture(true);
            let before = tree(wow.path());
            let config_before = tree(data.path());

            // Construction, rendering and the native-check message do not recover.
            assert!(app.model.recovery_job.is_none());
            assert_passive_close_subscription(&app);
            drop(app.view());
            no_command(app.dispatch(Message::NativeCheckDone));
            assert!(app.model.recovery_job.is_none());
            assert_no_release_work(&app.model);

            let preview = plan(&mut app);
            assert_eq!(preview.request.root, app.model.root);
            assert_eq!(preview.request.product, Product::Retail);
            assert_eq!(preview.request.addons.canonicalize().unwrap(), addons);
            assert_eq!(preview.approval.root(), addons.as_path());
            assert_ne!(preview.approval.root(), app.model.root.as_path());
            assert_eq!(preview.approval.originals(), ["Bar", "Foo"]);
            assert_eq!(
                preview.approval.move_aside_if_present(),
                ["Bar", "Foo", "New"]
            );
            // Bar is approved as affected, not falsely asserted to exist.
            assert!(!addons.join("Bar").exists());
            assert_eq!(tree(wow.path()), before);
            assert_eq!(tree(data.path()), config_before);

            // Exercise actual checked-core cancellation. The GUI's restore
            // worker always supplies admitted consent; it has no cancel dialog.
            let mut calls = 0;
            let outcome = restore_originals_checked(
                &addons,
                preview.approval.as_ref(),
                |originals, retained| {
                    calls += 1;
                    assert_eq!(originals, preview.approval.originals());
                    assert_eq!(retained, preview.approval.move_aside_if_present());
                    false
                },
            )
            .unwrap();
            assert_eq!(calls, 1);
            assert_eq!(outcome, Outcome::Cancelled);
            assert_eq!(tree(wow.path()), before);
            assert_eq!(tree(data.path()), config_before);

            no_command(app.dispatch(Message::RecoveryAckChanged(
                preview.clone(),
                "RESTORE".into(),
            )));
            no_command(app.dispatch(Message::CancelRecoveryPreview(preview.clone())));
            assert!(app.model.recovery_preview.is_none());
            assert!(app.model.recovery_ack.is_empty());
            assert_eq!(
                app.model.recovery_error.as_deref(),
                Some("Preview cancelled. No addon folders were changed.")
            );
            no_command(app.dispatch(Message::RestoreRecovery(preview)));
            assert!(app.model.recovery_outcome.is_none());
            assert!(app.model.recovery_job.is_none());
            assert!(app.model.receipt_worker.is_none());
            assert!(!addons.join(".patchloom-restore").exists());
            drop(app.view());
            assert_no_release_work(&app.model);
            assert_eq!(tree(wow.path()), before);
            assert_eq!(tree(data.path()), config_before);
        }

        #[test]
        fn no_journal_is_reported_without_creating_a_mutex_or_offering_a_plan() {
            let (wow, data, mut app, addons) = fixture(false);
            let before = tree(wow.path());
            let config_before = tree(data.path());

            let command = app.dispatch(Message::InspectRecovery);
            finish(&mut app, command);
            let inspection = app.model.recovery_inspection.as_ref().unwrap();
            assert_eq!(inspection.root, addons);
            assert_eq!(inspection.phase, "none");
            assert!(app.model.recovery_error.is_none());
            assert!(!app.model.can_preview_recovery());
            no_command(app.dispatch(Message::PreviewRecovery));
            assert!(app.model.recovery_preview.is_none());

            assert!(preflight_restore_originals(&addons).is_err());
            assert!(!addons.join(".patchloom-install.lock").exists());
            assert!(!addons.join(".patchloom-install").exists());
            assert!(!addons.join(".patchloom-restore").exists());
            drop(app.view());
            assert_no_release_work(&app.model);
            assert_eq!(tree(wow.path()), before);
            assert_eq!(tree(data.path()), config_before);
        }

        #[test]
        fn checked_restore_command_restores_bundle_and_retains_versions_and_journals() {
            let (wow, data, mut app, addons) = fixture(true);
            let untouched = [
                wow.path().join("_classic_"),
                wow.path().join("_retail_/WTF"),
                addons.join("WTF"),
                addons.join("SavedVariables"),
                addons.join("Unrelated"),
            ];
            let before: Vec<_> = untouched.iter().map(|path| tree(path)).collect();
            let config_before = tree(data.path());
            let build_before = fs::read(wow.path().join(".build.info")).unwrap();
            let transaction = addons.join(".patchloom-install");
            let journal_before = fs::read(transaction.join("plan.yml")).unwrap();
            let mutex_before = fs::read(addons.join(".patchloom-install.lock")).unwrap();

            let preview = plan(&mut app);
            let command = restore(&mut app, &preview);
            let request = app.model.pending_recovery.as_ref().unwrap().clone();

            // Admission consumes consent and reserves capacity before polling.
            no_command(app.dispatch(Message::RestoreRecovery(preview.clone())));
            owns(&app, &request);
            assert!(addons.join("Foo/new.lua").is_file());
            assert!(!addons.join(".patchloom-restore").exists());

            // Actual Command -> spawn_blocking -> checked core -> post-inspection.
            let message = local_message(command);
            assert!(matches!(
                &message,
                Message::RecoveryDone(
                    _,
                    RecoveryResult::Restore {
                        outcome: Ok(Outcome::RestoredRetained),
                        inspection: Ok(_),
                    }
                )
            ));
            // Completed worker output has not yet released UI capacity.
            owns(&app, &request);
            assert!(app.model.restoring());
            no_command(app.dispatch(message.clone()));

            assert!(!app.model.restoring());
            assert!(app.model.recovery_job.is_none());
            assert!(app.model.pending_recovery.is_none());
            assert!(app.model.receipt_worker.is_none());
            assert!(app.model.inventory.is_none());
            assert!(app.model.receipt.is_none());
            assert!(app.model.recovery_post_error.is_none());
            assert!(matches!(
                app.model.recovery_outcome.as_ref(),
                Some(Ok(Outcome::RestoredRetained))
            ));
            assert_eq!(fs::read(addons.join("Foo/old.lua")).unwrap(), b"original");
            assert_eq!(
                fs::read(addons.join("Bar/old.lua")).unwrap(),
                b"original-bar"
            );
            assert!(!addons.join("Foo/new.lua").exists());
            assert!(!addons.join("New").exists());
            assert_eq!(
                fs::read(addons.join(".patchloom-restore/backup/Foo/new.lua")).unwrap(),
                b"replacement"
            );
            assert_eq!(
                fs::read(addons.join(".patchloom-restore/backup/New/new.lua")).unwrap(),
                b"additional"
            );
            assert_eq!(
                fs::read(transaction.join("plan.yml")).unwrap(),
                journal_before
            );
            assert!(transaction.join("ready").is_file());
            assert!(transaction.join("backup").is_dir());
            assert!(transaction.join("staged").is_dir());
            assert!(addons.join(".patchloom-restore/plan.yml").is_file());
            assert!(addons.join(".patchloom-restore/committed").is_file());
            let inspection = app.model.recovery_inspection.as_ref().unwrap();
            assert_eq!(
                inspection.restore_phase.as_deref(),
                Some("restoring-or-restored")
            );
            assert_eq!(inspection.retained_replacements, ["Foo", "New"]);

            // Retained recovery state is not cleaned up or made reusable.
            assert!(preflight_restore_originals(&addons).is_err());
            no_command(app.dispatch(message));
            no_command(app.dispatch(Message::RestoreRecovery(preview)));
            no_command(app.dispatch(Message::NativeCheckDone));
            drop(app.view());
            assert!(app.model.recovery_job.is_none());
            assert!(app.model.scan.is_none());
            assert!(!app.model.scan_requested);
            assert_no_release_work(&app.model);
            assert!(addons.join(".patchloom-restore/committed").is_file());
            for (path, expected) in untouched.iter().zip(&before) {
                assert_eq!(&tree(path), expected);
            }
            assert_eq!(tree(data.path()), config_before);
            assert_eq!(
                fs::read(wow.path().join(".build.info")).unwrap(),
                build_before
            );
            assert_eq!(
                fs::read(addons.join(".patchloom-install.lock")).unwrap(),
                mutex_before
            );
        }

        #[test]
        fn journal_backup_and_current_changes_are_refused_by_the_real_worker() {
            for change in 0..3 {
                let (wow, data, mut app, addons) = fixture(true);
                let preview = plan(&mut app);
                let path = match change {
                    0 => addons.join(".patchloom-install/plan.yml"),
                    1 => addons.join(".patchloom-install/backup/Foo/old.lua"),
                    _ => addons.join("Foo/new.lua"),
                };
                let mut bytes = fs::read(&path).unwrap();
                if change == 0 {
                    // Same valid parsed plan, different authority bytes.
                    bytes.extend_from_slice(b"\n# changed after preview\n");
                } else {
                    bytes[0] ^= 1;
                }
                fs::write(path, bytes).unwrap();
                let before = tree(wow.path());
                let config_before = tree(data.path());

                let command = restore(&mut app, &preview);
                let message = local_message(command);
                assert!(matches!(
                    &message,
                    Message::RecoveryDone(
                        _,
                        RecoveryResult::Restore {
                            outcome: Ok(Outcome::Changed),
                            inspection: Ok(_),
                        }
                    )
                ));
                no_command(app.dispatch(message));
                assert!(matches!(
                    app.model.recovery_outcome.as_ref(),
                    Some(Ok(Outcome::Changed))
                ));
                assert!(app.model.recovery_preview.is_none());
                assert!(app.model.recovery_ack.is_empty());
                assert!(app.model.recovery_post_error.is_none());
                assert!(app.model.inventory.is_none());
                no_command(app.dispatch(Message::RecoveryAckChanged(
                    preview.clone(),
                    "RESTORE".into(),
                )));
                no_command(app.dispatch(Message::RestoreRecovery(preview)));
                drop(app.view());
                assert!(!addons.join(".patchloom-restore").exists());
                assert!(app.model.recovery_job.is_none());
                assert_no_release_work(&app.model);
                assert_eq!(tree(wow.path()), before);
                assert_eq!(tree(data.path()), config_before);
            }
        }

        #[test]
        fn consent_rejects_prefixes_and_foreign_scope_generation_token_and_ticket() {
            let (wow, data, mut app, addons) = fixture(true);
            let (_other_wow, _other_data, _other_app, other_addons) = fixture(true);
            let preview = plan(&mut app);
            let before = tree(wow.path());
            let config_before = tree(data.path());

            no_command(app.dispatch(Message::RestoreRecovery(preview.clone())));
            for answer in [
                "",
                "restore",
                " RESTORE",
                "RESTORE ",
                "RESTORE\n",
                "RESTOREx",
                "RESTORE\u{202e}",
            ] {
                // An invalid replacement must also remove previously valid text.
                no_command(app.dispatch(Message::RecoveryAckChanged(
                    preview.clone(),
                    "RESTORE".into(),
                )));
                no_command(
                    app.dispatch(Message::RecoveryAckChanged(preview.clone(), answer.into())),
                );
                assert!(!app.model.can_restore_recovery(&preview));
                no_command(app.dispatch(Message::RestoreRecovery(preview.clone())));
                assert!(app.model.recovery_job.is_none());
            }

            for mismatch in 0..7 {
                let mut foreign = preview.clone();
                match mismatch {
                    0 => foreign.request.root = foreign.request.root.join("other"),
                    1 => foreign.request.addons = foreign.request.addons.join("other"),
                    2 => foreign.request.product = Product::ClassicProgression,
                    3 => foreign.request.generation = Arc::new(()),
                    4 => foreign.request.token = Arc::new(()),
                    5 => {
                        // A real fresh ticket with identical content is still
                        // a different in-memory approval identity.
                        foreign.approval = Arc::new(preflight_restore_originals(&addons).unwrap());
                    }
                    _ => {
                        foreign.approval =
                            Arc::new(preflight_restore_originals(&other_addons).unwrap());
                    }
                }

                no_command(app.dispatch(Message::RecoveryAckChanged(
                    preview.clone(),
                    "RESTORE".into(),
                )));
                no_command(
                    app.dispatch(Message::RecoveryAckChanged(foreign.clone(), String::new())),
                );
                assert_eq!(app.model.recovery_ack, "RESTORE");
                no_command(app.dispatch(Message::RestoreRecovery(foreign.clone())));
                no_command(app.dispatch(Message::CancelRecoveryPreview(foreign)));
                assert!(app.model.current_recovery_preview(&preview));
                assert!(app.model.recovery_job.is_none());
                assert!(app.model.receipt_worker.is_none());
                assert!(app.model.recovery_outcome.is_none());
            }

            assert!(!addons.join(".patchloom-restore").exists());
            assert_no_release_work(&app.model);
            assert_eq!(tree(wow.path()), before);
            assert_eq!(tree(data.path()), config_before);
        }
        // Added 2026-10-28; original GPLv3 applies.
        fn finish_recovery_failure(app: &mut Preview, command: Command<Message>) {
            let message = local_message(command);
            let expected = match &message {
                Message::RecoveryDone(
                    request,
                    RecoveryResult::Inspection(Err(error)) | RecoveryResult::Preview(Err(error)),
                ) => {
                    owns(app, request);
                    assert!(!error.is_empty());
                    error.clone()
                }
                other => panic!("Expected actual recovery failure: {:?}", other),
            };
            no_command(app.dispatch(message));
            assert_eq!(app.model.recovery_error.as_deref(), Some(expected.as_str()));
            assert!(app.model.recovery_preview.is_none());
            assert!(app.model.recovery_ack.is_empty());
            assert!(app.model.recovery_outcome.is_none());
            assert!(app.model.recovery_post_error.is_none());
            assert!(app.model.pending_recovery.is_none());
            assert!(app.model.recovery_job.is_none());
            assert!(app.model.receipt_worker.is_none());
        }

        #[test]
        fn product_rescan_draft_and_open_aba_require_a_new_preview_and_ack() {
            for change in 0..4 {
                let (wow, data, mut app, addons) = fixture(true);
                let (other, _) = receipt_model();
                let before = tree(wow.path());
                let config_before = tree(data.path());
                let other_before = tree(other.path());
                let old = plan(&mut app);
                no_command(
                    app.dispatch(Message::RecoveryAckChanged(old.clone(), "RESTORE".into())),
                );
                let original = app.model.root.clone();
                let generation = app.model.generation.clone();

                match change {
                    0 => {
                        no_command(app.dispatch(Message::Select(Product::ClassicProgression)));
                        no_command(app.dispatch(Message::Select(Product::Retail)));
                        // Selection ABA must fail even without a new generation.
                        assert!(Arc::ptr_eq(&generation, &app.model.generation));
                    }
                    1 => {
                        let command = app.dispatch(Message::Rescan);
                        finish(&mut app, command);
                        no_command(app.dispatch(Message::Select(Product::Retail)));
                    }
                    2 => {
                        let away = original.join("unopened-draft");
                        no_command(app.dispatch(Message::RootInputChanged(
                            away.to_str().expect("UTF-8 fixture path").into(),
                        )));
                        no_command(app.dispatch(Message::RootInputChanged(
                            original.to_str().expect("UTF-8 fixture path").into(),
                        )));
                        assert!(app.root_edited);
                        no_command(app.dispatch(Message::RestoreRecovery(old.clone())));
                        let command = app.dispatch(Message::OpenRoot);
                        finish(&mut app, command);
                        no_command(app.dispatch(Message::Select(Product::Retail)));
                    }
                    _ => {
                        for root in [other.path(), original.as_path()] {
                            no_command(app.dispatch(Message::RootInputChanged(
                                root.to_str().expect("UTF-8 fixture path").into(),
                            )));
                            let command = app.dispatch(Message::OpenRoot);
                            finish(&mut app, command);
                        }
                        no_command(app.dispatch(Message::Select(Product::Retail)));
                    }
                }

                assert_eq!(app.model.root, original);
                assert_eq!(app.model.selected, Some(Product::Retail));
                if change != 0 {
                    assert!(!Arc::ptr_eq(&generation, &app.model.generation));
                }
                assert!(!app.root_edited);
                assert!(app.model.recovery_preview.is_none());
                assert!(app.model.recovery_ack.is_empty());
                assert!(!app.model.current_recovery_preview(&old));
                no_command(
                    app.dispatch(Message::RecoveryAckChanged(old.clone(), "RESTORE".into())),
                );
                no_command(app.dispatch(Message::RestoreRecovery(old.clone())));
                assert!(app.model.recovery_job.is_none());

                let fresh = plan(&mut app);
                assert_eq!(fresh.approval.root(), addons.as_path());
                assert!(!Arc::ptr_eq(&old.request.token, &fresh.request.token));
                assert!(!Arc::ptr_eq(&old.approval, &fresh.approval));
                assert!(app.model.recovery_ack.is_empty());
                no_command(app.dispatch(Message::RestoreRecovery(fresh.clone())));

                // Stale events cannot consume, clear or reuse fresh consent.
                no_command(
                    app.dispatch(Message::RecoveryAckChanged(fresh.clone(), "RESTORE".into())),
                );
                no_command(app.dispatch(Message::RecoveryAckChanged(old.clone(), String::new())));
                no_command(app.dispatch(Message::CancelRecoveryPreview(old.clone())));
                no_command(app.dispatch(Message::RestoreRecovery(old)));
                assert_eq!(app.model.recovery_ack, "RESTORE");
                assert!(app.model.current_recovery_preview(&fresh));
                assert!(app.model.can_restore_recovery(&fresh));
                assert!(app.model.recovery_job.is_none());
                assert!(app.model.receipt_worker.is_none());
                assert!(!addons.join(".patchloom-restore").exists());
                assert_no_release_work(&app.model);
                assert_eq!(tree(wow.path()), before);
                assert_eq!(tree(data.path()), config_before);
                assert_eq!(tree(other.path()), other_before);
            }
        }

        #[test]
        fn invalid_unknown_and_missing_original_states_fail_without_fixture_changes() {
            for change in 0..8 {
                let (wow, data, mut app, addons) = fixture(true);
                let command = app.dispatch(Message::InspectRecovery);
                finish(&mut app, command);
                assert!(app.model.can_preview_recovery());
                let transaction = addons.join(".patchloom-install");

                match change {
                    0 => fs::write(transaction.join("plan.yml"), b"version: [invalid").unwrap(),
                    1 => fs::write(
                        transaction.join("plan.yml"),
                        b"version: 99\nprevious: [Bar, Foo]\nincoming: [Foo, New]\n",
                    )
                    .unwrap(),
                    2 => fs::write(transaction.join("unknown-entry"), b"retain me").unwrap(),
                    3 => {
                        fs::create_dir(transaction.join("backup/Unexpected")).unwrap();
                        fs::write(
                            transaction.join("backup/Unexpected/keep.lua"),
                            b"unknown backup",
                        )
                        .unwrap();
                    }
                    4 => fs::write(transaction.join("ready"), b"invalid marker").unwrap(),
                    5 => fs::write(transaction.join("committed"), b"").unwrap(),
                    6 => fs::rename(
                        transaction.join("backup/Foo"),
                        wow.path().join("preserved-original"),
                    )
                    .unwrap(),
                    _ => {
                        fs::remove_file(addons.join(".patchloom-install.lock")).unwrap();
                        fs::create_dir(addons.join(".patchloom-install.lock")).unwrap();
                    }
                }
                let before = tree(wow.path());
                let config_before = tree(data.path());

                // A formerly readable inspection is not authority. Actual
                // preflight must reject the changed invalid state.
                let command = app.dispatch(Message::PreviewRecovery);
                finish_recovery_failure(&mut app, command);

                let command = app.dispatch(Message::InspectRecovery);
                if change == 6 {
                    // Inspection reports missing backups; it does not certify
                    // that the original bundle can be restored.
                    finish(&mut app, command);
                    let inspection = app.model.recovery_inspection.as_ref().unwrap();
                    assert_eq!(inspection.phase, "ready-marker");
                    assert_eq!(inspection.backups, ["Bar"]);
                    assert!(app.model.recovery_error.is_none());
                    let command = app.dispatch(Message::PreviewRecovery);
                    finish_recovery_failure(&mut app, command);
                } else {
                    finish_recovery_failure(&mut app, command);
                    assert!(app.model.recovery_inspection.is_none());
                }

                drop(app.view());
                no_command(app.dispatch(Message::NativeCheckDone));
                assert!(app.model.recovery_job.is_none());
                assert!(app.model.recovery_preview.is_none());
                assert!(app.model.scan.is_none());
                assert!(!addons.join(".patchloom-restore").exists());
                assert_no_release_work(&app.model);
                assert_eq!(tree(wow.path()), before);
                assert_eq!(tree(data.path()), config_before);
            }
        }

        #[test]
        fn busy_mutex_rejects_inspect_preflight_and_checked_restore_without_retry() {
            for action in 0..3 {
                let (wow, data, mut app, addons) = fixture(true);
                let preview = if action == 2 {
                    Some(plan(&mut app))
                } else {
                    if action == 1 {
                        let command = app.dispatch(Message::InspectRecovery);
                        finish(&mut app, command);
                    }
                    None
                };
                let before = tree(wow.path());
                let config_before = tree(data.path());
                let lock = fs::File::options()
                    .read(true)
                    .write(true)
                    .open(addons.join(".patchloom-install.lock"))
                    .unwrap();
                lock.try_lock().unwrap();

                let command = match action {
                    0 => app.dispatch(Message::InspectRecovery),
                    1 => app.dispatch(Message::PreviewRecovery),
                    _ => restore(&mut app, preview.as_ref().unwrap()),
                };
                let request = app.model.pending_recovery.as_ref().unwrap().clone();
                owns(&app, &request);

                if action == 2 {
                    let message = local_message(command);
                    assert!(matches!(
                        &message,
                        Message::RecoveryDone(
                            _,
                            RecoveryResult::Restore {
                                outcome: Err(_),
                                inspection: Err(_),
                            }
                        )
                    ));
                    // Error and follow-up error have both been obtained, but
                    // capacity remains reserved until matching dispatch.
                    owns(&app, &request);
                    assert!(app.model.restoring());
                    no_command(app.dispatch(message.clone()));
                    no_command(app.dispatch(message));
                    assert!(matches!(app.model.recovery_outcome.as_ref(), Some(Err(_))));
                    assert!(app.model.recovery_post_error.is_some());
                    assert!(app.model.recovery_inspection.is_none());
                    assert!(app.model.inventory.is_none());
                } else {
                    finish_recovery_failure(&mut app, command);
                }

                // Never inventory or reopen mutex bytes while this handle
                // holds the exclusive OS lock, including on Windows.
                drop(lock);
                assert!(app.model.recovery_job.is_none());
                assert!(app.model.pending_recovery.is_none());
                assert!(app.model.receipt_worker.is_none());
                drop(app.view());
                no_command(app.dispatch(Message::NativeCheckDone));
                assert!(app.model.recovery_job.is_none());
                assert_no_release_work(&app.model);
                assert!(!addons.join(".patchloom-restore").exists());
                assert_eq!(tree(wow.path()), before);
                assert_eq!(tree(data.path()), config_before);

                // Lock release permits a fresh real preflight. It does not
                // trigger an implicit GUI retry or reuse the consumed consent.
                let fresh = preflight_restore_originals(&addons).unwrap();
                assert_eq!(fresh.root(), addons.as_path());
                assert!(app.model.recovery_preview.is_none());
                assert!(app.model.recovery_ack.is_empty());
                assert_eq!(tree(wow.path()), before);
                assert_eq!(tree(data.path()), config_before);
            }
        }

        #[cfg(any(unix, windows))]
        fn fixture_directory_link(alias: &Path, target: &Path) {
            #[cfg(unix)]
            std::os::unix::fs::symlink(target, alias).unwrap();
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                let output = std::process::Command::new("cmd.exe")
                    .args(["/d", "/c", "mklink", "/J"])
                    .arg(alias.components().collect::<PathBuf>())
                    .arg(target.components().collect::<PathBuf>())
                    .creation_flags(0x08000000)
                    .output()
                    .unwrap();
                assert!(
                    output.status.success(),
                    "{}",
                    String::from_utf8_lossy(&output.stderr)
                );
                use std::os::windows::fs::MetadataExt;
                assert_ne!(
                    fs::symlink_metadata(alias).unwrap().file_attributes() & 0x400,
                    0
                );
            }
        }

        #[cfg(any(unix, windows))]
        #[test]
        fn linked_root_journal_backup_and_current_content_cannot_get_approval() {
            for location in 0..4 {
                let (wow, data, mut app, addons) = fixture(true);
                let command = app.dispatch(Message::InspectRecovery);
                finish(&mut app, command);
                let transaction = addons.join(".patchloom-install");
                let outside = data.path().join("link-target");
                fs::create_dir(&outside).unwrap();
                fs::write(outside.join("keep.lua"), b"outside sentinel").unwrap();
                let (alias, target) = match location {
                    0 => {
                        let saved = wow.path().join("preserved-addons");
                        fs::rename(&addons, &saved).unwrap();
                        (addons.clone(), saved)
                    }
                    1 => {
                        let saved = wow.path().join("preserved-journal");
                        fs::rename(&transaction, &saved).unwrap();
                        (transaction.clone(), saved)
                    }
                    2 => (transaction.join("backup/Foo/link"), outside.clone()),
                    _ => (addons.join("Foo/link"), outside.clone()),
                };
                fixture_directory_link(&alias, &target);
                let before = tree(wow.path());
                let config_before = tree(data.path());
                let target_before = tree(&target);

                let command = app.dispatch(Message::PreviewRecovery);
                finish_recovery_failure(&mut app, command);
                let command = app.dispatch(Message::InspectRecovery);
                if location == 3 {
                    // Inspection lists destinations without recursively
                    // approving current content. Preflight above rejected it.
                    finish(&mut app, command);
                    assert!(app.model.recovery_error.is_none());
                    assert!(app.model.recovery_inspection.is_some());
                    assert!(app.model.recovery_preview.is_none());
                } else {
                    finish_recovery_failure(&mut app, command);
                }
                assert!(app.model.recovery_outcome.is_none());
                assert!(app.model.recovery_job.is_none());
                assert_no_release_work(&app.model);
                assert_eq!(tree(wow.path()), before);
                assert_eq!(tree(data.path()), config_before);
                assert_eq!(tree(&target), target_before);

                #[cfg(windows)]
                fs::remove_dir(&alias).unwrap(); // Unlink only this fixture junction.
                assert_eq!(tree(&target), target_before);
                assert_eq!(
                    fs::read(outside.join("keep.lua")).unwrap(),
                    b"outside sentinel"
                );
            }
        }

        #[cfg(unix)]
        #[test]
        fn linked_mutex_plan_and_marker_fail_actual_dispatch_without_touching_targets() {
            for location in 0..3 {
                let (wow, data, mut app, addons) = fixture(true);
                let command = app.dispatch(Message::InspectRecovery);
                finish(&mut app, command);
                let path = match location {
                    0 => addons.join(".patchloom-install.lock"),
                    1 => addons.join(".patchloom-install/plan.yml"),
                    _ => addons.join(".patchloom-install/ready"),
                };
                let saved = wow.path().join("preserved-linked-file");
                fs::rename(&path, &saved).unwrap();
                std::os::unix::fs::symlink(&saved, &path).unwrap();
                let before = tree(wow.path());
                let config_before = tree(data.path());

                let command = app.dispatch(Message::PreviewRecovery);
                finish_recovery_failure(&mut app, command);
                let command = app.dispatch(Message::InspectRecovery);
                finish_recovery_failure(&mut app, command);
                assert!(!addons.join(".patchloom-restore").exists());
                assert_no_release_work(&app.model);
                assert_eq!(tree(wow.path()), before);
                assert_eq!(tree(data.path()), config_before);
            }
        }

        // Added 2026-10-28; original GPLv3 applies.
        #[test]
        fn scan_and_receipt_capacity_exclude_recovery_and_drain_one_queued_scan() {
            for receipt_owner in [false, true] {
                let (wow, data, mut app, _) = fixture(true);
                let preview = plan(&mut app);
                no_command(app.dispatch(Message::RecoveryAckChanged(
                    preview.clone(),
                    "RESTORE".into(),
                )));
                no_command(app.dispatch(Message::ReceiptIdChanged("a".repeat(64))));
                let before = tree(wow.path());
                let config_before = tree(data.path());
                let command = app.dispatch(if receipt_owner {
                    Message::InspectReceipt
                } else {
                    Message::Rescan
                });
                let token = if receipt_owner {
                    app.model.receipt_worker.as_ref().unwrap().clone()
                } else {
                    app.model.scan.as_ref().unwrap().clone()
                };

                for message in [
                    Message::InspectRecovery,
                    Message::PreviewRecovery,
                    Message::RestoreRecovery(preview.clone()),
                    Message::InspectReceipt,
                ] {
                    no_command(app.dispatch(message));
                }
                assert!(app.model.recovery_job.is_none());
                assert!(app.model.pending_recovery.is_none());
                no_command(app.dispatch(Message::Rescan));
                no_command(app.dispatch(Message::Rescan));
                assert!(app.model.scan_requested);
                let owner = if receipt_owner {
                    app.model.receipt_worker.as_ref().unwrap()
                } else {
                    app.model.scan.as_ref().unwrap()
                };
                assert!(Arc::ptr_eq(owner, &token));

                let old = local_message(command);
                let queued = app.dispatch(old.clone());
                assert!(app.model.receipt_worker.is_none());
                assert!(!app.model.scan_requested);
                let scan = app.model.scan.as_ref().unwrap().clone();
                no_command(app.dispatch(old));
                assert!(Arc::ptr_eq(app.model.scan.as_ref().unwrap(), &scan));
                no_command(app.dispatch(Message::InspectRecovery));
                no_command(app.dispatch(Message::PreviewRecovery));
                no_command(app.dispatch(Message::InspectReceipt));
                finish(&mut app, queued);
                no_command(app.dispatch(Message::Select(Product::Retail)));
                assert!(app.model.recovery_slot_available());
                assert!(app.model.recovery_inspection.is_none());
                assert!(app.model.recovery_preview.is_none());
                assert!(app.model.receipt.is_none());
                assert!(app.model.receipt_error.is_none());
                assert_no_release_work(&app.model);
                assert_eq!(tree(wow.path()), before);
                assert_eq!(tree(data.path()), config_before);
            }
        }

        #[test]
        fn invalidation_keeps_recovery_capacity_until_stale_matching_completion() {
            for preview_action in [false, true] {
                for change in 0..4 {
                    let (wow, data, mut app, _) = fixture(true);
                    let (other, _) = receipt_model();
                    let approved = plan(&mut app);
                    let before = tree(wow.path());
                    let config_before = tree(data.path());
                    let other_before = tree(other.path());
                    let command = app.dispatch(if preview_action {
                        Message::PreviewRecovery
                    } else {
                        Message::InspectRecovery
                    });
                    let request = app.model.pending_recovery.as_ref().unwrap().clone();
                    let message = local_message(command);
                    let root = app.model.root.clone();

                    match change {
                        0 => {
                            no_command(app.dispatch(Message::Select(Product::ClassicProgression)));
                            no_command(app.dispatch(Message::Select(Product::Retail)));
                        }
                        1 => no_command(app.dispatch(Message::Rescan)),
                        2 => {
                            no_command(app.dispatch(Message::Rescan));
                            for path in [root.join("draft-only"), root.clone()] {
                                no_command(app.dispatch(Message::RootInputChanged(
                                    path.to_str().unwrap().into(),
                                )));
                            }
                            assert!(!app.model.scan_requested);
                            assert!(app.root_edited);
                        }
                        _ => {
                            for path in [other.path(), root.as_path()] {
                                no_command(app.dispatch(Message::RootInputChanged(
                                    path.to_str().unwrap().into(),
                                )));
                                no_command(app.dispatch(Message::OpenRoot));
                            }
                        }
                    }
                    assert!(app.model.pending_recovery.is_none());
                    assert!(app
                        .model
                        .recovery_job
                        .as_ref()
                        .unwrap()
                        .same_request(&request));
                    assert!(Arc::ptr_eq(
                        app.model.receipt_worker.as_ref().unwrap(),
                        &request.token,
                    ));
                    assert!(app.model.scan.is_none());
                    for rejected in [
                        Message::InspectRecovery,
                        Message::PreviewRecovery,
                        Message::InspectReceipt,
                        Message::RestoreRecovery(approved),
                        Message::NativeCheckDone,
                    ] {
                        no_command(app.dispatch(rejected));
                    }
                    assert!(!app.should_exit());

                    let queued = app.model.scan_requested;
                    let next = app.dispatch(message.clone());
                    assert!(app.model.recovery_job.is_none());
                    assert!(app.model.receipt_worker.is_none());
                    assert!(app.model.recovery_inspection.is_none());
                    assert!(app.model.recovery_preview.is_none());
                    assert!(app.model.recovery_error.is_none());
                    assert!(app.model.recovery_outcome.is_none());
                    assert!(app.model.recovery_ack.is_empty());
                    if queued {
                        let token = app.model.scan.as_ref().unwrap().clone();
                        no_command(app.dispatch(message));
                        assert!(Arc::ptr_eq(app.model.scan.as_ref().unwrap(), &token,));
                        finish(&mut app, next);
                    } else {
                        no_command(next);
                        no_command(app.dispatch(message));
                        assert!(app.model.scan.is_none());
                    }
                    if app.root_edited {
                        let command = app.dispatch(Message::OpenRoot);
                        finish(&mut app, command);
                    }
                    no_command(app.dispatch(Message::Select(Product::Retail)));
                    assert!(app.model.recovery_slot_available());
                    assert!(app.model.recovery_inspection.is_none());
                    assert!(app.model.recovery_preview.is_none());
                    assert_no_release_work(&app.model);
                    assert_eq!(tree(wow.path()), before);
                    assert_eq!(tree(data.path()), config_before);
                    assert_eq!(tree(other.path()), other_before);
                }
            }
        }

        #[test]
        fn native_close_discards_preview_and_does_not_wait_for_read_only_checks() {
            for cancel_preview in [false, true] {
                let (wow, data, mut app, _) = fixture(true);
                let before = tree(wow.path());
                let config_before = tree(data.path());
                let preview = plan(&mut app);
                if cancel_preview {
                    no_command(app.dispatch(Message::CancelRecoveryPreview(preview.clone())));
                }
                app.model.set_repository("fixture/Addon".into());
                let check = app.model.begin_release().unwrap(); // No HTTP future is executed.
                let close = native_close_messages(
                    &app,
                    vec![(
                        iced_native::Event::Window(iced_native::window::Event::CloseRequested),
                        iced_native::event::Status::Ignored,
                    )],
                )
                .pop()
                .unwrap();
                no_command(app.dispatch(close));
                assert!(app.should_exit());
                for queued in [
                    Message::RestoreRecovery(preview.clone()),
                    Message::CheckRelease,
                    Message::Rescan,
                    Message::ReleaseDone(check.clone(), Ok("late version".into())),
                ] {
                    no_command(app.dispatch(queued));
                }
                assert!(app.model.recovery_job.is_none());
                assert!(app.model.release_result.is_none());
                assert_eq!(tree(wow.path()), before);
                assert_eq!(tree(data.path()), config_before);
            }
        }

        #[test]
        fn native_close_drains_only_matching_restore_and_blocks_queued_interactions() {
            for failure in 0..3 {
                for stale_display in [false, true] {
                    let (wow, data, mut app, addons) = fixture(true);
                    let preview = plan(&mut app);
                    let command = restore(&mut app, &preview);
                    let request = app.model.pending_recovery.as_ref().unwrap().clone();
                    if failure == 1 {
                        // Preserve the original elsewhere in this fixture while
                        // making the approved backup unavailable to recovery.
                        fs::rename(
                            addons.join(".patchloom-install/backup/Foo"),
                            wow.path().join("preserved-original-for-error"),
                        )
                        .unwrap();
                    }
                    let before = tree(wow.path());
                    let config_before = tree(data.path());
                    let input = app.root_input.clone();
                    let repository = app.model.repository.clone();
                    let receipt_id = app.model.receipt_id.clone();
                    let close = native_close_messages(
                        &app,
                        vec![(
                            iced_native::Event::Window(iced_native::window::Event::CloseRequested),
                            iced_native::event::Status::Captured,
                        )],
                    )
                    .pop()
                    .unwrap();
                    no_command(app.dispatch(close));
                    no_command(app.dispatch(Message::CloseRequested));
                    assert!(!app.should_exit());
                    owns(&app, &request);
                    drop(app.view());
                    for queued in [
                        Message::RootInputChanged("ignored-after-close".into()),
                        Message::OpenRoot,
                        Message::Select(Product::ClassicEra),
                        Message::Rescan,
                        Message::RepositoryChanged("ignored/Repository".into()),
                        Message::CheckRelease,
                        Message::ReceiptIdChanged("ignored".into()),
                        Message::InspectReceipt,
                        Message::InspectRecovery,
                        Message::PreviewRecovery,
                        Message::CancelRecoveryPreview(preview.clone()),
                        Message::RecoveryAckChanged(preview.clone(), "RESTORE".into()),
                        Message::RestoreRecovery(preview.clone()),
                        Message::NativeCheckDone,
                    ] {
                        no_command(app.dispatch(queued));
                        owns(&app, &request);
                        assert!(!app.should_exit());
                    }
                    assert_eq!(app.root_input, input);
                    assert_eq!(app.model.repository, repository);
                    assert_eq!(app.model.receipt_id, receipt_id);
                    assert_eq!(tree(wow.path()), before);

                    let mut foreign = request.clone();
                    foreign.token = Arc::new(());
                    no_command(app.dispatch(Message::RecoveryDone(
                        foreign,
                        RecoveryResult::Restore {
                            outcome: Err("foreign".into()),
                            inspection: Err("foreign".into()),
                        },
                    )));
                    no_command(app.dispatch(Message::RecoveryDone(
                        request.clone(),
                        RecoveryResult::Inspection(Err("wrong action".into())),
                    )));
                    owns(&app, &request);
                    assert!(!app.should_exit());
                    if stale_display {
                        app.model.pending_recovery = None; // Physical ownership remains valid.
                    }
                    // Real queued GUI worker runs checked restore and post-inspection.
                    let mut message = local_message(command);
                    if let Message::RecoveryDone(
                        _,
                        RecoveryResult::Restore {
                            outcome,
                            inspection,
                        },
                    ) = &mut message
                    {
                        assert_eq!(outcome.is_err(), failure == 1);
                        if failure == 2 {
                            *inspection = Err("post-inspection fixture error".into());
                        }
                    } else {
                        panic!("Expected restore completion");
                    }
                    no_command(app.dispatch(message.clone()));
                    assert!(app.should_exit());
                    assert!(app.model.recovery_job.is_none());
                    assert!(app.model.receipt_worker.is_none());
                    assert!(app.model.scan.is_none());
                    assert_eq!(app.model.recovery_outcome.is_none(), stale_display);
                    no_command(app.dispatch(message)); // Duplicate completion cannot reopen work.
                    no_command(app.dispatch(Message::Rescan));
                    assert!(app.should_exit());
                    let after = tree(wow.path());
                    if failure == 1 {
                        assert_eq!(after, before);
                    }
                    for bytes in [
                        b"original".as_slice(),
                        b"original-bar",
                        b"replacement",
                        b"additional",
                    ] {
                        assert!(
                            after
                                .values()
                                .any(|entry| matches!(entry, Entry::File(found) if found == bytes)),
                            "Version bytes must remain preserved"
                        );
                    }
                    assert_eq!(tree(data.path()), config_before);
                }
            }
        }

        fn refuse_restore_scope_changes(app: &mut Preview, preview: &RecoveryPreview) {
            let request = app.model.pending_recovery.as_ref().unwrap().clone();
            let root = app.model.root.clone();
            let input = app.root_input.clone();
            let edited = app.root_edited;
            let selected = app.model.selected;
            let generation = app.model.generation.clone();
            assert!(app.model.restoring());
            for message in [
                Message::RootInputChanged(root.join("refused-draft").to_str().unwrap().into()),
                Message::OpenRoot,
                Message::Select(Product::ClassicProgression),
                Message::Rescan,
                Message::CancelRecoveryPreview(preview.clone()),
                Message::RecoveryAckChanged(preview.clone(), "RESTORE".into()),
                Message::RestoreRecovery(preview.clone()),
                Message::InspectRecovery,
                Message::PreviewRecovery,
                Message::InspectReceipt,
                Message::CheckRelease,
            ] {
                no_command(app.dispatch(message));
                owns(app, &request);
                assert_eq!(app.model.root, root);
                assert_eq!(app.root_input, input);
                assert_eq!(app.root_edited, edited);
                assert_eq!(app.model.selected, selected);
                assert!(Arc::ptr_eq(&app.model.generation, &generation));
                assert!(app.model.scan.is_none());
                assert!(!app.model.scan_requested);
                assert!(app.model.inventory.is_none());
                assert!(app.model.receipt.is_none());
                assert!(app.model.pending_receipt.is_none());
                assert!(app.model.recovery_ack.is_empty());
            }
            drop(app.view());
            no_command(app.dispatch(Message::NativeCheckDone));
            assert!(!app.should_exit());
            owns(app, &request);
        }

        #[test]
        fn recovery_completions_require_full_identity_action_and_slot_ownership() {
            for action in 0..3 {
                let (wow, data, mut app, _) = fixture(true);
                // Obtain genuine historical scan and receipt completions.
                let command = app.dispatch(Message::Rescan);
                let scan_message = local_message(command);
                no_command(app.dispatch(scan_message.clone()));
                no_command(app.dispatch(Message::Select(Product::Retail)));
                no_command(app.dispatch(Message::ReceiptIdChanged("a".repeat(64))));
                let command = app.dispatch(Message::InspectReceipt);
                let receipt_message = local_message(command);
                no_command(app.dispatch(receipt_message.clone()));
                let preview = plan(&mut app);
                let alternate =
                    Arc::new(preflight_restore_originals(preview.approval.root()).unwrap());
                assert!(!Arc::ptr_eq(&alternate, &preview.approval));

                let command = match action {
                    0 => app.dispatch(Message::InspectRecovery),
                    1 => app.dispatch(Message::PreviewRecovery),
                    _ => restore(&mut app, &preview),
                };
                let request = app.model.pending_recovery.as_ref().unwrap().clone();
                owns(&app, &request);
                if action == 2 {
                    refuse_restore_scope_changes(&mut app, &preview);
                }
                let message = local_message(command);
                let result = match &message {
                    Message::RecoveryDone(echo, result) => {
                        assert!(echo.same_request(&request));
                        result.clone()
                    }
                    other => panic!("Expected recovery completion: {:?}", other),
                };
                if action == 2 {
                    assert!(matches!(
                        &result,
                        RecoveryResult::Restore {
                            outcome: Ok(ajour_core::fs::RecoveryRestoreOutcome::RestoredRetained),
                            inspection: Ok(_),
                        }
                    ));
                    // Worker has returned both results; dispatch still owns
                    // the reservation and continues refusing scope changes.
                    refuse_restore_scope_changes(&mut app, &preview);
                }
                let after_worker = tree(wow.path());
                let config_after_worker = tree(data.path());

                for changed_field in 0..6 {
                    let mut foreign = request.clone();
                    match changed_field {
                        0 => foreign.root = foreign.root.join("foreign"),
                        1 => foreign.addons = foreign.addons.join("foreign"),
                        2 => foreign.product = Product::ClassicProgression,
                        3 => foreign.generation = Arc::new(()),
                        4 => foreign.token = Arc::new(()),
                        _ => {
                            foreign.action = if action == 0 {
                                RecoveryAction::Preview
                            } else {
                                RecoveryAction::Inspect
                            }
                        }
                    }
                    no_command(app.dispatch(Message::RecoveryDone(foreign, result.clone())));
                    owns(&app, &request);
                }
                if action == 2 {
                    let mut wrong_ticket = request.clone();
                    wrong_ticket.action = RecoveryAction::Restore(alternate);
                    no_command(app.dispatch(Message::RecoveryDone(wrong_ticket, result.clone())));
                    owns(&app, &request);
                }
                // Deliberately malformed completion, not a claimed core result.
                let wrong_kind = if action == 0 {
                    RecoveryResult::Preview(Err("wrong result variant".into()))
                } else {
                    RecoveryResult::Inspection(Err("wrong result variant".into()))
                };
                no_command(app.dispatch(Message::RecoveryDone(request.clone(), wrong_kind)));
                owns(&app, &request);

                // A receipt/scan envelope cannot complete recovery merely by
                // borrowing its token. Payloads came from actual local workers.
                if let Message::ReceiptDone(mut receipt, result) = receipt_message.clone() {
                    receipt.token = request.token.clone();
                    no_command(app.dispatch(Message::ReceiptDone(receipt, result)));
                } else {
                    panic!("Expected receipt completion");
                }
                if let Message::InventoryDone(mut scan, result) = scan_message {
                    scan.token = request.token.clone();
                    no_command(app.dispatch(Message::InventoryDone(scan, result)));
                } else {
                    panic!("Expected inventory completion");
                }
                owns(&app, &request);
                assert_eq!(tree(wow.path()), after_worker);
                assert_eq!(tree(data.path()), config_after_worker);

                no_command(app.dispatch(message.clone()));
                assert!(app.model.recovery_job.is_none());
                assert!(app.model.pending_recovery.is_none());
                assert!(app.model.receipt_worker.is_none());

                let command = app.dispatch(Message::Rescan);
                let token = app.model.scan.as_ref().unwrap().clone();
                no_command(app.dispatch(message.clone()));
                assert!(Arc::ptr_eq(app.model.scan.as_ref().unwrap(), &token));
                finish(&mut app, command);
                no_command(app.dispatch(Message::Select(Product::Retail)));

                let command = app.dispatch(Message::InspectRecovery);
                let newer = app.model.pending_recovery.as_ref().unwrap().clone();
                assert!(!Arc::ptr_eq(&newer.token, &request.token));
                no_command(app.dispatch(message));
                no_command(app.dispatch(receipt_message));
                owns(&app, &newer);
                let current = local_message(command);
                no_command(app.dispatch(current.clone()));
                let published = app.model.recovery_inspection.as_ref().unwrap().clone();
                no_command(app.dispatch(current));
                assert!(Arc::ptr_eq(
                    app.model.recovery_inspection.as_ref().unwrap(),
                    &published,
                ));
                assert!(app.model.recovery_slot_available());
                assert_no_release_work(&app.model);
                assert_eq!(tree(wow.path()), after_worker);
                assert_eq!(tree(data.path()), config_after_worker);
            }
        }

        #[test]
        fn recovery_preserves_provider_capacity_and_typed_waits_without_http() {
            for action in 0..3 {
                for state in 0..4 {
                    for rate_result in [false, true] {
                        let (wow, data, mut app, _) = fixture(true);
                        let preview = plan(&mut app);
                        no_command(
                            app.dispatch(Message::RepositoryChanged("fixture/addon".into())),
                        );
                        // Construct real provider commands; never poll them.
                        let futures = app.dispatch(Message::CheckRelease).futures();
                        assert_eq!(futures.len(), 1);
                        let first = app.model.pending_release.as_ref().unwrap().clone();
                        drop(futures);
                        let active = if state == 0 {
                            Some(first.clone())
                        } else {
                            let now = Instant::now();
                            let wait = match state {
                                1 => RetryDeadline::At(now),
                                2 => RetryDeadline::from_wait(
                                    now,
                                    std::time::Duration::from_secs(172_800),
                                ),
                                _ => RetryDeadline::Blocked,
                            };
                            no_command(
                                app.dispatch(Message::ReleaseRateLimited(first.clone(), wait)),
                            );
                            if state == 1 {
                                let futures = app.dispatch(Message::CheckRelease).futures();
                                assert_eq!(futures.len(), 1);
                                let current = app.model.pending_release.as_ref().unwrap().clone();
                                drop(futures);
                                Some(current)
                            } else {
                                None
                            }
                        };
                        let deadline = app.model.release_retry_at;
                        let command = match action {
                            0 => app.dispatch(Message::InspectRecovery),
                            1 => app.dispatch(Message::PreviewRecovery),
                            _ => restore(&mut app, &preview),
                        };
                        let recovery = app.model.pending_recovery.as_ref().unwrap().clone();
                        owns(&app, &recovery);
                        assert_eq!(app.model.release_retry_at, deadline);
                        if let Some(provider) = &active {
                            assert!(Arc::ptr_eq(
                                app.model.release_worker.as_ref().unwrap(),
                                &provider.token,
                            ));
                            assert_eq!(app.model.pending_release.is_some(), action != 2);
                        } else {
                            assert!(app.model.release_worker.is_none());
                        }
                        no_command(app.dispatch(Message::CheckRelease));
                        let message = local_message(command);
                        owns(&app, &recovery);
                        no_command(app.dispatch(message));
                        assert_eq!(app.model.release_retry_at, deadline);
                        let after = tree(wow.path());
                        let config_after = tree(data.path());

                        let mut foreign = active.as_ref().unwrap_or(&first).clone();
                        foreign.token = Arc::new(());
                        no_command(app.dispatch(Message::ReleaseRateLimited(
                            foreign,
                            RetryDeadline::Blocked,
                        )));
                        assert_eq!(app.model.release_retry_at, deadline);

                        let expected = if let Some(provider) = active {
                            assert!(Arc::ptr_eq(
                                app.model.release_worker.as_ref().unwrap(),
                                &provider.token,
                            ));
                            let expected = if rate_result {
                                let incoming = if state == 0 {
                                    RetryDeadline::from_wait(
                                        Instant::now(),
                                        std::time::Duration::from_secs(172_800),
                                    )
                                } else {
                                    RetryDeadline::Blocked
                                };
                                no_command(app.dispatch(Message::ReleaseRateLimited(
                                    provider.clone(),
                                    incoming,
                                )));
                                Some(deadline.map_or(incoming, |old| old.max(incoming)))
                            } else {
                                no_command(app.dispatch(Message::ReleaseDone(
                                    provider.clone(),
                                    Ok("fixture-version".into()),
                                )));
                                deadline
                            };
                            assert_eq!(app.model.release_result.is_some(), action != 2);
                            no_command(app.dispatch(Message::ReleaseRateLimited(
                                provider.clone(),
                                RetryDeadline::Blocked,
                            )));
                            no_command(app.dispatch(Message::ReleaseDone(
                                provider,
                                Err("duplicate provider completion".into()),
                            )));
                            expected
                        } else {
                            no_command(app.dispatch(Message::ReleaseRateLimited(
                                first,
                                RetryDeadline::At(Instant::now()),
                            )));
                            deadline
                        };
                        assert_eq!(app.model.release_retry_at, expected);
                        assert!(app.model.release_worker.is_none());
                        assert!(app.model.pending_release.is_none());

                        let command = app.dispatch(Message::Rescan);
                        no_command(app.dispatch(local_message(command)));
                        no_command(app.dispatch(Message::Select(Product::Retail)));
                        drop(app.view());
                        no_command(app.dispatch(Message::NativeCheckDone));
                        assert_no_release_work(&app.model);
                        assert_eq!(app.model.release_retry_at, expected);
                        if expected.is_some_and(|wait| !wait.allows(Instant::now())) {
                            no_command(app.dispatch(Message::CheckRelease));
                            assert_no_release_work(&app.model);
                            assert_eq!(app.model.release_retry_at, expected);
                        }
                        assert_eq!(tree(wow.path()), after);
                        assert_eq!(tree(data.path()), config_after);
                    }
                }
            }
        }

        // Added 2026-10-28; original GPLv3 applies.
        #[test]
        fn restore_outcome_survives_independent_postinspection_failure() {
            use ajour_core::fs::{
                inspect_recovery, restore_originals_checked, RecoveryRestoreOutcome as Outcome,
            };
            use std::fs;

            for kind in 0..4 {
                for post_failure in [false, true] {
                    let (wow, data, mut app, _) = fixture(true);
                    let preview = plan(&mut app);
                    let command = restore(&mut app, &preview);
                    let request = app.model.pending_recovery.as_ref().unwrap().clone();
                    let addons = request.addons.clone();

                    match kind {
                        2 => {
                            let journal = addons.join(".patchloom-install/plan.yml");
                            let mut bytes = fs::read(&journal).unwrap();
                            bytes.extend_from_slice(b"\n# changed after GUI preview\n");
                            fs::write(journal, bytes).unwrap();
                        }
                        3 => {
                            // Preserve the original elsewhere in this fixture.
                            // Inspection can report the missing backup; checked
                            // restore must refuse to guess at its contents.
                            fs::rename(
                                addons
                                    .join(".patchloom-install/backup")
                                    .join(&preview.approval.originals()[0]),
                                wow.path().join("preserved-original-for-error"),
                            )
                            .unwrap();
                        }
                        _ => {}
                    }
                    let before_worker = tree(wow.path());
                    let config_before = tree(data.path());

                    let message = if kind == 1 {
                        // GUI's admitted worker callback always approves its
                        // matching slices. Exercise real core cancellation
                        // separately, then inject its completion envelope.
                        drop(command.futures());
                        let outcome = restore_originals_checked(
                            &addons,
                            preview.approval.as_ref(),
                            |originals, retained| {
                                assert_eq!(originals, preview.approval.originals());
                                assert_eq!(retained, preview.approval.move_aside_if_present(),);
                                false
                            },
                        )
                        .map_err(|error| error.to_string());
                        let inspection = inspect_recovery(&addons)
                            .map(Arc::new)
                            .map_err(|error| error.to_string());
                        Message::RecoveryDone(
                            request.clone(),
                            RecoveryResult::Restore {
                                outcome,
                                inspection,
                            },
                        )
                    } else {
                        // Actual GUI Command -> blocking worker -> checked core
                        // restore -> post-inspection -> completion message.
                        local_message(command)
                    };
                    let (outcome, inspection) = match message {
                        Message::RecoveryDone(
                            echo,
                            RecoveryResult::Restore {
                                outcome,
                                inspection,
                            },
                        ) => {
                            assert!(echo.same_request(&request));
                            (outcome, inspection)
                        }
                        other => panic!("Expected restore completion: {:?}", other),
                    };
                    match kind {
                        0 => assert_eq!(outcome, Ok(Outcome::RestoredRetained)),
                        1 => assert_eq!(outcome, Ok(Outcome::Cancelled)),
                        2 => assert_eq!(outcome, Ok(Outcome::Changed)),
                        _ => assert!(outcome.is_err()),
                    }
                    let inspection = inspection.unwrap();
                    if kind != 0 {
                        assert_eq!(tree(wow.path()), before_worker);
                    }
                    owns(&app, &request);
                    assert!(app.model.recovery_outcome.is_none());
                    assert!(app.model.recovery_post_error.is_none());

                    let expected = outcome.clone();
                    let after_worker = tree(wow.path());
                    // Only this follow-up failure is synthetic. No phase pause
                    // or failed filesystem inspection is claimed by this branch.
                    let follow_up = if post_failure {
                        Err("injected follow-up inspection failure".into())
                    } else {
                        Ok(inspection.clone())
                    };
                    no_command(app.dispatch(Message::RecoveryDone(
                        request.clone(),
                        RecoveryResult::Restore {
                            outcome,
                            inspection: follow_up,
                        },
                    )));
                    assert_eq!(app.model.recovery_outcome, Some(expected.clone()));
                    assert_eq!(
                        app.model.recovery_post_error.as_deref(),
                        post_failure.then_some("injected follow-up inspection failure"),
                    );
                    if post_failure {
                        assert!(app.model.recovery_inspection.is_none());
                    } else {
                        assert!(Arc::ptr_eq(
                            app.model.recovery_inspection.as_ref().unwrap(),
                            &inspection,
                        ));
                    }
                    assert!(app.model.recovery_error.is_none());
                    assert!(app.model.recovery_preview.is_none());
                    assert!(app.model.recovery_ack.is_empty());
                    assert!(app.model.inventory.is_none());
                    assert!(app.model.receipt.is_none());
                    assert!(app.model.receipt_error.is_none());
                    assert!(app.model.recovery_slot_available());

                    let text = recovery_outcome_text(&expected);
                    match kind {
                        0 => {
                            assert!(text.starts_with("Original folders restored."));
                            assert!(text.contains("Updates remain blocked"));
                        }
                        1 => assert_eq!(text, "Restore cancelled. No addon folders were changed.",),
                        2 => {
                            assert!(text.starts_with("Recovery state changed."));
                            assert!(text.contains("No restore was started."));
                        }
                        _ => {
                            assert!(text.starts_with("Restore failed:"));
                            assert!(text.contains("Some folders may already have moved."));
                            assert!(!text.starts_with("Original folders restored."));
                        }
                    }

                    // A duplicate cannot replace either saved result.
                    no_command(app.dispatch(Message::RecoveryDone(
                        request,
                        RecoveryResult::Restore {
                            outcome: Err("duplicate outcome".into()),
                            inspection: Err("duplicate inspection".into()),
                        },
                    )));
                    drop(app.view());
                    assert_eq!(app.model.recovery_outcome, Some(expected));
                    assert_eq!(
                        app.model.recovery_post_error.as_deref(),
                        post_failure.then_some("injected follow-up inspection failure"),
                    );
                    assert_eq!(tree(wow.path()), after_worker);
                    assert_eq!(tree(data.path()), config_before);
                    assert_no_release_work(&app.model);
                }
            }
        }

        #[test]
        fn restore_admission_clears_ownership_displays_and_rejects_delayed_receipts() {
            for prior_error in [false, true] {
                let (wow, data, mut app, _) = fixture(true);
                let preview = plan(&mut app);
                let command = app.dispatch(Message::InspectReceipt);
                let receipt = app.model.pending_receipt.as_ref().unwrap().clone();
                drop(command.futures());
                let result = if prior_error {
                    Err("prior ownership error".into())
                } else {
                    Ok(matching_receipt(&receipt))
                };
                let delayed = Message::ReceiptDone(receipt, result);
                no_command(app.dispatch(delayed.clone()));
                assert_eq!(app.model.receipt.is_some(), !prior_error);
                assert_eq!(app.model.receipt_error.is_some(), prior_error);
                assert!(app.model.inventory.is_some());

                let command = restore(&mut app, &preview);
                let request = app.model.pending_recovery.as_ref().unwrap().clone();
                // These assertions precede polling the restore command.
                assert!(app.model.inventory.is_none());
                assert!(app.model.error.is_none());
                assert!(app.choices.is_empty());
                assert!(app.model.receipt.is_none());
                assert!(app.model.receipt_error.is_none());
                assert!(app.model.pending_receipt.is_none());
                no_command(app.dispatch(delayed.clone()));
                owns(&app, &request);
                assert!(app.model.receipt.is_none());
                assert!(app.model.receipt_error.is_none());

                let completed = local_message(command);
                no_command(app.dispatch(delayed.clone()));
                owns(&app, &request);
                assert!(app.model.receipt.is_none());
                assert!(app.model.receipt_error.is_none());
                no_command(app.dispatch(completed));
                assert!(matches!(
                    app.model.recovery_outcome.as_ref(),
                    Some(Ok(ajour_core::fs::RecoveryRestoreOutcome::RestoredRetained))
                ));
                let report = app.model.recovery_inspection.as_ref().unwrap().clone();
                let after = tree(wow.path());
                let config_after = tree(data.path());
                no_command(app.dispatch(delayed.clone()));
                assert!(app.model.receipt.is_none());
                assert!(app.model.receipt_error.is_none());
                assert!(Arc::ptr_eq(
                    app.model.recovery_inspection.as_ref().unwrap(),
                    &report,
                ));

                let scan = app.dispatch(Message::Rescan);
                let token = app.model.scan.as_ref().unwrap().clone();
                no_command(app.dispatch(delayed.clone()));
                assert!(Arc::ptr_eq(app.model.scan.as_ref().unwrap(), &token));
                finish(&mut app, scan);
                no_command(app.dispatch(Message::Select(Product::Retail)));

                let command = app.dispatch(Message::InspectReceipt);
                let current = app.model.pending_receipt.as_ref().unwrap().clone();
                no_command(app.dispatch(delayed));
                assert!(Arc::ptr_eq(
                    app.model.receipt_worker.as_ref().unwrap(),
                    &current.token,
                ));
                assert!(Arc::ptr_eq(
                    &app.model.pending_receipt.as_ref().unwrap().token,
                    &current.token,
                ));
                assert!(app.model.receipt.is_none());
                assert!(app.model.receipt_error.is_none());
                // Finish the actual current ownership worker, whatever its
                // honest fixture result; the injected old display cannot win.
                finish(&mut app, command);
                assert!(app.model.receipt_worker.is_none());
                assert!(app.model.pending_receipt.is_none());
                assert_eq!(tree(wow.path()), after);
                assert_eq!(tree(data.path()), config_after);
                assert_no_release_work(&app.model);
            }
        }

        #[test]
        fn recovery_paging_preserves_complete_approval_and_rejects_stale_messages() {
            use std::fs;

            let (wow, data, mut app, _) = fixture(true);
            let old_root_preview = plan(&mut app);
            let container = tempfile::tempdir().unwrap();
            let mut root = container.path().canonicalize().unwrap();
            for _ in 0..8 {
                root = root.join("paging-scope-é-界-\u{202e}-abcdefghijklmnop");
            }
            fs::create_dir_all(&root).unwrap();
            for entry in fs::read_dir(wow.path()).unwrap() {
                let entry = entry.unwrap();
                fs::rename(entry.path(), root.join(entry.file_name())).unwrap();
            }
            let addons = root
                .join("_retail_/Interface/AddOns")
                .canonicalize()
                .unwrap();
            let state = ajour_core::fs::inspect_recovery(&addons).unwrap();
            let mut previous = state.previous;
            for index in 0..(2 * FOLDERS_SHOWN + 3) {
                let name = format!("Paging{index:03}_{}_\u{202e}_end", "界".repeat(65));
                assert!(name.len() <= 255);
                let backup = addons.join(".patchloom-install/backup").join(&name);
                fs::create_dir(&backup).unwrap();
                fs::write(backup.join("original.lua"), b"paging original").unwrap();
                previous.push(name);
            }
            fs::write(
                addons.join(".patchloom-install/plan.yml"),
                serde_json::to_vec(&serde_json::json!({
                    "version": 1,
                    "previous": previous,
                    "incoming": state.incoming,
                }))
                .unwrap(),
            )
            .unwrap();
            no_command(app.dispatch(Message::RootInputChanged(root.to_str().unwrap().into())));
            let command = app.dispatch(Message::OpenRoot);
            finish(&mut app, command);
            no_command(app.dispatch(Message::Select(Product::Retail)));
            let command = app.dispatch(Message::InspectRecovery);
            finish(&mut app, command);
            let command = app.dispatch(Message::PreviewRecovery);
            finish(&mut app, command);
            let old = app.model.recovery_preview.as_ref().unwrap().clone();
            // A fresh real preflight replaces request/ticket identity.
            let command = app.dispatch(Message::PreviewRecovery);
            finish(&mut app, command);
            let preview = app.model.recovery_preview.as_ref().unwrap().clone();
            let originals = preview.approval.originals().to_vec();
            let retained = preview.approval.move_aside_if_present().to_vec();
            let canonical = preview.approval.root().to_path_buf();
            let before = tree(container.path());
            let config_before = tree(data.path());
            no_command(app.dispatch(Message::RecoveryAckChanged(
                preview.clone(),
                "RESTORE".into(),
            )));

            let root_pages = recovery_root_pages(&canonical);
            assert!(root_pages > 1);
            let mut displayed_root = String::new();
            for page in 0..root_pages {
                no_command(app.dispatch(Message::RecoveryRootPage(preview.clone(), page)));
                assert_eq!(app.recovery.root_page, page);
                let chunk = recovery_root_chunk(&canonical, page);
                assert!(chunk.is_ascii());
                assert!(chunk.len() <= 10 * ERROR_CHARS);
                displayed_root.push_str(&chunk);
                drop(app.view());
            }
            assert_eq!(
                displayed_root,
                format!("{:?}", canonical.as_os_str())
                    .escape_default()
                    .to_string(),
            );
            let count = originals.len().max(retained.len());
            let last = count.saturating_sub(1) / FOLDERS_SHOWN;
            assert!(last >= 2);
            for page in 0..=last {
                no_command(app.dispatch(Message::RecoveryPage(preview.clone(), page)));
                assert_eq!(app.model.recovery_page, page);
                for folders in [&originals, &retained] {
                    let text = recovery_folder_page(folders, page);
                    let start = page * FOLDERS_SHOWN;
                    let names: Vec<_> = folders.iter().skip(start).take(FOLDERS_SHOWN).collect();
                    if names.is_empty() {
                        assert_eq!(text, "No folders on this page.");
                    } else {
                        assert_eq!(text.lines().count(), names.len());
                        for (offset, (line, name)) in text.lines().zip(names).enumerate() {
                            assert_eq!(
                                line,
                                format!("{}. \"{}\"", start + offset + 1, name.escape_default(),)
                            );
                        }
                    }
                    assert!(text.is_ascii());
                    assert!(!text.contains('\u{202e}'));
                }
                drop(app.view());
            }
            no_command(app.dispatch(Message::RecoveryPage(preview.clone(), usize::MAX)));
            no_command(app.dispatch(Message::RecoveryRootPage(preview.clone(), usize::MAX)));
            assert_eq!(app.model.recovery_page, last);
            assert_eq!(app.recovery.root_page, root_pages - 1);

            no_command(app.dispatch(Message::RecoveryPage(preview.clone(), 1)));
            no_command(app.dispatch(Message::RecoveryRootPage(preview.clone(), 1)));
            let mut stale = vec![old, old_root_preview.clone()];
            for field in 0..7 {
                let mut foreign = preview.clone();
                match field {
                    0 => foreign.request.root = foreign.request.root.join("foreign"),
                    1 => foreign.request.addons = foreign.request.addons.join("foreign"),
                    2 => foreign.request.product = Product::ClassicProgression,
                    3 => foreign.request.generation = Arc::new(()),
                    4 => foreign.request.token = Arc::new(()),
                    5 => foreign.request.action = RecoveryAction::Inspect,
                    _ => {
                        foreign.approval =
                            Arc::new(ajour_core::fs::preflight_restore_originals(&addons).unwrap())
                    }
                }
                stale.push(foreign);
            }
            for stale in stale {
                no_command(app.dispatch(Message::RecoveryPage(stale.clone(), 0)));
                no_command(app.dispatch(Message::RecoveryRootPage(stale, 0)));
                assert_eq!(app.model.recovery_page, 1);
                assert_eq!(app.recovery.root_page, 1);
            }

            // Even current paging messages cannot change controls while the
            // shared physical filesystem slot belongs to a receipt worker.
            let command = app.dispatch(Message::InspectReceipt);
            no_command(app.dispatch(Message::RecoveryPage(preview.clone(), 0)));
            no_command(app.dispatch(Message::RecoveryRootPage(preview.clone(), 0)));
            assert_eq!(app.model.recovery_page, 1);
            assert_eq!(app.recovery.root_page, 1);
            finish(&mut app, command);
            assert_eq!(preview.approval.root(), canonical);
            assert_eq!(preview.approval.originals(), originals);
            assert_eq!(preview.approval.move_aside_if_present(), retained);
            assert!(Arc::ptr_eq(
                &app.model.recovery_preview.as_ref().unwrap().approval,
                &preview.approval,
            ));
            assert_eq!(app.model.recovery_ack, "RESTORE");
            assert_eq!(tree(container.path()), before);
            assert_eq!(tree(data.path()), config_before);

            let command = app.dispatch(Message::RestoreRecovery(preview.clone()));
            let request = app.model.pending_recovery.as_ref().unwrap().clone();
            owns(&app, &request);
            no_command(app.dispatch(Message::RecoveryPage(preview.clone(), 1)));
            no_command(app.dispatch(Message::RecoveryRootPage(preview.clone(), 1)));
            assert_eq!(app.model.recovery_page, 0);
            assert_eq!(app.recovery.root_page, 0);
            finish(&mut app, command);
            assert!(matches!(
                app.model.recovery_outcome.as_ref(),
                Some(Ok(ajour_core::fs::RecoveryRestoreOutcome::RestoredRetained))
            ));
            assert_eq!(preview.approval.originals(), originals);
            assert_eq!(preview.approval.move_aside_if_present(), retained);
            assert_eq!(tree(data.path()), config_before);
            assert_no_release_work(&app.model);
        }

        #[test]
        fn recovery_display_helpers_escape_controls_and_handle_empty_or_extreme_pages() {
            for (name, expected) in [
                ("A\nB", "1. \"A\\nB\""),
                ("界\u{202e}", "1. \"\\u{754c}\\u{202e}\""),
                ("\"\\", r#"1. "\"\\""#),
            ] {
                let names = vec![name.to_string()];
                assert_eq!(recovery_folder_page(&names, 0), expected);
                assert_eq!(names[0], name);
                assert_eq!(
                    recovery_folder_page(&names, usize::MAX),
                    "No folders on this page.",
                );
            }
            assert_eq!(recovery_folder_page(&[], 0), "None.");
            assert_eq!(recovery_folder_page(&[], usize::MAX), "None.");
            let path = PathBuf::from("root\nwith\tcontrols\u{202e}");
            let expected = format!("{:?}", path.as_os_str())
                .escape_default()
                .to_string();
            assert_eq!(recovery_root_pages(&path), 1);
            assert_eq!(recovery_root_chunk(&path, usize::MAX), expected);
            #[cfg(unix)]
            {
                use std::{ffi::OsString, os::unix::ffi::OsStringExt};
                let path = PathBuf::from(OsString::from_vec(b"root-\xff".to_vec()));
                let shown: String = (0..recovery_root_pages(&path))
                    .map(|page| recovery_root_chunk(&path, page))
                    .collect();
                assert_eq!(
                    shown,
                    format!("{:?}", path.as_os_str())
                        .escape_default()
                        .to_string(),
                );
                assert!(!shown.contains('\u{fffd}'));
            }
        }

        #[test]
        fn recovery_stays_idle_on_rootless_startup_render_and_native_timer_messages() {
            let data = tempfile::tempdir().unwrap();
            std::fs::write(data.path().join("config-sentinel"), b"unchanged").unwrap();
            let before = tree(data.path());
            for native_check in [false, true] {
                let (mut app, command) = Preview::new(Flags {
                    root: PathBuf::new(),
                    native_check,
                });
                no_command(command);
                let model_before = format!("{:?}", app.model);
                let generation = app.model.generation.clone();
                for _ in 0..3 {
                    drop(app.view());
                    assert_passive_close_subscription(&app);
                    no_command(app.dispatch(Message::InspectRecovery));
                    no_command(app.dispatch(Message::PreviewRecovery));
                    no_command(app.dispatch(Message::NativeCheckDone));
                    assert!(app.should_exit());
                    assert_eq!(format!("{:?}", app.model), model_before);
                    assert!(Arc::ptr_eq(&app.model.generation, &generation));
                    assert!(app.model.recovery_slot_available());
                    assert_no_release_work(&app.model);
                }
            }
            assert_eq!(tree(data.path()), before);
        }

        #[test]
        fn recovery_preview_and_completed_states_do_not_act_on_render_or_timer() {
            for native_check in [false, true] {
                for state in 0..5 {
                    let (wow, data, _, _) = fixture(true);
                    let before_startup = tree(wow.path());
                    let config_before = tree(data.path());
                    let (mut app, command) = Preview::new(Flags {
                        root: wow.path().into(),
                        native_check,
                    });
                    assert!(app.model.recovery_job.is_none());
                    assert!(app.model.pending_recovery.is_none());
                    assert!(app.model.recovery_inspection.is_none());
                    assert!(app.model.recovery_preview.is_none());
                    assert_no_release_work(&app.model);
                    let message = local_message(command);
                    assert!(matches!(&message, Message::InventoryDone(..)));
                    no_command(app.dispatch(message));
                    no_command(app.dispatch(Message::Select(Product::Retail)));
                    assert_eq!(tree(wow.path()), before_startup);
                    assert!(app.model.recovery_inspection.is_none());
                    assert!(app.model.recovery_outcome.is_none());

                    if state != 0 {
                        let preview = plan(&mut app);
                        match state {
                            1 => {
                                no_command(app.dispatch(Message::RecoveryAckChanged(
                                    preview,
                                    "RESTORE".into(),
                                )))
                            }
                            2 => {
                                let command = restore(&mut app, &preview);
                                finish(&mut app, command);
                                assert!(matches!(
                                    app.model.recovery_outcome.as_ref(),
                                    Some(Ok(
                                        ajour_core::fs::RecoveryRestoreOutcome::RestoredRetained
                                    ))
                                ));
                            }
                            3 => {
                                no_command(app.dispatch(Message::CancelRecoveryPreview(preview)));
                                assert!(app.model.recovery_preview.is_none());
                                assert!(app.model.recovery_error.is_some());
                            }
                            _ => {
                                std::fs::write(
                                    preview.approval.root().join(".patchloom-install/plan.yml"),
                                    b"version: [invalid",
                                )
                                .unwrap();
                                let command = restore(&mut app, &preview);
                                finish(&mut app, command);
                                assert!(matches!(
                                    app.model.recovery_outcome.as_ref(),
                                    Some(Err(_))
                                ));
                                assert!(app.model.recovery_post_error.is_some());
                            }
                        }
                    }
                    let after = tree(wow.path());
                    let model_before = format!("{:?}", app.model);
                    let generation = app.model.generation.clone();
                    let ticket = app
                        .model
                        .recovery_preview
                        .as_ref()
                        .map(|preview| preview.approval.clone());
                    for _ in 0..3 {
                        drop(app.view());
                        assert_passive_close_subscription(&app);
                        no_command(app.dispatch(Message::NativeCheckDone));
                        assert!(app.should_exit());
                        assert_eq!(format!("{:?}", app.model), model_before);
                        assert!(Arc::ptr_eq(&app.model.generation, &generation));
                        if let Some(ticket) = &ticket {
                            assert!(Arc::ptr_eq(
                                &app.model.recovery_preview.as_ref().unwrap().approval,
                                ticket,
                            ));
                            assert_eq!(app.model.recovery_ack, "RESTORE");
                        }
                        assert!(app.model.recovery_slot_available());
                        assert!(app.model.pending_recovery.is_none());
                        assert!(app.model.pending_receipt.is_none());
                        assert!(app.model.scan.is_none());
                        assert!(!app.model.scan_requested);
                        assert_no_release_work(&app.model);
                        assert_eq!(tree(wow.path()), after);
                        assert_eq!(tree(data.path()), config_before);
                    }
                }
            }
        }
    }

    // Added 2026-10-28; original GPLv3 applies.
    mod startup_root_tests {
        use super::*;
        use std::{path::Path, time::Duration};

        fn no_command(command: Command<Message>) {
            assert!(command.futures().is_empty());
        }

        // Call only with a command returned by constructor/Open/Rescan or
        // InspectReceipt. Never pass CheckRelease commands here.
        fn local_message(command: Command<Message>) -> Message {
            let mut futures = command.futures();
            assert_eq!(futures.len(), 1);
            let message = async_std::task::block_on(futures.remove(0));
            assert!(matches!(
                &message,
                Message::InventoryDone(..) | Message::ReceiptDone(..)
            ));
            message
        }

        fn finish_local(app: &mut Preview, command: Command<Message>) {
            let message = local_message(command);
            no_command(app.dispatch(message));
        }

        fn edit(app: &mut Preview, root: &Path) {
            no_command(app.dispatch(Message::RootInputChanged(
                root.to_str().expect("UTF-8 fixture path").into(),
            )));
        }

        fn open(app: &mut Preview, root: &Path) -> Command<Message> {
            edit(app, root);
            app.dispatch(Message::OpenRoot)
        }

        fn owns(worker: Option<&Arc<()>>, token: &Arc<()>) {
            assert!(worker.is_some_and(|worker| Arc::ptr_eq(worker, token)));
        }

        fn invalidated(app: &Preview) {
            assert!(app.model.inventory.is_none());
            assert!(app.model.selected.is_none());
            assert!(app.model.error.is_none());
            assert!(app.choices.is_empty());
            assert!(app.model.receipt.is_none());
            assert!(app.model.receipt_error.is_none());
            assert!(app.model.pending_receipt.is_none());
            assert!(app.model.release_result.is_none());
            assert!(app.model.pending_release.is_none());
        }

        fn click_release(app: &mut Preview) -> ReleaseRequest {
            let futures = app.dispatch(Message::CheckRelease).futures();
            assert_eq!(futures.len(), 1);
            let request = app.model.pending_release.as_ref().unwrap().clone();
            owns(app.model.release_worker.as_ref(), &request.token);
            drop(futures);
            request
        }

        fn tree(root: &Path) -> Vec<(PathBuf, Option<Vec<u8>>)> {
            fn visit(root: &Path, path: &Path, entries: &mut Vec<(PathBuf, Option<Vec<u8>>)>) {
                entries.push((
                    path.strip_prefix(root).unwrap().into(),
                    if path.is_file() {
                        Some(std::fs::read(path).unwrap())
                    } else {
                        None
                    },
                ));
                if path.is_dir() {
                    for entry in std::fs::read_dir(path).unwrap() {
                        visit(root, &entry.unwrap().path(), entries);
                    }
                }
            }
            let mut entries = Vec::new();
            visit(root, root, &mut entries);
            entries.sort();
            entries
        }

        #[test]
        fn empty_constructor_view_and_empty_open_do_no_work_or_fixture_writes() {
            for config_state in 0..3 {
                let temp = tempfile::tempdir().unwrap();
                let config = temp.path().join("isolated-config");
                if config_state != 0 {
                    std::fs::create_dir(&config).unwrap();
                }
                if config_state == 2 {
                    std::fs::write(config.join("existing-config"), b"preserve\n").unwrap();
                }
                let before = tree(temp.path());

                let (mut app, command) = Preview::new(Flags {
                    root: PathBuf::new(),
                    native_check: false,
                });
                no_command(command);
                invalidated(&app);
                assert!(app.model.root.as_os_str().is_empty());
                assert!(app.root_input.is_empty());
                assert!(!app.root_edited);
                assert!(app.model.scan.is_none());
                assert!(app.model.receipt_worker.is_none());
                assert!(app.model.release_worker.is_none());
                assert!(!app.model.scan_requested);
                assert!(app.model.release_retry_at.is_none());
                assert_passive_close_subscription(&app);
                drop(app.view());

                no_command(app.dispatch(Message::CheckRelease));
                no_command(app.dispatch(Message::InspectReceipt));
                no_command(app.dispatch(Message::Rescan));
                assert!(app.model.error.is_some());
                assert!(app.model.scan.is_none());

                edit(&mut app, &temp.path().join("not-opened"));
                no_command(app.dispatch(Message::RootInputChanged(String::new())));
                invalidated(&app);
                no_command(app.dispatch(Message::OpenRoot));
                assert!(app.model.error.is_some());
                assert!(app.model.inventory.is_none());
                assert!(app.model.scan.is_none());
                assert!(!app.model.scan_requested);
                assert!(app.model.start_scan().is_none());
                assert_no_release_work(&app.model);
                assert_eq!(tree(temp.path()), before);
            }
        }

        #[test]
        fn explicit_open_discovers_all_modern_products_and_rejects_bad_roots() {
            let (temp, _) = receipt_model();
            for folder in ["_classic_era_", "_anniversary_"] {
                std::fs::create_dir_all(temp.path().join(folder).join("Interface").join("AddOns"))
                    .unwrap();
            }
            std::fs::write(
                temp.path().join(".build.info"),
                concat!(
                    "Active!DEC:1|Product!STRING:0|Version!STRING:0\n",
                    "1|wow|12.1.5.70575\n",
                    "1|wow_classic|5.5.4.70579\n",
                    "1|wow_classic_era|1.15.7.60000\n",
                    "1|wow_anniversary|2.5.5.60000\n",
                ),
            )
            .unwrap();
            let before = tree(temp.path());
            let (mut app, command) = Preview::new(Flags {
                root: PathBuf::new(),
                native_check: false,
            });
            no_command(command);

            edit(&mut app, temp.path());
            assert!(app.model.root.as_os_str().is_empty());
            assert!(app.model.scan.is_none());
            assert_no_release_work(&app.model);
            let command = app.dispatch(Message::OpenRoot);
            finish_local(&mut app, command);

            let report = app.model.inventory.as_ref().unwrap();
            assert!(!app.model.needs_review());
            assert_eq!(report.installations.len(), 4);
            for product in [
                Product::Retail,
                Product::ClassicProgression,
                Product::ClassicEra,
                Product::Anniversary,
            ] {
                assert!(report.installations.iter().any(|i| i.product == product));
            }
            assert_eq!(app.choices.len(), 4);
            assert!(app.model.selected.is_none());
            no_command(app.dispatch(Message::Select(Product::Anniversary)));
            assert_eq!(
                app.model.installation().unwrap().product,
                Product::Anniversary
            );
            assert_no_release_work(&app.model);
            assert_eq!(tree(temp.path()), before);

            let missing = temp.path().join("missing-root");
            let command = open(&mut app, &missing);
            finish_local(&mut app, command);
            assert!(app.model.error.is_some());
            assert!(app.model.inventory.is_none());
            assert!(app.model.selected.is_none());
            assert!(app.choices.is_empty());
            assert!(!missing.exists());
            assert_no_release_work(&app.model);
            assert_eq!(tree(temp.path()), before);

            std::fs::write(
                temp.path().join(".build.info"),
                concat!(
                    "Active!DEC:1|Product!STRING:0|Version!STRING:0\n",
                    "1|wow_unknown|13.0.0.1\n",
                ),
            )
            .unwrap();
            let before = tree(temp.path());
            let command = open(&mut app, temp.path());
            finish_local(&mut app, command);
            assert!(app.model.needs_review());
            let report = app.model.inventory.as_ref().unwrap();
            assert!(report.installations.is_empty());
            assert!(report
                .issues
                .iter()
                .any(|issue| issue.contains("Unknown product/version family")));
            assert!(app.model.selected.is_none());
            assert!(app.choices.is_empty());
            drop(app.view());
            assert_no_release_work(&app.model);
            assert_eq!(tree(temp.path()), before);
        }

        #[test]
        fn draft_edits_immediately_clear_success_and_failure_displays() {
            for failure in [false, true] {
                let (temp, model) = release_fixture();
                let mut app = Preview::from_model(model, false);
                let receipt = app.model.begin_receipt().unwrap();
                let receipt_result = if failure {
                    Err("old receipt failure".into())
                } else {
                    Ok(matching_receipt(&receipt))
                };
                no_command(app.dispatch(Message::ReceiptDone(receipt, receipt_result)));
                let release = click_release(&mut app);
                let release_result = if failure {
                    Err("old release failure".into())
                } else {
                    Ok("old version".into())
                };
                no_command(app.dispatch(Message::ReleaseDone(release, release_result)));
                assert!(app.model.release_result.is_some());
                assert!(app.model.receipt.is_some() || app.model.receipt_error.is_some());

                let original = app.model.root.clone();
                let generation = app.model.generation.clone();
                let repository = app.model.repository.clone();
                let receipt_id = app.model.receipt_id.clone();
                edit(&mut app, &temp.path().join("new-root"));
                invalidated(&app);
                assert_eq!(app.model.root, original);
                assert!(!Arc::ptr_eq(&generation, &app.model.generation));
                assert_eq!(app.model.repository, repository);
                assert_eq!(app.model.receipt_id, receipt_id);
                assert!(app.root_edited);
                assert!(app.model.scan.is_none());
                assert!(!app.model.scan_requested);
                no_command(app.dispatch(Message::Rescan));
                no_command(app.dispatch(Message::Select(Product::Retail)));
                no_command(app.dispatch(Message::CheckRelease));
                assert_no_release_work(&app.model);
                drop(app.view());
            }
        }

        #[test]
        fn busy_scan_coalesces_latest_open_and_rejects_foreign_and_duplicate_results() {
            let (a, _) = receipt_model();
            let (b, _) = receipt_model();
            let (c, _) = receipt_model();
            let (mut app, command) = Preview::new(Flags {
                root: a.path().into(),
                native_check: false,
            });
            let old_message = local_message(command);
            let old = match &old_message {
                Message::InventoryDone(request, _) => request.clone(),
                other => panic!("Expected scan: {:?}", other),
            };

            no_command(open(&mut app, b.path()));
            no_command(open(&mut app, c.path()));
            invalidated(&app);
            assert_eq!(app.model.root, c.path());
            assert!(app.model.scan_requested);
            owns(app.model.scan.as_ref(), &old.token);

            let mut foreign = old.clone();
            foreign.token = Arc::new(());
            no_command(app.dispatch(Message::InventoryDone(foreign, Err("foreign scan".into()))));
            owns(app.model.scan.as_ref(), &old.token);
            assert!(app.model.scan_requested);

            let latest_command = app.dispatch(old_message.clone());
            invalidated(&app);
            assert!(!app.model.scan_requested);
            let latest_token = app.model.scan.as_ref().unwrap().clone();
            assert!(!Arc::ptr_eq(&latest_token, &old.token));

            no_command(app.dispatch(old_message));
            owns(app.model.scan.as_ref(), &latest_token);
            let latest_message = local_message(latest_command);
            match &latest_message {
                Message::InventoryDone(request, _) => {
                    assert_eq!(request.root, c.path());
                    owns(app.model.scan.as_ref(), &request.token);
                    let mut foreign = request.clone();
                    foreign.token = Arc::new(());
                    no_command(app.dispatch(Message::InventoryDone(
                        foreign,
                        Err("foreign latest scan".into()),
                    )));
                }
                other => panic!("Expected latest scan: {:?}", other),
            }
            owns(app.model.scan.as_ref(), &latest_token);
            no_command(app.dispatch(latest_message.clone()));
            let published = app.model.inventory.as_ref().unwrap().clone();
            assert_eq!(published.root, c.path().canonicalize().unwrap());
            assert_eq!(app.choices.len(), 2);
            assert!(app.model.selected.is_none());
            no_command(app.dispatch(latest_message));
            assert!(Arc::ptr_eq(
                app.model.inventory.as_ref().unwrap(),
                &published
            ));
            assert_no_release_work(&app.model);
        }

        #[test]
        fn draft_away_back_aba_rejects_scan_even_without_a_queued_open() {
            let (a, _) = receipt_model();
            let (b, _) = receipt_model();
            let (mut app, command) = Preview::new(Flags {
                root: a.path().into(),
                native_check: false,
            });
            let message = local_message(command);
            let old_generation = app.model.generation.clone();

            edit(&mut app, b.path());
            edit(&mut app, a.path());
            assert_eq!(app.model.root, a.path());
            assert!(!app.model.scan_requested);
            assert!(!Arc::ptr_eq(&old_generation, &app.model.generation));
            no_command(app.dispatch(message.clone()));
            invalidated(&app);
            assert!(app.model.scan.is_none());
            no_command(app.dispatch(message));
            invalidated(&app);

            let command = app.dispatch(Message::OpenRoot);
            finish_local(&mut app, command);
            assert_eq!(
                app.model.inventory.as_ref().unwrap().root,
                a.path().canonicalize().unwrap()
            );
            assert!(app.model.selected.is_none());
            assert_no_release_work(&app.model);
        }

        #[test]
        fn receipt_holds_shared_slot_until_matching_completion_then_latest_root_scans() {
            let (_a, model) = release_fixture();
            let (b, _) = receipt_model();
            let (c, _) = receipt_model();
            let mut app = Preview::from_model(model, false);
            let receipt_command = app.dispatch(Message::InspectReceipt);
            let old = app.model.pending_receipt.as_ref().unwrap().clone();

            no_command(open(&mut app, b.path()));
            no_command(open(&mut app, c.path()));
            invalidated(&app);
            owns(app.model.receipt_worker.as_ref(), &old.token);
            assert!(app.model.scan.is_none());
            assert!(app.model.scan_requested);
            no_command(app.dispatch(Message::InspectReceipt));
            no_command(app.dispatch(Message::CheckRelease));

            let mut foreign = old.clone();
            foreign.token = Arc::new(());
            no_command(app.dispatch(Message::ReceiptDone(foreign, Err("foreign receipt".into()))));
            owns(app.model.receipt_worker.as_ref(), &old.token);
            assert!(app.model.scan.is_none());

            let message = local_message(receipt_command);
            let scan = app.dispatch(message.clone());
            assert!(app.model.receipt_worker.is_none());
            assert!(app.model.receipt.is_none());
            assert!(app.model.receipt_error.is_none());
            let scan_token = app.model.scan.as_ref().unwrap().clone();
            no_command(app.dispatch(message));
            owns(app.model.scan.as_ref(), &scan_token);
            finish_local(&mut app, scan);
            assert_eq!(
                app.model.inventory.as_ref().unwrap().root,
                c.path().canonicalize().unwrap()
            );

            no_command(app.dispatch(Message::Select(Product::Retail)));
            let current_command = app.dispatch(Message::InspectReceipt);
            let current = app.model.pending_receipt.as_ref().unwrap().clone();
            no_command(app.dispatch(Message::ReceiptDone(
                old.clone(),
                Ok(matching_receipt(&old)),
            )));
            owns(app.model.receipt_worker.as_ref(), &current.token);
            assert!(app
                .model
                .pending_receipt
                .as_ref()
                .is_some_and(|pending| Arc::ptr_eq(&pending.token, &current.token)));
            finish_local(&mut app, current_command);
            assert!(app.model.receipt.is_none());
            assert!(app.model.receipt_error.is_some());
            assert_no_release_work(&app.model);
        }

        #[test]
        fn provider_reservation_survives_root_aba_and_only_manual_click_readmits() {
            let (a, model) = release_fixture();
            let (b, _) = receipt_model();
            let mut app = Preview::from_model(model, false);
            let old = click_release(&mut app);

            for root in [b.path(), a.path()] {
                let command = open(&mut app, root);
                owns(app.model.release_worker.as_ref(), &old.token);
                assert!(app.model.pending_release.is_none());
                finish_local(&mut app, command);
                no_command(app.dispatch(Message::Select(Product::Retail)));
                no_command(app.dispatch(Message::CheckRelease));
                owns(app.model.release_worker.as_ref(), &old.token);
            }
            assert_eq!(app.model.root, old.root);
            assert_eq!(app.model.selected, Some(old.product));
            assert!(!Arc::ptr_eq(
                app.model.inventory.as_ref().unwrap(),
                &old.inventory
            ));

            let mut foreign = old.clone();
            foreign.token = Arc::new(());
            no_command(app.dispatch(Message::ReleaseDone(foreign, Ok("foreign".into()))));
            owns(app.model.release_worker.as_ref(), &old.token);
            no_command(app.dispatch(Message::ReleaseDone(old.clone(), Ok("stale ABA".into()))));
            assert_no_release_work(&app.model);

            drop(app.view());
            no_command(app.dispatch(Message::RepositoryChanged(old.repository.clone())));
            no_command(app.dispatch(Message::ReceiptIdChanged("b".repeat(64))));
            assert_no_release_work(&app.model);
            let current = click_release(&mut app);
            no_command(app.dispatch(Message::ReleaseRateLimited(old, RetryDeadline::Blocked)));
            owns(app.model.release_worker.as_ref(), &current.token);
            assert!(app.model.release_retry_at.is_none());
            no_command(app.dispatch(Message::CheckRelease));
            no_command(app.dispatch(Message::NativeCheckDone));
            owns(app.model.release_worker.as_ref(), &current.token);
            no_command(app.dispatch(Message::ReleaseDone(current.clone(), Ok("current".into()))));
            no_command(app.dispatch(Message::ReleaseDone(current, Err("duplicate".into()))));
            assert_eq!(
                app.model
                    .release_result
                    .as_ref()
                    .unwrap()
                    .as_ref()
                    .unwrap()
                    .version,
                "current"
            );
        }

        #[test]
        fn stale_rate_completion_preserves_finite_or_blocked_wait_across_root_flow() {
            let finite = RetryDeadline::from_wait(Instant::now(), Duration::from_secs(172_800));
            for deadline in [finite, RetryDeadline::Blocked] {
                let (a, model) = release_fixture();
                let (b, _) = receipt_model();
                let (c, _) = receipt_model();
                let roots = [a.path(), b.path(), c.path()];
                let before: Vec<_> = roots.iter().map(|root| tree(root)).collect();
                let mut app = Preview::from_model(model, false);
                let old = click_release(&mut app);
                let scan_b = open(&mut app, b.path());
                owns(app.model.release_worker.as_ref(), &old.token);
                assert!(app.model.pending_release.is_none());

                let mut foreign = old.clone();
                foreign.token = Arc::new(());
                no_command(
                    app.dispatch(Message::ReleaseRateLimited(foreign, RetryDeadline::Blocked)),
                );
                owns(app.model.release_worker.as_ref(), &old.token);
                assert!(app.model.release_retry_at.is_none());

                no_command(app.dispatch(Message::ReleaseRateLimited(old.clone(), deadline)));
                assert!(app.model.release_worker.is_none());
                assert!(app.model.release_result.is_none());
                assert_eq!(app.model.release_retry_at, Some(deadline));

                no_command(open(&mut app, c.path()));
                assert_eq!(app.model.release_retry_at, Some(deadline));
                no_command(app.dispatch(Message::ReleaseRateLimited(
                    old.clone(),
                    RetryDeadline::Blocked,
                )));
                assert_eq!(app.model.release_retry_at, Some(deadline));

                let scan_c = app.dispatch(local_message(scan_b));
                assert!(app.model.inventory.is_none());
                assert_eq!(app.model.root, c.path());
                finish_local(&mut app, scan_c);
                no_command(app.dispatch(Message::Select(Product::Retail)));
                no_command(app.dispatch(Message::CheckRelease));
                assert_no_release_work(&app.model);

                let command = open(&mut app, a.path());
                assert_eq!(app.model.release_retry_at, Some(deadline));
                finish_local(&mut app, command);
                no_command(app.dispatch(Message::Select(Product::Retail)));
                no_command(app.dispatch(Message::CheckRelease));
                no_command(app.dispatch(Message::ReleaseDone(old, Ok("late success".into()))));
                assert_no_release_work(&app.model);

                no_command(app.dispatch(Message::RootInputChanged(String::new())));
                assert_eq!(app.model.release_retry_at, Some(deadline));
                no_command(app.dispatch(Message::OpenRoot));
                assert!(app.model.error.is_some());
                assert!(app.model.scan.is_none());
                assert_eq!(app.model.release_retry_at, Some(deadline));
                assert_no_release_work(&app.model);
                for (root, bytes) in roots.iter().zip(&before) {
                    assert_eq!(&tree(root), bytes);
                }
            }
        }
    }
}
