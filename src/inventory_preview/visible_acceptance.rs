//! Offline, disposable native acceptance wrapper. Never a runtime entry point.
//! Loaded only by debug fixture unit tests; original GPLv3 applies, 2026-11-06.
use super::*;
use std::sync::Condvar;
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Point {
    Preparation,
    BeforeApply,
    PostInspection,
}

#[derive(Default, Debug)]
struct State {
    armed: Option<Point>,
    reached: Option<Point>,
    closed: bool,
}

#[derive(Default, Debug)]
struct Gate(Mutex<State>, Condvar);
impl Gate {
    fn arm(&self, point: Point) {
        let mut state = self.0.lock().unwrap();
        if !state.closed && state.reached.is_none() {
            state.armed = Some(point);
        }
    }
    fn wait(&self, point: Point) -> WorkResult<()> {
        self.wait_for(point, Duration::from_secs(120))
    }
    fn wait_for(&self, point: Point, timeout: Duration) -> WorkResult<()> {
        let mut state = self.0.lock().unwrap();
        if !state.closed && state.armed == Some(point) {
            state.reached = Some(point);
            let (next, result) = self
                .1
                .wait_timeout_while(state, timeout, |s| !s.closed && s.armed == Some(point))
                .unwrap();
            state = next;
            if result.timed_out() && state.armed == Some(point) {
                state.armed = None;
                return Err(std::io::Error::other("Fixture gate timed out").into());
            }
        }
        if state.closed {
            return Err(std::io::Error::other("Fixture wrapper closed").into());
        }
        Ok(())
    }
    fn release(&self) {
        let mut state = self.0.lock().unwrap();
        state.armed = None;
        state.reached = None;
        self.1.notify_all();
    }
    fn close(&self) {
        self.0.lock().unwrap().closed = true;
        self.1.notify_all();
    }
    fn reached(&self) -> Option<Point> {
        self.0.lock().unwrap().reached
    }
}

#[derive(Clone, Debug)]
enum Event {
    App(Message),
    FixtureRelease(bool),
    Hold(Point),
    Continue,
    Tick,
}

