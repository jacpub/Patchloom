//! Explicit installation lifecycle; modified 2026-11-06, original GPLv3 applies.
use super::*;
use ajour_core::github_releases::{Candidate, InstallApproval, PreparedInstall};
use std::sync::Mutex;

#[cfg(all(test, debug_assertions, feature = "fixture-tests"))]
#[path = "install_tests.rs"]
mod adversarial_tests;

type WorkResult<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

// Cloneable messages share one capability; they never clone its core object.
struct PreparedCell(Mutex<Option<PreparedInstall>>);
impl std::fmt::Debug for PreparedCell {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PreparedCell(opaque)")
    }
}
impl PreparedCell {
    fn take(&self) -> Option<PreparedInstall> {
        self.0.lock().ok()?.take()
    }
    fn take_matching(&self, ready: &Ready) -> Option<PreparedInstall> {
        let mut value = self.0.lock().ok()?;
        let plan = value.as_ref()?.preview();
        if plan.added() != ready.added
            || plan.replaced() != ready.replaced
            || plan.removed() != ready.removed
        {
            return None;
        }
        value.take()
    }
}
impl Drop for PreparedCell {
    fn drop(&mut self) {
        // Normal cancellation is an explicitly tracked disposal command. This
        // fallback covers abandoned/foreign messages and shutdown, cache only.
        let value = self.0.get_mut().unwrap_or_else(|e| e.into_inner()).take();
        if let Some(prepared) = value {
            drop(async_std::task::spawn_blocking(move || drop(prepared)));
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Prepare,
    Apply,
    Dispose,
}

#[derive(Debug, Clone)]
pub(super) struct Request {
    root: PathBuf,
    addons: PathBuf,
    repository: String,
    pub(super) product: Product,
    generation: Arc<()>,
    pub(super) inventory: Arc<Discovery>,
    pub(super) candidate: Arc<Candidate>,
    receipt: Option<Arc<ajour_core::fs::OwnershipPreview>>,
    token: Arc<()>,
    phase: Phase,
}
impl Request {
    fn same(&self, other: &Self) -> bool {
        self.root == other.root
            && self.addons == other.addons
            && self.repository == other.repository
            && self.product == other.product
            && self.phase == other.phase
            && Arc::ptr_eq(&self.generation, &other.generation)
            && Arc::ptr_eq(&self.inventory, &other.inventory)
            && Arc::ptr_eq(&self.candidate, &other.candidate)
            && Arc::ptr_eq(&self.token, &other.token)
            && match (&self.receipt, &other.receipt) {
                (None, None) => true,
                (Some(a), Some(b)) => Arc::ptr_eq(a, b),
                _ => false,
            }
    }
}

#[derive(Debug, Clone)]
pub(super) struct Ready {
    request: Request,
    payload: Arc<PreparedCell>,
    added: Vec<String>,
    replaced: Vec<String>,
    removed: Vec<String>,
}
#[derive(Debug, Clone)]
struct Applied {
    receipt_id: String,
    version: String,
}
#[derive(Debug, Clone)]
pub(super) struct Outcome {
    operation: Result<Applied, String>,
    inventory: Result<Arc<Discovery>, String>,
    receipt: Option<Result<Arc<ajour_core::fs::OwnershipPreview>, String>>,
    recovery: Result<Arc<ajour_core::fs::RecoveryInspection>, String>,
}
impl Outcome {
    fn recovery_warning(&self) -> Option<&'static str> {
        self.recovery
            .as_ref()
            .ok()
            .filter(|r| r.phase != "none")
            .map(|_| "Retained installation state needs recovery inspection before another change.")
    }
}
#[derive(Debug, Clone)]
pub(super) enum ResultMessage {
    Prepared(Result<Arc<Ready>, ReleaseFailure>),
    Applied(Arc<Outcome>),
    Disposed,
}
impl ResultMessage {
    fn matches(&self, phase: Phase) -> bool {
        matches!(
            (self, phase),
            (Self::Prepared(_), Phase::Prepare)
                | (Self::Applied(_), Phase::Apply)
                | (Self::Disposed, Phase::Dispose)
        )
    }
}
#[derive(Debug, Clone)]
pub(super) enum InstallMessage {
    Prepare(bool),
    Ack(Request, String),
    Page(Request, usize),
    RootPage(Request, usize),
    Cancel(Request),
    Apply(Request),
    Done(Request, ResultMessage),
}
impl InstallMessage {
    pub(super) fn is_completion(&self) -> bool {
        matches!(self, Self::Done(..))
    }
}

#[derive(Debug, Default)]
pub(super) struct InstallState {
    checked: Option<(ReleaseRequest, Arc<Candidate>)>,
    job: Option<Request>,
    pending: Option<Request>,
    ready: Option<Arc<Ready>>,
    discarded: Option<Arc<Ready>>,
    ack: String,
    page: usize,
    error: Option<String>,
    outcome: Option<Arc<Outcome>>,
}
impl InstallState {
    pub(super) fn writing(&self) -> bool {
        self.job.as_ref().is_some_and(|r| r.phase == Phase::Apply)
    }
    pub(super) fn occupied(&self) -> bool {
        self.job.is_some() || self.ready.is_some() || self.discarded.is_some()
    }
    pub(super) fn has_job(&self) -> bool {
        self.job.is_some()
    }
    pub(super) fn has_disposal(&self) -> bool {
        self.discarded.is_some()
    }
}

impl Model {
    pub(super) fn clear_install_preview(&mut self) {
        if let Some(ready) = self.install.ready.take() {
            self.install.discarded = Some(ready);
        }
        self.install.pending = None;
        self.install.ack.clear();
        self.install.page = 0;
        self.install.error = None;
        self.install.outcome = None;
    }
    pub(super) fn clear_checked_install(&mut self) {
        self.clear_install_preview();
        self.install.checked = None;
    }
    pub(super) fn complete_release_candidate(
        &mut self,
        request: ReleaseRequest,
        candidate: Arc<Candidate>,
    ) -> bool {
        if !self.complete_release(
            request.clone(),
            Ok(clip(&candidate.version, SOURCE_FIELD_CHARS)),
        ) {
            return false;
        }
        self.install.checked = Some((request, candidate));
        true
    }
    fn install_scope_current(&self, r: &Request) -> bool {
        self.root == r.root
            && self.selected == Some(r.product)
            && self.repository == r.repository
            && Arc::ptr_eq(&self.generation, &r.generation)
            && self
                .inventory
                .as_ref()
                .is_some_and(|i| Arc::ptr_eq(i, &r.inventory))
            && self.install.checked.as_ref().is_some_and(|(checked, c)| {
                Arc::ptr_eq(c, &r.candidate) && Arc::ptr_eq(&checked.inventory, &r.inventory)
            })
            && self
                .installation()
                .is_some_and(|i| i.directory.join("Interface/AddOns") == r.addons)
            && match &r.receipt {
                None => true,
                Some(receipt) => {
                    self.receipt_id == receipt.id
                        && self
                            .receipt
                            .as_ref()
                            .is_some_and(|p| Arc::ptr_eq(p, receipt))
                }
            }
    }
    fn install_slot_available(&self) -> bool {
        self.scan.is_none()
            && !self.scan_requested
            && self.receipt_worker.is_none()
            && self.recovery_job.is_none()
            && self.install.job.is_none()
    }
    fn can_prepare_install(&self, update: bool) -> bool {
        self.install_slot_available()
            && !self.install.occupied()
            && self.release_worker.is_none()
            && self
                .release_retry_at
                .is_none_or(|d| d.allows(Instant::now()))
            && self.install.checked.as_ref().is_some_and(|(checked, _)| {
                self.root == checked.root
                    && self.repository == checked.repository
                    && self.selected == Some(checked.product)
                    && self
                        .inventory
                        .as_ref()
                        .is_some_and(|i| Arc::ptr_eq(i, &checked.inventory))
            })
            && (!update
                || self.receipt.as_ref().is_some_and(|r| {
                    r.id == self.receipt_id
                        && r.source["repository"].as_str() == Some(self.repository.as_str())
                        && self.selected.is_some_and(|p| {
                            serde_json::to_value(p).ok().as_ref() == Some(&r.source["product"])
                        })
                }))
    }
    fn begin_prepare_install(&mut self, update: bool) -> Option<Request> {
        if !self.can_prepare_install(update) {
            return None;
        }
        let (checked, candidate) = self.install.checked.as_ref()?;
        let r = Request {
            root: self.root.clone(),
            addons: self.installation()?.directory.join("Interface/AddOns"),
            repository: self.repository.clone(),
            product: checked.product,
            generation: self.generation.clone(),
            inventory: checked.inventory.clone(),
            candidate: candidate.clone(),
            receipt: if update { self.receipt.clone() } else { None },
            token: Arc::new(()),
            phase: Phase::Prepare,
        };
        self.install.error = None;
        self.install.outcome = None;
        self.install.ack.clear();
        self.receipt_worker = Some(r.token.clone());
        self.install.job = Some(r.clone());
        self.install.pending = Some(r.clone());
        Some(r)
    }
    fn current_install_preview(&self, request: &Request) -> bool {
        self.install
            .ready
            .as_ref()
            .is_some_and(|ready| ready.request.same(request))
            && self.install_scope_current(request)
    }
    fn begin_apply_install(&mut self, request: &Request) -> Option<(Request, InstallApproval)> {
        if !self.install_slot_available()
            || self.install.discarded.is_some()
            || self.release_worker.is_some()
            || self.install.ack != "APPLY"
            || !self.current_install_preview(request)
        {
            return None;
        }
        let ready = self.install.ready.as_ref()?;
        let Some(prepared) = ready.payload.take_matching(ready) else {
            self.clear_install_preview();
            self.install.error =
                Some("This preview is no longer available. Download and preview again.".into());
            return None;
        };
        self.install.ready = None;
        self.install.ack.clear();
        let mut r = request.clone();
        r.phase = Phase::Apply;
        r.token = Arc::new(());
        self.receipt_worker = Some(r.token.clone());
        self.install.job = Some(r.clone());
        self.install.pending = Some(r.clone());
        Some((r, prepared.approve()))
    }
    fn begin_install_disposal(&mut self) -> Option<(Request, Arc<PreparedCell>)> {
        if self.scan.is_some()
            || self.receipt_worker.is_some()
            || self.recovery_job.is_some()
            || self.install.job.is_some()
        {
            return None;
        }
        let ready = self.install.discarded.take()?;
        let mut r = ready.request.clone();
        r.phase = Phase::Dispose;
        r.token = Arc::new(());
        self.receipt_worker = Some(r.token.clone());
        self.install.job = Some(r.clone());
        Some((r, ready.payload.clone()))
    }
    fn complete_install(&mut self, request: Request, result: ResultMessage) -> bool {
        if !self
            .install
            .job
            .as_ref()
            .is_some_and(|j| j.same(&request) && result.matches(j.phase))
            || !self
                .receipt_worker
                .as_ref()
                .is_some_and(|t| Arc::ptr_eq(t, &request.token))
        {
            return false;
        }
        self.install.job = None;
        self.receipt_worker = None;
        let pending = self.install.pending.take();
        let current = pending.as_ref().is_some_and(|p| p.same(&request))
            && self.install_scope_current(&request);
        match result {
            ResultMessage::Prepared(Ok(ready)) => {
                if current && ready.request.same(&request) {
                    self.install.ready = Some(ready);
                    self.install.page = 0;
                } else {
                    self.install.discarded = Some(ready);
                }
            }
            ResultMessage::Prepared(Err(error)) => {
                if let ReleaseFailure::RateLimited(deadline) = error {
                    self.release_retry_at = Some(
                        self.release_retry_at
                            .map_or(deadline, |old| old.max(deadline)),
                    );
                    if current {
                        self.install.error = Some("GitHub paused this download. Wait before downloading or checking again.".into());
                    }
                } else if let ReleaseFailure::Other(error) = error {
                    if current {
                        self.install.error = Some(clip(&error, ERROR_CHARS));
                    }
                }
            }
            ResultMessage::Applied(outcome) if current => {
                // The operation result survives all post-read failures. Old
                // inventory/candidate/receipt snapshots cannot authorize reuse.
                self.clear_receipt();
                self.clear_release();
                self.clear_recovery();
                self.generation = Arc::new(());
                self.inventory = outcome.inventory.as_ref().ok().cloned();
                self.error = outcome.inventory.as_ref().err().cloned();
                if let Ok(applied) = &outcome.operation {
                    self.receipt_id = applied.receipt_id.clone();
                    self.receipt = outcome
                        .receipt
                        .as_ref()
                        .and_then(|r| r.as_ref().ok())
                        .cloned();
                }
                self.install.outcome = Some(outcome);
            }
            ResultMessage::Applied(_) | ResultMessage::Disposed => {}
        }
        current
    }
}

pub(super) fn prepare_command(request: Request) -> Command<Message> {
    prepare_command_with(request, |r| {
        let installation = r
            .inventory
            .installations
            .iter()
            .find(|i| i.product == r.product)
            .ok_or_else(|| std::io::Error::other("Selected installation is missing"))?;
        let cache = std::env::temp_dir().canonicalize()?;
        async_std::task::block_on(r.candidate.prepare(
            installation,
            r.receipt.as_ref().map(|p| p.id.as_str()),
            &cache,
        ))
    })
}
fn prepare_command_with(
    request: Request,
    prepare: impl FnOnce(Request) -> WorkResult<PreparedInstall> + Send + 'static,
) -> Command<Message> {
    Command::perform(
        async move {
            let worker = request.clone();
            let result = async_std::task::spawn_blocking(move || {
                let prepared = prepare(worker.clone()).map_err(release_failure)?;
                let expected = serde_json::to_value(worker.candidate.as_ref()).map_err(|_| {
                    ReleaseFailure::Other("Could not verify checked release identity.".into())
                })?;
                let actual = serde_json::to_value(prepared.candidate()).map_err(|_| {
                    ReleaseFailure::Other("Could not verify prepared release identity.".into())
                })?;
                if actual != expected {
                    return Err(ReleaseFailure::Other(
                        "Prepared release identity does not match this check.".into(),
                    ));
                }
                let preview = prepared.preview();
                Ok(Arc::new(Ready {
                    request: worker,
                    added: preview.added().to_vec(),
                    replaced: preview.replaced().to_vec(),
                    removed: preview.removed().to_vec(),
                    payload: Arc::new(PreparedCell(Mutex::new(Some(prepared)))),
                }))
            })
            .await;
            (request, ResultMessage::Prepared(result))
        },
        |(r, result)| Message::Install(InstallMessage::Done(r, result)),
    )
}

pub(super) fn apply_command(request: Request, approval: InstallApproval) -> Command<Message> {
    apply_command_with(request, approval, || Ok(()), || {})
}

// Private worker boundary: fixtures can hold a running worker before apply or
// before post-inspection. Approval and all real writes still use the core API.
fn apply_command_with<G>(
    request: Request,
    approval: InstallApproval,
    before_apply: impl FnOnce() -> WorkResult<()> + Send + 'static,
    before_inspection: impl FnOnce() -> G + Send + 'static,
) -> Command<Message> {
    Command::perform(
        async move {
            let worker = request.clone();
            let outcome = async_std::task::spawn_blocking(move || {
                let operation = before_apply()
                    .and_then(|_| async_std::task::block_on(approval.apply()))
                    .and_then(|installed| {
                        if !installed.matches_release(&worker.candidate) {
                            return Err(std::io::Error::other(
                                "Installed release identity mismatch",
                            )
                            .into());
                        }
                        Ok(Applied {
                            receipt_id: installed.receipt_id,
                            version: clip(&worker.candidate.version, SOURCE_FIELD_CHARS),
                        })
                    })
                    .map_err(|e| clip(&e.to_string(), ERROR_CHARS));
                let scope = before_inspection();
                let inventory = discovery::discover(&worker.root)
                    .map(Arc::new)
                    .map_err(|e| clip(&e.to_string(), ERROR_CHARS));
                let receipt = operation.as_ref().ok().map(|s| {
                    ajour_core::fs::inspect_ownership(&worker.addons, &s.receipt_id)
                        .map(Arc::new)
                        .map_err(|e| clip(&e.to_string(), ERROR_CHARS))
                });
                let recovery = ajour_core::fs::inspect_recovery(&worker.addons)
                    .map(Arc::new)
                    .map_err(|e| clip(&e.to_string(), ERROR_CHARS));
                let outcome = Outcome {
                    operation,
                    inventory,
                    receipt,
                    recovery,
                };
                drop(scope);
                outcome
            })
            .await;
            (request, ResultMessage::Applied(Arc::new(outcome)))
        },
        |(r, result)| Message::Install(InstallMessage::Done(r, result)),
    )
}
fn dispose_command(request: Request, payload: Arc<PreparedCell>) -> Command<Message> {
    Command::perform(
        async move {
            async_std::task::spawn_blocking(move || drop(payload.take())).await;
            request
        },
        |r| Message::Install(InstallMessage::Done(r, ResultMessage::Disposed)),
    )
}

impl Preview {
    pub(super) fn install_message(
        &mut self,
        message: InstallMessage,
        prepare: impl FnOnce(Request) -> Command<Message>,
        apply: impl FnOnce(Request, InstallApproval) -> Command<Message>,
    ) -> Option<Command<Message>> {
        match message {
            InstallMessage::Prepare(update) => {
                if let Some(r) = self.model.begin_prepare_install(update) {
                    self.install_controls.root_page = 0;
                    return Some(prepare(r));
                }
            }
            InstallMessage::Apply(request) => {
                if let Some((r, approval)) = self.model.begin_apply_install(&request) {
                    return Some(apply(r, approval));
                }
            }
            InstallMessage::Ack(request, answer)
                if self.model.current_install_preview(&request) =>
            {
                self.model.install.ack = if answer.chars().take(6).count() <= 5 {
                    answer
                } else {
                    String::new()
                };
            }
            InstallMessage::Page(request, page) if self.model.current_install_preview(&request) => {
                self.model.install.page = page.min(
                    install_pages(self.model.install.ready.as_ref().unwrap()).saturating_sub(1),
                );
            }
            InstallMessage::RootPage(request, page)
                if self.model.current_install_preview(&request) =>
            {
                self.install_controls.root_page =
                    page.min(recovery_root_pages(&request.addons).saturating_sub(1));
            }
            InstallMessage::Cancel(request) if self.model.current_install_preview(&request) => {
                self.model.clear_install_preview()
            }
            InstallMessage::Cancel(request)
                if self
                    .model
                    .install
                    .job
                    .as_ref()
                    .is_some_and(|j| j.phase == Phase::Prepare && j.same(&request))
                    && self
                        .model
                        .install
                        .pending
                        .as_ref()
                        .is_some_and(|p| p.same(&request)) =>
            {
                self.model.clear_install_preview()
            }
            InstallMessage::Done(request, result) => {
                self.model.complete_install(request, result);
                self.synchronize_choices();
            }
            _ => {}
        }
        None
    }
    pub(super) fn followup_command(&mut self) -> Command<Message> {
        if let Some((r, payload)) = self.model.begin_install_disposal() {
            return dispose_command(r, payload);
        }
        self.model
            .start_scan()
            .map_or_else(Command::none, scan_command)
    }
}

fn install_pages(ready: &Ready) -> usize {
    [ready.added.len(), ready.replaced.len(), ready.removed.len()]
        .iter()
        .copied()
        .max()
        .unwrap_or(0)
        .saturating_sub(1)
        / FOLDERS_SHOWN
        + 1
}

pub(super) struct InstallControls {
    fresh: button::State,
    update: button::State,
    confirm: button::State,
    cancel: button::State,
    cancel_preparation: button::State,
    ack: text_input::State,
    previous: button::State,
    next: button::State,
    root_previous: button::State,
    root_next: button::State,
    root_page: usize,
}
impl InstallControls {
    pub(super) fn new() -> Self {
        Self {
            fresh: button::State::new(),
            update: button::State::new(),
            confirm: button::State::new(),
            cancel: button::State::new(),
            cancel_preparation: button::State::new(),
            ack: text_input::State::new(),
            previous: button::State::new(),
            next: button::State::new(),
            root_previous: button::State::new(),
            root_next: button::State::new(),
            root_page: 0,
        }
    }
    pub(super) fn view<'a>(
        &'a mut self,
        model: &'a Model,
        root_edited: bool,
    ) -> Element<'a, Message> {
        let mut content = Column::new()
            .spacing(8)
            .push(Text::new("Install or update").size(20));
        let mut fresh = Button::new(&mut self.fresh, Text::new("Download new install preview"));
        let mut update = Button::new(&mut self.update, Text::new("Download update preview"));
        if !root_edited && model.can_prepare_install(false) {
            fresh = fresh.on_press(Message::Install(InstallMessage::Prepare(false)));
        }
        if !root_edited && model.can_prepare_install(true) {
            update = update.on_press(Message::Install(InstallMessage::Prepare(true)));
        }
        content = content.push(Row::new().spacing(8).push(fresh).push(update))
            .push(Text::new("Download and preview saves the addon archive in temporary storage. It does not change your addons. Updates require a checked ownership receipt; existing unmanaged folders cannot be replaced."));
        if let Some(job) = &model.install.job {
            content = content.push(Text::new(match job.phase {
                Phase::Prepare if model.install.pending.is_none() => {
                    "Preparation cancelled. Waiting to discard the temporary download..."
                }
                Phase::Prepare => "Downloading and preparing the folder preview...",
                Phase::Apply => "Applying changes and checking the result...",
                Phase::Dispose => "Discarding the temporary preview...",
            }));
            if job.phase == Phase::Prepare && model.install.pending.is_some() && !root_edited {
                content = content.push(
                    Button::new(
                        &mut self.cancel_preparation,
                        Text::new("Cancel preparation"),
                    )
                    .on_press(Message::Install(InstallMessage::Cancel(job.clone()))),
                );
            }
        }
        if let Some(error) = &model.install.error {
            content = content.push(Text::new(clip(error, ERROR_CHARS)));
        }
        if let Some(ready) = &model.install.ready {
            let r = &ready.request;
            let page = model
                .install
                .page
                .min(install_pages(ready).saturating_sub(1));
            content = content
                .push(Text::new(format!(
                    "{} — {}",
                    clip(&r.repository, SOURCE_FIELD_CHARS),
                    clip(&r.candidate.version, SOURCE_FIELD_CHARS)
                )))
                .push(Text::new(format!(
                    "{}: exact AddOns folder",
                    product_label(r.product)
                )))
                .push(Text::new(recovery_root_chunk(&r.addons, self.root_page)));
            let mut rp = Button::new(&mut self.root_previous, Text::new("Previous path"));
            let mut rn = Button::new(&mut self.root_next, Text::new("Next path"));
            if !root_edited && self.root_page > 0 {
                rp = rp.on_press(Message::Install(InstallMessage::RootPage(
                    r.clone(),
                    self.root_page - 1,
                )));
            }
            if !root_edited && self.root_page + 1 < recovery_root_pages(&r.addons) {
                rn = rn.on_press(Message::Install(InstallMessage::RootPage(
                    r.clone(),
                    self.root_page + 1,
                )));
            }
            content = content.push(Row::new().spacing(8).push(rp).push(rn));
            for (label, names) in [
                ("Add", &ready.added),
                ("Replace", &ready.replaced),
                ("Remove", &ready.removed),
            ] {
                content = content
                    .push(Text::new(format!("{} ({})", label, names.len())))
                    .push(Text::new(recovery_folder_page(names, page)));
            }
            let mut previous = Button::new(&mut self.previous, Text::new("Previous folders"));
            let mut next = Button::new(&mut self.next, Text::new("Next folders"));
            if !root_edited && page > 0 {
                previous =
                    previous.on_press(Message::Install(InstallMessage::Page(r.clone(), page - 1)));
            }
            if !root_edited && page + 1 < install_pages(ready) {
                next = next.on_press(Message::Install(InstallMessage::Page(r.clone(), page + 1)));
            }
            content = content
                .push(Text::new(format!(
                    "Folder page {} of {}",
                    page + 1,
                    install_pages(ready)
                )))
                .push(Row::new().spacing(8).push(previous).push(next));
            if !root_edited {
                let preview = r.clone();
                content = content.push(TextInput::new(
                    &mut self.ack,
                    "Type APPLY to confirm these changes",
                    &model.install.ack,
                    move |s| Message::Install(InstallMessage::Ack(preview.clone(), s)),
                ));
            }
            let mut confirm = Button::new(&mut self.confirm, Text::new("Apply these changes"));
            let mut cancel = Button::new(&mut self.cancel, Text::new("Cancel preview"));
            if !root_edited && model.install_slot_available() && model.current_install_preview(r) {
                cancel = cancel.on_press(Message::Install(InstallMessage::Cancel(r.clone())));
                if model.install.ack == "APPLY" {
                    confirm = confirm.on_press(Message::Install(InstallMessage::Apply(r.clone())));
                }
            }
            content = content.push(Row::new().spacing(8).push(confirm).push(cancel));
        }
        if let Some(outcome) = &model.install.outcome {
            match &outcome.operation {
                Ok(applied) => {
                    content = content.push(Text::new(format!(
                        "Installed {}. Ownership receipt: {}",
                        applied.version, applied.receipt_id
                    )));
                }
                Err(error) => {
                    content=content.push(Text::new("Installation did not finish cleanly. Inspect recovery before trying again; some changes may have committed."))
                    .push(Text::new(clip(error,ERROR_CHARS)));
                }
            }
            if let Err(error) = &outcome.inventory {
                content = content.push(Text::new(format!(
                    "Could not refresh inventory: {}",
                    clip(error, ERROR_CHARS)
                )));
            }
            if let Some(Err(error)) = &outcome.receipt {
                content = content.push(Text::new(format!(
                    "Could not verify the new receipt: {}",
                    clip(error, ERROR_CHARS)
                )));
            }
            if let Err(error) = &outcome.recovery {
                content = content.push(Text::new(format!(
                    "Could not inspect recovery state: {}",
                    clip(error, ERROR_CHARS)
                )));
            } else if let Some(warning) = outcome.recovery_warning() {
                content = content.push(Text::new(warning));
            }
        }
        content.into()
    }
}

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
