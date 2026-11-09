// Offline, disposable native acceptance wrapper. Never a runtime entry point.
// Loaded only by debug fixture unit tests; original GPLv3 applies, 2026-11-06.
use super::*;
use std::fs;
use std::sync::Condvar;
use std::time::Duration;

#[derive(Debug, PartialEq, Eq)]
enum EntryMode {
    Skip,
    Preflight,
    Visible,
    Denied,
}
fn entry_mode(args: &[std::ffi::OsString], opt_in: bool) -> EntryMode {
    match args {
        [arg] if arg == "--preflight" => EntryMode::Preflight,
        [arg] if arg == "--visible" && opt_in => EntryMode::Visible,
        _ if args.iter().any(|arg| arg == "--visible") => EntryMode::Denied,
        _ => EntryMode::Skip,
    }
}

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

// Called synchronously by the harness-free test target's process main. Ordinary
// unit tests have no native entry: libtest workers cannot host this event loop.
pub(crate) fn offline_acceptance_entry(args: &[std::ffi::OsString]) -> i32 {
    match entry_mode(
        args,
        std::env::var_os("PATCHLOOM_VISIBLE_FIXTURE_APPROVED").as_deref()
            == Some(std::ffi::OsStr::new("yes")),
    ) {
        EntryMode::Skip => {
            println!(
                "Offline acceptance skipped; use --preflight, or explicitly approved --visible."
            );
            0
        }
        EntryMode::Denied => {
            eprintln!("Visible acceptance requires only --visible and process-local PATCHLOOM_VISIBLE_FIXTURE_APPROVED=yes.");
            2
        }
        EntryMode::Preflight => {
            let harness = Acceptance::create();
            let report = preflight(&harness);
            println!("{}", serde_json::to_string(&report).unwrap());
            0
        }
        EntryMode::Visible => {
            if std::thread::current().name() != Some("main") {
                eprintln!("Use the harness-free offline_acceptance executable on its process main thread.");
                return 2;
            }
            run_visible_fixture();
            0
        }
    }
}

fn preflight(harness: &Acceptance) -> serde_json::Value {
    let fixture = &harness.fixture;
    let scope = fixture.temp.path().canonicalize().unwrap();
    for path in [
        &fixture.app.model.root,
        &fixture.addons,
        &fixture.cache,
        &fixture.data,
    ] {
        assert!(path.canonicalize().unwrap().starts_with(&scope));
        assert!(!fs::symlink_metadata(path).unwrap().file_type().is_symlink());
    }
    assert_eq!(fixture.app.model.root, fixture.temp.path().join("wow"));
    assert!(fixture.app.model.install.job.is_none());
    assert!(fixture.app.model.release_worker.is_none());
    serde_json::json!({"target":"offline_acceptance", "mode":"preflight", "native_window_started":false,
        "thread_name":std::thread::current().name(), "version":env!("CARGO_PKG_VERSION"),
        "no_self_update":cfg!(feature="no-self-update"), "scope":fixture.temp.path(),
        "root":fixture.app.model.root, "addons":fixture.addons, "cache":fixture.cache, "data":fixture.data})
}

fn run_visible_fixture() {
    assert_eq!(
        std::env::var("PATCHLOOM_VISIBLE_FIXTURE_APPROVED").as_deref(),
        Ok("yes"),
        "No native fixture window without explicit approval"
    );
    let harness = Acceptance::create();
    println!("{}", serde_json::to_string(&preflight(&harness)).unwrap());
    // Process-local only, in this ignored native entry. No routine headless
    // test mutates environment variables or invokes this path.
    let _data_scope = DataScope::enter(&harness.fixture.data);
    let base = preview_settings(harness.fixture.app.model.root.clone(), false, false);
    let mut settings = Settings::with_flags(Some(harness));
    settings.exit_on_close_request = base.exit_on_close_request;
    settings.default_text_size = base.default_text_size;
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

pub(super) struct Fixture {
    pub(super) temp: Arc<tempfile::TempDir>,
    pub(super) app: Preview,
    pub(super) addons: PathBuf,
    pub(super) cache: PathBuf,
    pub(super) data: PathBuf,
}
pub(super) async fn fixture() -> Fixture {
    let temp = Arc::new(tempfile::tempdir().unwrap());
    let root = temp.path().join("wow");
    let addons = root.join("_retail_/Interface/AddOns");
    fs::create_dir_all(addons.join("Keep")).unwrap();
    fs::write(addons.join("Keep/Keep.toc"), "## Interface: 120105\n").unwrap();
    fs::write(addons.join("Keep/unrelated.lua"), b"unrelated user data").unwrap();
    fs::write(
        root.join(".build.info"),
        "Active!DEC:1|Product!STRING:0|Version!STRING:0\n1|wow|12.1.5.70575\n",
    )
    .unwrap();
    let cache = temp.path().join("cache");
    fs::create_dir(&cache).unwrap();
    let data = temp.path().join("data");
    fs::create_dir(&data).unwrap();
    fs::write(data.join("config"), b"preserved settings").unwrap();
    let (mut app, command) = Preview::new(Flags {
        root,
        native_check: false,
    });
    drain(&mut app, command).await;
    app.dispatch(Message::Select(Product::Retail));
    app.dispatch(Message::RepositoryChanged("fixture/addon".into()));
    Fixture {
        temp,
        app,
        addons,
        cache,
        data,
    }
}
pub(super) async fn drain(app: &mut Preview, mut command: Command<Message>) {
    for _ in 0..5 {
        let mut futures = command.futures();
        if futures.is_empty() {
            return;
        }
        assert_eq!(futures.len(), 1);
        command = app.dispatch(futures.remove(0).await);
    }
    panic!("unexpected command loop");
}