struct Acceptance {
    fixture: Fixture,
    gate: Arc<Gate>,
    bytes: Vec<u8>,
    buttons: [button::State; 6],
    scroll: scrollable::State,
}
impl Drop for Acceptance {
    fn drop(&mut self) {
        // Wake blocked workers. Their Arc<TempDir> keeps all fixture paths alive
        // until they return; cancellation cannot delete beneath a live worker.
        self.gate.close();
    }
}
impl Acceptance {
    fn create() -> Self {
        Self {
            fixture: async_std::task::block_on(fixture()),
            gate: Arc::new(Gate::default()),
            bytes: Vec::new(),
            buttons: Default::default(),
            scroll: scrollable::State::new(),
        }
    }
    fn choose_release(&mut self, update: bool) {
        let app = &mut self.fixture.app;
        if app.close_requested {
            return;
        }
        if let Some(request) = app.model.begin_release() {
            // Synthetic metadata only. Never construct or poll release_command.
            let entries = if update {
                vec![
                    ("Alpha/Alpha.toc", "## Interface: 120105\n## Version: 3\n"),
                    ("New/New.toc", "## Interface: 120105\n"),
                ]
            } else {
                vec![
                    ("Alpha/Alpha.toc", "## Interface: 120105\n## Version: 2\n"),
                    ("Old/Old.toc", "## Interface: 120105\n"),
                ]
            };
            let (candidate, bytes) = ajour_core::github_releases::fixture_support::package(
                app.model.installation().unwrap(),
                "fixture/addon",
                if update { "v3" } else { "v2" },
                &entries,
            )
            .unwrap();
            self.bytes = bytes;
            assert!(app
                .dispatch(Message::ReleaseCandidateDone(request, Arc::new(candidate)))
                .futures()
                .is_empty());
        }
    }
    fn handle(&mut self, event: Event) -> Command<Event> {
        match event {
            Event::FixtureRelease(update) => self.choose_release(update),
            Event::Hold(point) if self.fixture.app.model.install.job.is_none() => {
                self.gate.arm(point)
            }
            Event::Continue => self.gate.release(),
            Event::App(message) => return self.app_message(message).map(Event::App),
            Event::Hold(_) | Event::Tick => {}
        }
        Command::none()
    }
    fn app_message(&mut self, message: Message) -> Command<Message> {
        // Fixed owned scope; no path/provider/configuration controls are shown.
        // Fail closed even if a future view starts emitting another message.
        if self.fixture.app.model.root != self.fixture.temp.path().join("wow") {
            self.gate.close();
            self.fixture.app.dispatch(Message::CloseRequested);
            return Command::none();
        }
        if !matches!(
            &message,
            Message::Install(_)
                | Message::CloseRequested
                | Message::InventoryDone(..)
                | Message::ReceiptDone(..)
                | Message::Rescan
                | Message::InspectReceipt
        ) {
            return Command::none();
        }
        let cancelling = matches!(&message, Message::Install(InstallMessage::Cancel(_)));
        let cache = self.fixture.cache.clone();
        let bytes = self.bytes.clone();
        let prepare_gate = self.gate.clone();
        let apply_gate = self.gate.clone();
        let post_gate = self.gate.clone();
        let prepare_scope = self.fixture.temp.clone();
        let apply_scope = self.fixture.temp.clone();
        let command = self.fixture.app.dispatch_with_install_workers(
            message,
            move |r| {
                prepare_command_with(r, move |r| {
                    let _scope = prepare_scope;
                    prepare_gate.wait(Point::Preparation)?;
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
            move |r, approval| {
                apply_command_with(
                    r,
                    approval,
                    move || apply_gate.wait(Point::BeforeApply),
                    move || {
                        // Commit has finished. Never abort or skip post-inspection
                        // because a gate times out or the wrapper is being dropped.
                        let _ = post_gate.wait(Point::PostInspection);
                        apply_scope
                    },
                )
            },
        );
        if cancelling {
            self.gate.release();
        }
        command
    }
}

impl Application for Acceptance {
    type Executor = iced::executor::Default;
    type Message = Event;
    type Flags = Option<Self>;
    fn new(flags: Option<Self>) -> (Self, Command<Event>) {
        (flags.unwrap_or_else(Self::create), Command::none())
    }
    fn title(&self) -> String {
        "Patchloom | Offline acceptance fixture".into()
    }
    fn subscription(&self) -> Subscription<Event> {
        Subscription::batch(vec![
            self.fixture.app.subscription().map(Event::App),
            iced_futures::time::every(Duration::from_millis(100)).map(|_| Event::Tick),
        ])
    }
    fn should_exit(&self) -> bool {
        self.fixture.app.should_exit()
    }
    fn update(&mut self, message: Event, _: &mut Clipboard) -> Command<Event> {
        self.handle(message)
    }
    fn view(&mut self) -> Element<'_, Event> {
        let [fresh, update, preparation, apply, inspection, release] = &mut self.buttons;
        let controls = Row::new()
            .spacing(8)
            .push(
                Button::new(fresh, Text::new("Fixture v2")).on_press(Event::FixtureRelease(false)),
            )
            .push(
                Button::new(update, Text::new("Fixture v3 update"))
                    .on_press(Event::FixtureRelease(true)),
            );
        let gates = Row::new()
            .spacing(8)
            .push(
                Button::new(preparation, Text::new("Hold preparation"))
                    .on_press(Event::Hold(Point::Preparation)),
            )
            .push(
                Button::new(apply, Text::new("Hold before install"))
                    .on_press(Event::Hold(Point::BeforeApply)),
            )
            .push(
                Button::new(inspection, Text::new("Hold after install"))
                    .on_press(Event::Hold(Point::PostInspection)),
            )
            .push(Button::new(release, Text::new("Continue")).on_press(Event::Continue));
        let app = &mut self.fixture.app;
        // The production install panel and dispatcher are reused, without the
        // production provider/root/recovery panels or any live network button.
        let panel = app.install_controls.view(&app.model, false).map(Event::App);
        let content = Column::new()
            .spacing(12)
            .padding(16)
            .push(Text::new(
                "Offline fixture only: temporary WoW, cache and app-data folders.",
            ))
            .push(Text::new(format!(
                "Gate: {:?}; writer/post-read reservation: {}; close requested: {}",
                self.gate.reached(),
                app.model.restoring(),
                app.close_requested
            )))
            .push(controls)
            .push(gates)
            .push(panel);
        Scrollable::new(&mut self.scroll).push(content).into()
    }
}

// The ONLY native entry. Routine cargo test and the exact bridge invocations
// skip it. This milestone compiles it with --no-run, never executes it.
#[test]
#[ignore = "opens a native window; requires explicit user approval for this exact offline fixture"]
fn visible_offline_acceptance_requires_specific_approval() {
    assert_eq!(
        std::env::var("PATCHLOOM_VISIBLE_FIXTURE_APPROVED").as_deref(),
        Ok("yes"),
        "No native fixture window without explicit approval"
    );
    let harness = Acceptance::create();
    // Process-local only, in this ignored native entry. No routine headless
    // test mutates environment variables or invokes this path.
    let _data_scope = DataScope::enter(&harness.fixture.data);
    let mut settings = Settings::with_flags(Some(harness));
    settings.exit_on_close_request = false;
    settings.window.size = (1100, 780);
    Acceptance::run(settings).unwrap();
}

struct DataScope([Option<std::ffi::OsString>; 2]);
impl DataScope {
    fn enter(path: &std::path::Path) -> Self {
        let old = Self([
            std::env::var_os("APPDATA"),
            std::env::var_os("LOCALAPPDATA"),
        ]);
        std::env::set_var("APPDATA", path);
        std::env::set_var("LOCALAPPDATA", path);
        old
    }
}
impl Drop for DataScope {
    fn drop(&mut self) {
        for (name, value) in ["APPDATA", "LOCALAPPDATA"].iter().zip(&self.0) {
            if let Some(value) = value {
                std::env::set_var(name, value);
            } else {
                std::env::remove_var(name);
            }
        }
    }
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
