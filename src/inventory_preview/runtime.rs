// Shared app implementation; original GPLv3 applies. Extracted 2026-11-06.
use ajour_core::discovery::{self, Discovery, Installation, Product};
use iced::{
    button, container, scrollable, text_input, Application, Button, Clipboard, Column, Command,
    Container, Element, Length, Row, Scrollable, Settings, Subscription, Text, TextInput,
};
use serde::Serialize;
use std::{path::PathBuf, sync::Arc, time::Instant};

use install::{InstallControls, InstallMessage, InstallState};

#[derive(Debug, Clone)]
struct ScanRequest {
    root: PathBuf,
    generation: Arc<()>,
    token: Arc<()>,
}
#[derive(Debug, Clone)]
struct ReceiptRequest {
    root: PathBuf,
    addons: PathBuf,
    product: Product,
    id: String,
    token: Arc<()>,
}
// Modified 2026-10-28; original GPLv3 applies.
#[derive(Debug, Clone)]
struct ReleaseRequest {
    root: PathBuf,
    repository: String,
    product: Product,
    inventory: Arc<Discovery>,
    token: Arc<()>,
}

// Added 2026-10-28; original GPLv3 applies.
#[derive(Debug, Clone)]
enum RecoveryAction {
    Inspect,
    Preview,
    Restore(Arc<ajour_core::fs::RecoveryApproval>),
}

#[derive(Debug, Clone)]
struct RecoveryRequest {
    root: PathBuf,
    addons: PathBuf,
    product: Product,
    generation: Arc<()>,
    token: Arc<()>,
    action: RecoveryAction,
}

impl RecoveryRequest {
    fn same_request(&self, other: &Self) -> bool {
        self.root == other.root
            && self.addons == other.addons
            && self.product == other.product
            && Arc::ptr_eq(&self.generation, &other.generation)
            && Arc::ptr_eq(&self.token, &other.token)
            && match (&self.action, &other.action) {
                (RecoveryAction::Inspect, RecoveryAction::Inspect)
                | (RecoveryAction::Preview, RecoveryAction::Preview) => true,
                (RecoveryAction::Restore(a), RecoveryAction::Restore(b)) => Arc::ptr_eq(a, b),
                _ => false,
            }
    }
}

#[derive(Debug, Clone)]
struct RecoveryPreview {
    request: RecoveryRequest,
    approval: Arc<ajour_core::fs::RecoveryApproval>,
}

#[derive(Debug, Clone)]
enum RecoveryResult {
    Inspection(Result<Arc<ajour_core::fs::RecoveryInspection>, String>),
    Preview(Result<Arc<ajour_core::fs::RecoveryApproval>, String>),
    Restore {
        outcome: Result<ajour_core::fs::RecoveryRestoreOutcome, String>,
        inspection: Result<Arc<ajour_core::fs::RecoveryInspection>, String>,
    },
}

impl RecoveryResult {
    fn matches_action(&self, action: &RecoveryAction) -> bool {
        matches!(
            (self, action),
            (Self::Inspection(_), RecoveryAction::Inspect)
                | (Self::Preview(_), RecoveryAction::Preview)
                | (Self::Restore { .. }, RecoveryAction::Restore(_))
        )
    }
}

#[derive(Debug, Clone)]
struct ReleaseSummary {
    repository: String,
    product: Product,
    version: String,
}

// Modified 2026-10-28; original GPLv3 applies.
// Modified 2026-10-28; original GPLv3 applies.
// Declaration order makes Blocked dominate finite deadlines when merged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum RetryDeadline {
    At(Instant),
    Blocked,
}

impl RetryDeadline {
    fn from_wait(now: Instant, wait: std::time::Duration) -> Self {
        if wait == std::time::Duration::MAX {
            return Self::Blocked;
        }
        now.checked_add(wait).map_or(Self::Blocked, Self::At)
    }

    fn allows(self, now: Instant) -> bool {
        matches!(self, Self::At(deadline) if now >= deadline)
    }
}

#[derive(Debug, Clone)]
enum ReleaseFailure {
    Other(String),
    RateLimited(RetryDeadline),
}

fn release_failure(error: Box<dyn std::error::Error + Send + Sync>) -> ReleaseFailure {
    if let Some(ajour_core::error::DownloadError::RateLimited { retry_after, .. }) =
        error.downcast_ref::<ajour_core::error::DownloadError>()
    {
        return ReleaseFailure::RateLimited(RetryDeadline::from_wait(Instant::now(), *retry_after));
    }
    ReleaseFailure::Other(clip(&error.to_string(), ERROR_CHARS))
}

fn serialize_inventory<S: serde::Serializer>(
    inventory: &Option<Arc<Discovery>>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    inventory.as_deref().serialize(serializer)
}
#[derive(Debug, Serialize)]
struct Model {
    #[serde(skip)]
    install: InstallState,
    root: PathBuf,
    #[serde(skip)]
    generation: Arc<()>,
    #[serde(serialize_with = "serialize_inventory")]
    inventory: Option<Arc<Discovery>>,
    error: Option<String>,
    selected: Option<Product>,
    #[serde(skip)]
    scan: Option<Arc<()>>,
    #[serde(skip)]
    receipt_id: String,
    #[serde(skip)]
    receipt: Option<Arc<ajour_core::fs::OwnershipPreview>>,
    #[serde(skip)]
    receipt_error: Option<String>,
    #[serde(skip)]
    pending_receipt: Option<ReceiptRequest>,
    #[serde(skip)]
    receipt_worker: Option<Arc<()>>,
    #[serde(skip)]
    scan_requested: bool,
    #[serde(skip)]
    repository: String,
    #[serde(skip)]
    release_result: Option<Result<ReleaseSummary, String>>,
    #[serde(skip)]
    pending_release: Option<ReleaseRequest>,
    #[serde(skip)]
    release_worker: Option<Arc<()>>,
    #[serde(skip)]
    release_retry_at: Option<RetryDeadline>,
    // Recovery borrows receipt_worker's existing physical filesystem slot.
    // Logical invalidation clears pending_recovery, never recovery_job.
    #[serde(skip)]
    recovery_job: Option<RecoveryRequest>,
    #[serde(skip)]
    pending_recovery: Option<RecoveryRequest>,
    #[serde(skip)]
    recovery_inspection: Option<Arc<ajour_core::fs::RecoveryInspection>>,
    #[serde(skip)]
    recovery_preview: Option<RecoveryPreview>,
    #[serde(skip)]
    recovery_ack: String,
    #[serde(skip)]
    recovery_page: usize,
    #[serde(skip)]
    recovery_error: Option<String>,
    #[serde(skip)]
    recovery_outcome: Option<Result<ajour_core::fs::RecoveryRestoreOutcome, String>>,
    #[serde(skip)]
    recovery_post_error: Option<String>,
}
impl Model {
    fn new(root: PathBuf) -> Self {
        Self {
            install: InstallState::default(),
            root,
            generation: Arc::new(()),
            inventory: None,
            error: None,
            selected: None,
            scan: None,
            receipt_id: String::new(),
            receipt: None,
            receipt_error: None,
            pending_receipt: None,
            receipt_worker: None,
            scan_requested: false,
            repository: String::new(),
            release_result: None,
            pending_release: None,
            release_worker: None,
            release_retry_at: None,
            recovery_job: None,
            pending_recovery: None,
            recovery_inspection: None,
            recovery_preview: None,
            recovery_ack: String::new(),
            recovery_page: 0,
            recovery_error: None,
            recovery_outcome: None,
            recovery_post_error: None,
        }
    }
    // Modified 2026-10-28; original GPLv3 applies.
    // Logical invalidation never releases a physical worker or provider wait.
    // Logical invalidation never releases a physical worker or provider wait.
    fn invalidate_snapshot(&mut self) {
        if self.restoring() {
            return;
        }
        self.generation = Arc::new(());
        self.scan_requested = false;
        self.inventory = None;
        self.selected = None;
        self.error = None;
        self.clear_receipt();
        self.clear_release();
        self.clear_recovery();
    }

    fn set_root(&mut self, root: PathBuf) {
        if self.restoring() {
            return;
        }
        self.invalidate_snapshot();
        self.root = root;
    }

    // Synchronous only for the deterministic headless check and fixture tests.
    fn begin_scan(&mut self) -> Option<ScanRequest> {
        if self.restoring() {
            return None;
        }
        self.invalidate_snapshot();
        if self.root.as_os_str().is_empty() {
            self.error = Some("Enter a copied WoW folder path, then click Open.".into());
            return None;
        }
        self.scan_requested = true;
        self.start_scan()
    }
    // Modified 2026-10-28; original GPLv3 applies.
    // ponytail: one snapshot worker; split only if measured latency requires it.
    fn start_scan(&mut self) -> Option<ScanRequest> {
        if !self.scan_requested
            || self.root.as_os_str().is_empty()
            || self.scan.is_some()
            || self.receipt_worker.is_some()
            || self.install.has_disposal()
        {
            return None;
        }
        self.scan_requested = false;
        let request = ScanRequest {
            root: self.root.clone(),
            generation: self.generation.clone(),
            token: Arc::new(()),
        };
        self.scan = Some(request.token.clone());
        Some(request)
    }

    fn complete_scan(
        &mut self,
        request: ScanRequest,
        result: Result<Arc<Discovery>, String>,
    ) -> bool {
        if !self
            .scan
            .as_ref()
            .is_some_and(|token| Arc::ptr_eq(token, &request.token))
        {
            return false;
        }
        // Only this physical completion may release this reservation.
        self.scan = None;
        if self.root != request.root
            || self.scan_requested
            || !Arc::ptr_eq(&self.generation, &request.generation)
        {
            return false;
        }
        match result {
            Ok(report) => {
                self.inventory = Some(report);
                self.error = None;
            }
            Err(error) => {
                self.inventory = None;
                self.error = Some(error);
            }
        }
        true
    }
    fn clear_receipt(&mut self) {
        self.clear_install_preview();
        self.pending_receipt = None;
        self.receipt = None;
        self.receipt_error = None;
    }
    fn set_receipt_id(&mut self, id: String) {
        if self.restoring() {
            return;
        }
        if self.receipt_id != id {
            self.clear_receipt();
            self.receipt_id = id;
        }
    }
    fn begin_receipt(&mut self) -> Option<ReceiptRequest> {
        if self.scan.is_some() || self.scan_requested || self.receipt_worker.is_some() {
            return None;
        }
        let installation = self.installation()?;
        let request = ReceiptRequest {
            root: self.root.clone(),
            addons: installation.directory.join("Interface").join("AddOns"),
            product: installation.product,
            id: self.receipt_id.clone(),
            token: Arc::new(()),
        };
        self.clear_receipt();
        self.receipt_worker = Some(request.token.clone());
        self.pending_receipt = Some(request.clone());
        Some(request)
    }
    fn complete_receipt(
        &mut self,
        request: ReceiptRequest,
        result: Result<Arc<ajour_core::fs::OwnershipPreview>, String>,
    ) -> bool {
        // Recovery uses this physical slot too. A receipt message cannot
        // complete a recovery job, even if supplied with its token.
        if self.recovery_job.is_some() || self.install.has_job() {
            return false;
        }
        if !self
            .receipt_worker
            .as_ref()
            .is_some_and(|token| Arc::ptr_eq(token, &request.token))
        {
            return false;
        }
        // Invalidating displayed data never releases physical capacity.
        self.receipt_worker = None;
        let pending = self.pending_receipt.take();
        if self.root != request.root
            || self.selected != Some(request.product)
            || self.receipt_id != request.id
            || !pending
                .as_ref()
                .is_some_and(|pending| Arc::ptr_eq(&pending.token, &request.token))
            || !self.installation().is_some_and(|installation| {
                installation.directory.join("Interface").join("AddOns") == request.addons
            })
        {
            return false;
        }
        let result = result.and_then(|preview| {
            let product =
                serde_json::to_value(request.product).map_err(|error| error.to_string())?;
            if preview.source.get("product") != Some(&product) {
                return Err("Ownership source/product mismatch".into());
            }
            Ok(preview)
        });
        match result {
            Ok(preview) => self.receipt = Some(preview),
            Err(error) => self.receipt_error = Some(error),
        }
        true
    }
    fn select(&mut self, product: Product) {
        if self.restoring() {
            return;
        }
        if self
            .inventory
            .as_ref()
            .is_some_and(|r| r.installations.iter().any(|i| i.product == product))
        {
            if self.selected != Some(product) {
                self.clear_receipt();
                self.clear_release();
                self.clear_recovery();
            }
            self.selected = Some(product);
        }
    }
    fn installation(&self) -> Option<&Installation> {
        let selected = self.selected?;
        self.inventory
            .as_ref()?
            .installations
            .iter()
            .find(|i| i.product == selected)
    }
    // Modified 2026-10-28; original GPLv3 applies.
    fn scan_status(&self) -> Option<&'static str> {
        if self.scan.is_some() {
            Some("Reading addon folders...")
        } else if self.scan_requested {
            Some("Waiting to read addon folders...")
        } else {
            None
        }
    }

    fn receipt_status(&self) -> Option<&'static str> {
        if self.recovery_job.is_some() {
            None
        } else {
            self.receipt_worker.as_ref().map(|_| "Checking receipt...")
        }
    }

    fn can_inspect_receipt(&self) -> bool {
        !self.receipt_id.is_empty()
            && self.installation().is_some()
            && self.scan.is_none()
            && !self.scan_requested
            && self.receipt_worker.is_none()
    }

    // Invalidation never releases physical provider capacity.
    fn clear_release(&mut self) {
        self.clear_checked_install();
        self.pending_release = None;
        self.release_result = None;
    }

    fn set_repository(&mut self, repository: String) {
        if self.restoring() {
            return;
        }
        if self.repository != repository {
            self.clear_release();
            self.repository = repository;
        }
    }

    fn release_status(&self) -> Option<&'static str> {
        self.release_worker
            .as_ref()
            .map(|_| "Checking GitHub release...")
    }

    fn release_inputs_ready(&self) -> bool {
        valid_repository(&self.repository)
            && self.installation().is_some()
            && self.scan.is_none()
            && !self.scan_requested
            && !self.restoring()
            && self.release_worker.is_none()
            && !self.install.occupied()
    }

    fn can_check_release(&self) -> bool {
        self.can_check_release_at(Instant::now())
    }

    fn can_check_release_at(&self, now: Instant) -> bool {
        self.release_inputs_ready()
            && self
                .release_retry_at
                .is_none_or(|deadline| deadline.allows(now))
    }

    fn begin_release(&mut self) -> Option<ReleaseRequest> {
        if !self.can_check_release() {
            return None;
        }
        let installation = self.installation()?;
        let request = ReleaseRequest {
            root: self.root.clone(),
            repository: self.repository.clone(),
            product: installation.product,
            inventory: self.inventory.as_ref()?.clone(),
            token: Arc::new(()),
        };
        self.clear_release();
        self.release_worker = Some(request.token.clone());
        self.pending_release = Some(request.clone());
        Some(request)
    }

    fn complete_release(
        &mut self,
        request: ReleaseRequest,
        result: Result<String, String>,
    ) -> bool {
        if !self
            .release_worker
            .as_ref()
            .is_some_and(|token| Arc::ptr_eq(token, &request.token))
        {
            return false;
        }
        // Only the matching physical completion may release this reservation.
        self.release_worker = None;
        let pending = self.pending_release.take();
        if self.root != request.root
            || self.repository != request.repository
            || self.selected != Some(request.product)
            || self.scan.is_some()
            || self.scan_requested
            || !pending
                .as_ref()
                .is_some_and(|pending| Arc::ptr_eq(&pending.token, &request.token))
            || !self
                .inventory
                .as_ref()
                .is_some_and(|report| Arc::ptr_eq(report, &request.inventory))
        {
            return false;
        }
        self.release_result = Some(
            result
                .map(|version| ReleaseSummary {
                    repository: request.repository,
                    product: request.product,
                    version: clip(&version, SOURCE_FIELD_CHARS),
                })
                .map_err(|error| clip(&error, ERROR_CHARS)),
        );
        true
    }

    fn complete_release_rate_limited(
        &mut self,
        request: ReleaseRequest,
        deadline: RetryDeadline,
    ) -> bool {
        if !self
            .release_worker
            .as_ref()
            .is_some_and(|token| Arc::ptr_eq(token, &request.token))
        {
            return false;
        }

        // Only the matching physical completion may establish or extend a
        // wait. Stale display sources still establish it; foreign/duplicate
        // completions cannot change it. Blocked dominates every finite wait.
        self.release_retry_at = Some(
            self.release_retry_at
                .map_or(deadline, |existing| existing.max(deadline)),
        );
        self.complete_release(
            request,
            Err("The server returned a rate limit. Release checks are paused.".into()),
        )
    }

    // Added 2026-10-28; original GPLv3 applies.
    fn restoring(&self) -> bool {
        self.install.writing()
            || self
                .recovery_job
                .as_ref()
                .is_some_and(|job| matches!(&job.action, RecoveryAction::Restore(_)))
    }

    fn clear_recovery(&mut self) {
        self.pending_recovery = None;
        self.recovery_inspection = None;
        self.recovery_preview = None;
        self.recovery_ack.clear();
        self.recovery_page = 0;
        self.recovery_error = None;
        self.recovery_outcome = None;
        self.recovery_post_error = None;
        // recovery_job and receipt_worker describe physical work, not display.
    }

    fn recovery_scope_current(&self, request: &RecoveryRequest) -> bool {
        if self.root != request.root
            || self.selected != Some(request.product)
            || !Arc::ptr_eq(&self.generation, &request.generation)
        {
            return false;
        }
        // Restore admission deliberately invalidates the old inventory.
        // Root/product/generation cannot change through admitted GUI actions
        // until the physical restore and post-inspection have completed.
        if matches!(&request.action, RecoveryAction::Restore(_)) {
            return true;
        }
        self.installation().is_some_and(|installation| {
            installation.directory.join("Interface").join("AddOns") == request.addons
        })
    }

    fn recovery_slot_available(&self) -> bool {
        self.scan.is_none()
            && !self.scan_requested
            && self.receipt_worker.is_none()
            && self.recovery_job.is_none()
    }

    fn can_inspect_recovery(&self) -> bool {
        self.installation().is_some() && self.recovery_slot_available()
    }

    fn can_preview_recovery(&self) -> bool {
        self.can_inspect_recovery()
            && self
                .recovery_inspection
                .as_ref()
                .is_some_and(|inspection| inspection.phase != "none")
    }

    fn begin_recovery(&mut self, preview: bool) -> Option<RecoveryRequest> {
        if if preview {
            !self.can_preview_recovery()
        } else {
            !self.can_inspect_recovery()
        } {
            return None;
        }
        let installation = self.installation()?;
        let request = RecoveryRequest {
            root: self.root.clone(),
            addons: installation.directory.join("Interface").join("AddOns"),
            product: installation.product,
            generation: self.generation.clone(),
            token: Arc::new(()),
            action: if preview {
                RecoveryAction::Preview
            } else {
                RecoveryAction::Inspect
            },
        };
        if !preview {
            self.clear_recovery();
        } else {
            self.recovery_preview = None;
            self.recovery_ack.clear();
            self.recovery_page = 0;
            self.recovery_error = None;
            self.recovery_outcome = None;
            self.recovery_post_error = None;
        }

        // Reserve synchronously, before constructing the Command. Existing
        // scan/receipt admission already checks this physical reservation.
        self.receipt_worker = Some(request.token.clone());
        self.recovery_job = Some(request.clone());
        self.pending_recovery = Some(request.clone());
        Some(request)
    }

    fn current_recovery_preview(&self, preview: &RecoveryPreview) -> bool {
        self.recovery_scope_current(&preview.request)
            && self.recovery_preview.as_ref().is_some_and(|current| {
                current.request.same_request(&preview.request)
                    && Arc::ptr_eq(&current.approval, &preview.approval)
                    && current.approval.root() == preview.approval.root()
            })
    }

    fn set_recovery_ack(&mut self, preview: &RecoveryPreview, answer: String) {
        if self.recovery_slot_available() && self.current_recovery_preview(preview) {
            // No trimming, case folding or reuse across preview identities.
            self.recovery_ack = answer.chars().take(7).collect();
        }
    }

    fn cancel_recovery_preview(&mut self, preview: &RecoveryPreview) {
        if self.recovery_slot_available() && self.current_recovery_preview(preview) {
            self.recovery_preview = None;
            self.recovery_ack.clear();
            self.recovery_page = 0;
            self.recovery_error = Some("Preview cancelled. No addon folders were changed.".into());
        }
    }

    fn can_restore_recovery(&self, preview: &RecoveryPreview) -> bool {
        self.recovery_slot_available()
            && self.recovery_ack == "RESTORE"
            && self.current_recovery_preview(preview)
    }

    fn begin_restore_recovery(&mut self, preview: &RecoveryPreview) -> Option<RecoveryRequest> {
        if !self.can_restore_recovery(preview) {
            return None;
        }
        let request = RecoveryRequest {
            root: preview.request.root.clone(),
            addons: preview.request.addons.clone(),
            product: preview.request.product,
            generation: preview.request.generation.clone(),
            token: Arc::new(()),
            action: RecoveryAction::Restore(preview.approval.clone()),
        };

        // Consume consent before dispatch. A duplicate click cannot reuse it.
        self.recovery_preview = None;
        self.recovery_ack.clear();
        self.recovery_page = 0;
        self.recovery_inspection = None;
        self.recovery_error = None;
        self.recovery_outcome = None;
        self.recovery_post_error = None;

        // This attempt can write. Old inventory/ownership and provider display
        // assumptions cease to apply even if core later returns an error.
        self.inventory = None;
        self.error = None;
        self.clear_receipt();
        self.clear_release();
        // Preserve selected scope, generation, physical provider reservation
        // and release_retry_at. No provider request is queued or retried.
        self.receipt_worker = Some(request.token.clone());
        self.recovery_job = Some(request.clone());
        self.pending_recovery = Some(request.clone());
        Some(request)
    }

    fn complete_recovery(&mut self, request: RecoveryRequest, result: RecoveryResult) -> bool {
        if !self
            .recovery_job
            .as_ref()
            .is_some_and(|job| job.same_request(&request) && result.matches_action(&job.action))
            || !self
                .receipt_worker
                .as_ref()
                .is_some_and(|token| Arc::ptr_eq(token, &request.token))
        {
            return false;
        }

        // Only the matching physical completion releases the borrowed slot.
        // A stale matching completion releases it without publishing.
        self.recovery_job = None;
        self.receipt_worker = None;
        let pending = self.pending_recovery.take();
        if !pending
            .as_ref()
            .is_some_and(|pending| pending.same_request(&request))
            || !self.recovery_scope_current(&request)
        {
            return false;
        }

        match result {
            RecoveryResult::Inspection(inspection) => match inspection {
                Ok(inspection) => self.recovery_inspection = Some(inspection),
                Err(error) => self.recovery_error = Some(error),
            },
            RecoveryResult::Preview(approval) => match approval {
                Ok(approval) => {
                    // Both readings came from this selected product scope.
                    // Canonical-root disagreement requires another inspection.
                    if self
                        .recovery_inspection
                        .as_ref()
                        .is_none_or(|inspection| inspection.root != approval.root())
                    {
                        self.recovery_inspection = None;
                        self.recovery_error =
                            Some("The AddOns folder changed. Inspect recovery again.".into());
                    } else {
                        self.recovery_preview = Some(RecoveryPreview { request, approval });
                    }
                }
                Err(error) => self.recovery_error = Some(error),
            },
            RecoveryResult::Restore {
                outcome,
                inspection,
            } => {
                // Keep the restore result even when post-inspection failed.
                self.recovery_outcome = Some(outcome);
                match inspection {
                    Ok(inspection) => self.recovery_inspection = Some(inspection),
                    Err(error) => self.recovery_post_error = Some(error),
                }
            }
        }
        true
    }

    fn recovery_status(&self) -> Option<&'static str> {
        self.recovery_job.as_ref().map(|job| match &job.action {
            RecoveryAction::Inspect => "Reading recovery state...",
            RecoveryAction::Preview => "Checking the restore plan...",
            RecoveryAction::Restore(_) => {
                "Restoring originals and checking the result. Keep Patchloom open and wait before changing folders."
            }
        })
    }

    fn set_recovery_page(&mut self, preview: &RecoveryPreview, page: usize) {
        if self.recovery_slot_available() && self.current_recovery_preview(preview) {
            let count = preview
                .approval
                .originals()
                .len()
                .max(preview.approval.move_aside_if_present().len());
            let last = count.saturating_sub(1) / FOLDERS_SHOWN;
            self.recovery_page = page.min(last);
        }
    }
}
fn inventory_snapshot(request: &ScanRequest) -> Result<Arc<Discovery>, String> {
    discovery::discover(&request.root)
        .map(Arc::new)
        .map_err(|error| error.to_string())
}
fn receipt_snapshot(
    request: &ReceiptRequest,
) -> Result<Arc<ajour_core::fs::OwnershipPreview>, String> {
    ajour_core::fs::inspect_ownership(&request.addons, &request.id)
        .map(Arc::new)
        .map_err(|error| error.to_string())
}
// Added 2026-10-28; original GPLv3 applies.
fn recovery_snapshot(request: &RecoveryRequest) -> RecoveryResult {
    match &request.action {
        RecoveryAction::Inspect => RecoveryResult::Inspection(
            ajour_core::fs::inspect_recovery(&request.addons)
                .map(Arc::new)
                .map_err(|error| error.to_string()),
        ),
        RecoveryAction::Preview => RecoveryResult::Preview(
            ajour_core::fs::preflight_restore_originals(&request.addons)
                .map(Arc::new)
                .map_err(|error| error.to_string()),
        ),
        RecoveryAction::Restore(approval) => {
            // Reaching this worker means fresh explicit consent was admitted
            // synchronously. Never show a dialog or wait under the core lock.
            let outcome = ajour_core::fs::restore_originals_checked(
                &request.addons,
                approval.as_ref(),
                |originals, move_aside| {
                    originals == approval.originals()
                        && move_aside == approval.move_aside_if_present()
                },
            )
            .map_err(|error| error.to_string());

            // Run after every checked call, including Err. Completion is not
            // delivered, and physical GUI capacity is not released, until
            // this inspection finishes.
            let inspection = ajour_core::fs::inspect_recovery(&request.addons)
                .map(Arc::new)
                .map_err(|error| error.to_string());
            RecoveryResult::Restore {
                outcome,
                inspection,
            }
        }
    }
}

fn recovery_command(request: RecoveryRequest) -> Command<Message> {
    Command::perform(
        async move {
            let worker = request.clone();
            let result = async_std::task::spawn_blocking(move || recovery_snapshot(&worker)).await;
            (request, result)
        },
        |(request, result)| Message::RecoveryDone(request, result),
    )
}

fn scan_command(request: ScanRequest) -> Command<Message> {
    Command::perform(
        async move {
            let worker = request.clone();
            let result = async_std::task::spawn_blocking(move || inventory_snapshot(&worker)).await;
            (request, result)
        },
        |(request, result)| Message::InventoryDone(request, result),
    )
}
fn receipt_command(request: ReceiptRequest) -> Command<Message> {
    Command::perform(
        async move {
            let worker = request.clone();
            let result = async_std::task::spawn_blocking(move || receipt_snapshot(&worker)).await;
            (request, result)
        },
        |(request, result)| Message::ReceiptDone(request, result),
    )
}
// GUI admission mirrors the existing core grammar; core validation remains
// authoritative. Raw input is neither trimmed nor converted from a URL.
fn valid_repository(repository: &str) -> bool {
    if repository.len() > 201 {
        return false;
    }
    let Some((owner, name)) = repository.split_once('/') else {
        return false;
    };
    [owner, name].iter().all(|part| {
        !part.is_empty()
            && part.len() <= 100
            && *part != "."
            && *part != ".."
            && part
                .bytes()
                .all(|ch| ch.is_ascii_alphanumeric() || b"-_.".contains(&ch))
    })
}

fn release_command(request: ReleaseRequest) -> Command<Message> {
    // ponytail: one independent metadata check, no queue; local snapshot
    // capacity stays unchanged. Revisit only for measured concurrency needs.
    Command::perform(
        async move {
            let result = match request
                .inventory
                .installations
                .iter()
                .find(|installation| installation.product == request.product)
            {
                Some(installation) => {
                    ajour_core::github_releases::check(&request.repository, installation)
                        .await
                        .map(Arc::new)
                        .map_err(release_failure)
                }
                None => Err(ReleaseFailure::Other(
                    "The selected WoW version is no longer available.".into(),
                )),
            };
            (request, result)
        },
        |(request, result)| match result {
            Ok(candidate) => Message::ReleaseCandidateDone(request, candidate),
            Err(ReleaseFailure::Other(error)) => Message::ReleaseDone(request, Err(error)),
            Err(ReleaseFailure::RateLimited(deadline)) => {
                Message::ReleaseRateLimited(request, deadline)
            }
        },
    )
}

fn product_label(product: Product) -> &'static str {
    match product {
        Product::Retail => "Retail",
        Product::ClassicProgression => "Mists of Pandaria Classic",
        Product::ClassicEra => "Classic Era",
        Product::Anniversary => "Burning Crusade Classic Anniversary",
    }
}

fn issue_text(issue: &str) -> String {
    let (product, detail) = issue
        .split_once(": ")
        .unwrap_or(("this installation", issue));
    if detail.contains("Unknown product/version family") {
        format!("Unsupported WoW version: {product}.")
    } else if detail.contains("Ambiguous active product builds") {
        format!("More than one active build was found for {product}.")
    } else if detail.contains("No active supported WoW products") {
        "No supported active WoW versions were found.".into()
    } else {
        format!("This WoW installation needs review: {issue}")
    }
}

// Modified 2026-10-28: bounded display of untrusted receipt text; original GPLv3 applies.
const SOURCE_FIELD_CHARS: usize = 80;
const FOLDER_CHARS: usize = 64;
const FOLDERS_SHOWN: usize = 20;
const ERROR_CHARS: usize = 300;

// Modified 2026-10-28; original GPLv3 applies.
// Bounds emitted characters before the existing optional "...", and examines
// at most max + 1 input characters. Stored strings remain unchanged.
fn clip(text: &str, max: usize) -> String {
    let mut output = String::new();
    let mut used = 0;

    for ch in text.chars() {
        let escape = ch.is_control()
            || matches!(
                ch,
                '\u{061c}'
                    | '\u{200e}'
                    | '\u{200f}'
                    | '\u{2028}'..='\u{202e}'
                    | '\u{2066}'..='\u{2069}'
            );
        let escaped = escape.then(|| ch.escape_unicode().to_string());
        let width = escaped.as_ref().map_or(1, String::len);
        if width > max - used {
            output.push_str("...");
            return output;
        }
        if let Some(escaped) = escaped {
            output.push_str(&escaped);
        } else {
            output.push(ch);
        }
        used += width;
    }

    output
}

// Reads two fields only. The source JSON is never formatted whole.
fn source_summary(source: &serde_json::Value) -> String {
    let field = |key: &str| {
        source
            .get(key)
            .and_then(|value| value.as_str())
            .map_or_else(
                || "not recorded".to_owned(),
                |value| clip(value, SOURCE_FIELD_CHARS),
            )
    };
    format!(
        "Recorded source: {} ({})",
        field("repository"),
        field("product")
    )
}

fn folder_summary(folders: &[String]) -> String {
    let mut text = folders
        .iter()
        .take(FOLDERS_SHOWN)
        .map(|folder| clip(folder, FOLDER_CHARS))
        .collect::<Vec<_>>()
        .join(", ");
    if folders.len() > FOLDERS_SHOWN {
        text.push_str(&format!(", and {} more", folders.len() - FOLDERS_SHOWN));
    }
    text
}

#[derive(Debug, Clone)]
enum Message {
    Install(InstallMessage),
    CloseRequested,
    RootInputChanged(String),
    OpenRoot,
    Select(Product),
    Rescan,
    InventoryDone(ScanRequest, Result<Arc<Discovery>, String>),
    ReceiptIdChanged(String),
    InspectReceipt,
    ReceiptDone(
        ReceiptRequest,
        Result<Arc<ajour_core::fs::OwnershipPreview>, String>,
    ),
    RepositoryChanged(String),
    CheckRelease,
    ReleaseDone(ReleaseRequest, Result<String, String>),
    ReleaseCandidateDone(ReleaseRequest, Arc<ajour_core::github_releases::Candidate>),
    ReleaseRateLimited(ReleaseRequest, RetryDeadline),
    InspectRecovery,
    PreviewRecovery,
    RecoveryAckChanged(RecoveryPreview, String),
    RecoveryPage(RecoveryPreview, usize),
    RecoveryRootPage(RecoveryPreview, usize),
    CancelRecoveryPreview(RecoveryPreview),
    RestoreRecovery(RecoveryPreview),
    RecoveryDone(RecoveryRequest, RecoveryResult),
    NativeCheckDone,
}
struct Flags {
    root: PathBuf,
    native_check: bool,
}

// Modified 2026-11-06; normal window close is owned by Preview on every platform.
fn preview_settings(root: PathBuf, native_check: bool, antialiasing: bool) -> Settings<Flags> {
    let mut settings = Settings::with_flags(Flags { root, native_check });
    settings.window.size = (960, 680);
    settings.default_text_size = 16;
    settings.antialiasing = antialiasing;
    settings.exit_on_close_request = false;
    settings
}
// Added 2026-10-28; original GPLv3 applies.
// Added 2026-10-28; original GPLv3 applies.
fn recovery_root_pages(root: &std::path::Path) -> usize {
    // OS-string debug formatting preserves non-UTF-8 path distinctions.
    // Page the representation, not a lossy or clipped approval root.
    let text = format!("{:?}", root.as_os_str());
    text.chars().count().saturating_sub(1) / ERROR_CHARS + 1
}

fn recovery_root_chunk(root: &std::path::Path, page: usize) -> String {
    let page = page.min(recovery_root_pages(root).saturating_sub(1));
    let text = format!("{:?}", root.as_os_str());

    // Bound input characters before escaping. escape_default emits ASCII,
    // including visible escapes for controls, bidi characters and backslashes.
    // The complete path remains accessible across pages.
    text.chars()
        .skip(page.saturating_mul(ERROR_CHARS))
        .take(ERROR_CHARS)
        .flat_map(char::escape_default)
        .collect()
}

fn recovery_folder_page(folders: &[String], page: usize) -> String {
    if folders.is_empty() {
        return "None.".into();
    }

    let start = page.saturating_mul(FOLDERS_SHOWN);
    let lines: Vec<_> = folders
        .iter()
        .skip(start)
        .take(FOLDERS_SHOWN)
        .enumerate()
        .map(|(offset, name)| {
            // Approval names have already passed core's 255-byte component
            // validation. Escape each complete name; do not use FOLDER_CHARS.
            let name: String = name.chars().flat_map(char::escape_default).collect();
            format!(
                "{}. \"{}\"",
                start.saturating_add(offset).saturating_add(1),
                name
            )
        })
        .collect();

    if lines.is_empty() {
        "No folders on this page.".into()
    } else {
        lines.join("\n")
    }
}

fn recovery_outcome_text(
    outcome: &Result<ajour_core::fs::RecoveryRestoreOutcome, String>,
) -> String {
    use ajour_core::fs::RecoveryRestoreOutcome;

    match outcome {
        Ok(RecoveryRestoreOutcome::Cancelled) => {
            "Restore cancelled. No addon folders were changed.".into()
        }
        Ok(RecoveryRestoreOutcome::Changed) => {
            "Recovery state changed. No restore was started. Inspect recovery and preview a new plan before confirming again.".into()
        }
        Ok(RecoveryRestoreOutcome::RestoredRetained) => {
            "Original folders restored. Replacement folders and recovery journals are retained. Updates remain blocked pending cleanup.".into()
        }
        Err(error) => format!(
            "Restore failed: {}. Some folders may already have moved. Review the recovery state before trying again.",
            clip(error, ERROR_CHARS)
        ),
    }
}

struct RecoveryControls {
    inspect: button::State,
    preview: button::State,
    restore: button::State,
    cancel: button::State,
    previous: button::State,
    next: button::State,
    root_previous: button::State,
    root_next: button::State,
    acknowledgement: text_input::State,
    root_page: usize,
}

impl RecoveryControls {
    fn new() -> Self {
        Self {
            inspect: button::State::new(),
            preview: button::State::new(),
            restore: button::State::new(),
            cancel: button::State::new(),
            previous: button::State::new(),
            next: button::State::new(),
            root_previous: button::State::new(),
            root_next: button::State::new(),
            acknowledgement: text_input::State::new(),
            root_page: 0,
        }
    }
}

// Added 2026-10-28; original GPLv3 applies.
impl RecoveryControls {
    fn view<'a>(&'a mut self, model: &'a Model, root_edited: bool) -> Element<'a, Message> {
        let mut inspect = Button::new(&mut self.inspect, Text::new("Inspect recovery"));
        if !root_edited && model.can_inspect_recovery() {
            inspect = inspect.on_press(Message::InspectRecovery);
        }

        let mut preview_button = Button::new(&mut self.preview, Text::new("Preview restore"));
        if !root_edited && model.can_preview_recovery() {
            preview_button = preview_button.on_press(Message::PreviewRecovery);
        }

        let mut content = Column::new()
            .spacing(10)
            .push(Text::new("Recovery").size(22))
            .push(Text::new(
                "Inspection and preview do not change files. Restoring originals moves addon folders.",
            ))
            .push(
                Row::new()
                    .spacing(12)
                    .push(inspect)
                    .push(preview_button),
            );

        if let Some(product) = model.selected {
            content = content.push(Text::new(format!(
                "Recovery WoW version: {}",
                product_label(product)
            )));
        }

        if let Some(status) = model.recovery_status() {
            content = content.push(Text::new(status));
        }

        if let Some(error) = &model.recovery_error {
            content = content
                .push(Text::new("Recovery message:"))
                .push(Text::new(clip(error, ERROR_CHARS)).size(14));
        }

        if let Some(outcome) = &model.recovery_outcome {
            content = content
                .push(Text::new("Restore result").size(18))
                .push(Text::new(recovery_outcome_text(outcome)))
                .push(Text::new(
                    "Rescan folder to refresh inventory before another recovery check.",
                ));
        }

        if let Some(error) = &model.recovery_post_error {
            content = content
                .push(Text::new(
                    "The follow-up recovery check failed. The restore result above still applies, but the current recovery state could not be read.",
                ))
                .push(Text::new(clip(error, ERROR_CHARS)).size(14));
        }

        if let Some(inspection) = &model.recovery_inspection {
            content = content
                .push(Text::new("Recovery inspection").size(18))
                .push(
                    Text::new(format!(
                        "AddOns path summary: {}",
                        clip(&format!("{:?}", inspection.root.as_os_str()), ERROR_CHARS)
                    ))
                    .size(14),
                );

            if inspection.phase == "none" {
                content = content.push(Text::new("No recovery journal was found."));
            } else {
                content = content
                    .push(Text::new(format!(
                        "Journal phase: {}",
                        clip(&inspection.phase, SOURCE_FIELD_CHARS)
                    )))
                    .push(Text::new(format!(
                        "Recorded original folders: {}; incoming folders: {}.",
                        inspection.previous.len(),
                        inspection.incoming.len()
                    )))
                    .push(Text::new(format!(
                        "Backups found: {}; staged folders: {}; affected destinations present: {}.",
                        inspection.backups.len(),
                        inspection.staged.len(),
                        inspection.destinations.len()
                    )));
            }

            if let Some(phase) = &inspection.restore_phase {
                content = content.push(Text::new(format!(
                    "Restore phase: {}",
                    clip(phase, SOURCE_FIELD_CHARS)
                )));
            }

            content = content
                .push(Text::new(format!(
                    "Retained replacement folders: {}.",
                    inspection.retained_replacements.len()
                )))
                .push(Text::new(
                    "This inspection does not authorize a restore. Preview restore checks whether a restore plan is available.",
                ));
        }

        if let Some(preview) = model
            .recovery_preview
            .as_ref()
            .filter(|preview| model.current_recovery_preview(preview))
        {
            let ready = !root_edited && model.recovery_slot_available();
            let root_pages = recovery_root_pages(preview.approval.root());
            let root_page = self.root_page.min(root_pages.saturating_sub(1));
            let count = preview
                .approval
                .originals()
                .len()
                .max(preview.approval.move_aside_if_present().len());
            let pages = count.saturating_sub(1) / FOLDERS_SHOWN + 1;
            let page = model.recovery_page.min(pages.saturating_sub(1));

            content = content
                .push(Text::new("Restore plan").size(18))
                .push(Text::new(format!(
                    "WoW version: {}",
                    product_label(preview.request.product)
                )))
                .push(Text::new(format!(
                    "AddOns path, part {} of {} (escaped):",
                    root_page + 1,
                    root_pages
                )))
                .push(Text::new(recovery_root_chunk(
                    preview.approval.root(),
                    root_page,
                )).size(14))
                .push(Text::new(
                    "The path includes its recorded quotes and escapes. Each part continues the previous one.",
                ));

            let mut root_previous =
                Button::new(&mut self.root_previous, Text::new("Previous path part"));
            if ready && root_page > 0 {
                root_previous = root_previous
                    .on_press(Message::RecoveryRootPage(preview.clone(), root_page - 1));
            }

            let mut root_next = Button::new(&mut self.root_next, Text::new("Next path part"));
            if ready && root_page + 1 < root_pages {
                root_next =
                    root_next.on_press(Message::RecoveryRootPage(preview.clone(), root_page + 1));
            }

            content = content
                .push(Row::new().spacing(12).push(root_previous).push(root_next))
                .push(Text::new(format!(
                    "Folder page {} of {}. Names use quoted escapes.",
                    page + 1,
                    pages
                )))
                .push(Text::new(format!(
                    "Original folders to restore ({}):",
                    preview.approval.originals().len()
                )))
                .push(Text::new(recovery_folder_page(preview.approval.originals(), page)).size(14))
                .push(Text::new(format!(
                    "Current folder names checked by this plan ({}):",
                    preview.approval.move_aside_if_present().len()
                )))
                .push(
                    Text::new(recovery_folder_page(
                        preview.approval.move_aside_if_present(),
                        page,
                    ))
                    .size(14),
                );

            let mut previous = Button::new(&mut self.previous, Text::new("Previous folder page"));
            if ready && page > 0 {
                previous = previous.on_press(Message::RecoveryPage(preview.clone(), page - 1));
            }

            let mut next = Button::new(&mut self.next, Text::new("Next folder page"));
            if ready && page + 1 < pages {
                next = next.on_press(Message::RecoveryPage(preview.clone(), page + 1));
            }

            content = content
                .push(Row::new().spacing(12).push(previous).push(next))
                .push(Text::new(
                    "Folders moved aside are kept. A resumed restore continues from its recorded stage. Replacement folders and recovery journals remain; updates stay blocked pending cleanup.",
                ))
                .push(Text::new(
                    "WTF, SavedVariables and unrelated addons are outside this restore.",
                ))
                .push(Text::new(
                    "Type RESTORE for this plan, then click Restore originals. Keep Patchloom open until restoration finishes.",
                ));

            if ready {
                let acknowledged_preview = preview.clone();
                content = content.push(TextInput::new(
                    &mut self.acknowledgement,
                    "RESTORE",
                    &model.recovery_ack,
                    move |answer| Message::RecoveryAckChanged(acknowledged_preview.clone(), answer),
                ));
            } else {
                // No editable acknowledgement or actionable confirmation while
                // another physical filesystem job owns the shared slot.
                content = content.push(Text::new(
                    "Wait for the current folder check to finish before confirming.",
                ));
            }

            let mut restore = Button::new(&mut self.restore, Text::new("Restore originals"));
            if !root_edited && model.can_restore_recovery(preview) {
                restore = restore.on_press(Message::RestoreRecovery(preview.clone()));
            }

            let mut cancel = Button::new(&mut self.cancel, Text::new("Cancel preview"));
            if ready {
                cancel = cancel.on_press(Message::CancelRecoveryPreview(preview.clone()));
            }

            content = content.push(Row::new().spacing(12).push(restore).push(cancel));
        }

        content.into()
    }
}

struct Preview {
    install_controls: InstallControls,
    model: Model,
    root_input: String,
    root_edited: bool,
    root_input_state: text_input::State,
    open_root: button::State,
    choices: Vec<button::State>,
    rescan: button::State,
    receipt_input: text_input::State,
    inspect: button::State,
    repository_input: text_input::State,
    check_release: button::State,
    recovery: RecoveryControls,
    scroll: scrollable::State,
    native_check: bool,
    exit: bool,
    close_requested: bool,
}
impl Preview {
    fn from_model(model: Model, native_check: bool) -> Self {
        let root_input = model.root.to_string_lossy().into_owned();
        let mut app = Self {
            install_controls: InstallControls::new(),
            model,
            root_input,
            root_edited: false,
            root_input_state: text_input::State::new(),
            open_root: button::State::new(),
            choices: vec![],
            rescan: button::State::new(),
            receipt_input: text_input::State::new(),
            inspect: button::State::new(),
            repository_input: text_input::State::new(),
            check_release: button::State::new(),
            recovery: RecoveryControls::new(),
            scroll: scrollable::State::new(),
            native_check,
            exit: false,
            close_requested: false,
        };
        app.synchronize_choices();
        app
    }
    // Modified 2026-10-28; original GPLv3 applies.
    fn dispatch(&mut self, message: Message) -> Command<Message> {
        self.dispatch_with_prepare(message, install::prepare_command)
    }
    // The private command boundary lets headless fixtures substitute only the
    // download, while the same admission/dispatch/apply code remains exercised.
    fn dispatch_with_prepare(
        &mut self,
        message: Message,
        prepare: impl FnOnce(install::Request) -> Command<Message>,
    ) -> Command<Message> {
        self.dispatch_with_install_workers(message, prepare, install::apply_command)
    }
    fn dispatch_with_install_workers(
        &mut self,
        message: Message,
        prepare: impl FnOnce(install::Request) -> Command<Message>,
        apply: impl FnOnce(
            install::Request,
            ajour_core::github_releases::InstallApproval,
        ) -> Command<Message>,
    ) -> Command<Message> {
        // Keep only physical recovery completions after close. Queued clicks
        // cannot start another writer before the shell observes should_exit.
        if self.close_requested
            && !matches!(
                &message,
                Message::CloseRequested
                    | Message::RecoveryDone(_, _)
                    | Message::Install(InstallMessage::Done(..))
            )
        {
            return Command::none();
        }
        if self.model.restoring()
            && matches!(
                &message,
                Message::RootInputChanged(_)
                    | Message::OpenRoot
                    | Message::Select(_)
                    | Message::Rescan
                    | Message::ReceiptIdChanged(_)
                    | Message::RepositoryChanged(_)
            )
        {
            return Command::none();
        }

        match message {
            Message::Install(event) => {
                if !self.root_edited || event.is_completion() {
                    if let Some(command) = self.install_message(event, prepare, apply) {
                        return command;
                    }
                }
            }
            Message::CloseRequested => {
                self.close_requested = true;
                self.exit = !self.model.restoring();
                return Command::none();
            }
            Message::RootInputChanged(input) => {
                if self.root_input != input {
                    self.root_input = input;
                    self.root_edited = true;
                    self.model.invalidate_snapshot();
                    self.recovery.root_page = 0;
                    self.synchronize_choices();
                }
                // Logical invalidation does not release physical capacity.
                return self.followup_command();
            }
            Message::OpenRoot => {
                let root = if self.root_edited {
                    PathBuf::from(&self.root_input)
                } else {
                    self.model.root.clone()
                };
                self.model.set_root(root);
                self.root_edited = false;
                self.recovery.root_page = 0;
                let request = self.model.begin_scan();
                self.synchronize_choices();
                return request.map_or_else(|| self.followup_command(), scan_command);
            }
            Message::Select(product) => {
                let selected = self.model.selected;
                self.model.select(product);
                if self.model.selected != selected {
                    self.recovery.root_page = 0;
                }
            }
            Message::Rescan => {
                if self.root_edited {
                    return Command::none();
                }
                let request = self.model.begin_scan();
                self.recovery.root_page = 0;
                self.synchronize_choices();
                return request.map_or_else(|| self.followup_command(), scan_command);
            }
            Message::InventoryDone(request, result) => {
                if self.model.complete_scan(request, result) {
                    self.synchronize_choices();
                }
            }
            Message::ReceiptIdChanged(id) => self.model.set_receipt_id(id),
            Message::InspectReceipt => {
                if let Some(request) = self.model.begin_receipt() {
                    return receipt_command(request);
                }
            }
            Message::ReceiptDone(request, result) => {
                self.model.complete_receipt(request, result);
            }
            Message::RepositoryChanged(repository) => {
                self.model.set_repository(repository);
            }
            Message::CheckRelease => {
                if let Some(request) = self.model.begin_release() {
                    return release_command(request);
                }
            }
            Message::ReleaseDone(request, result) => {
                self.model.complete_release(request, result);
            }
            Message::ReleaseCandidateDone(request, candidate) => {
                self.model.complete_release_candidate(request, candidate);
            }
            Message::ReleaseRateLimited(request, deadline) => {
                self.model.complete_release_rate_limited(request, deadline);
            }
            Message::InspectRecovery => {
                if !self.root_edited {
                    if let Some(request) = self.model.begin_recovery(false) {
                        self.recovery.root_page = 0;
                        return recovery_command(request);
                    }
                }
            }
            Message::PreviewRecovery => {
                if !self.root_edited {
                    if let Some(request) = self.model.begin_recovery(true) {
                        self.recovery.root_page = 0;
                        return recovery_command(request);
                    }
                }
            }
            Message::RecoveryAckChanged(preview, answer) => {
                if !self.root_edited {
                    // Bound inspection of input, with no prefix acceptance.
                    let answer = if answer.chars().take(8).count() <= 7 {
                        answer
                    } else {
                        String::new()
                    };
                    self.model.set_recovery_ack(&preview, answer);
                }
            }
            Message::RecoveryPage(preview, page) => {
                if !self.root_edited {
                    self.model.set_recovery_page(&preview, page);
                }
            }
            Message::RecoveryRootPage(preview, page) => {
                if !self.root_edited
                    && self.model.recovery_slot_available()
                    && self.model.current_recovery_preview(&preview)
                {
                    self.recovery.root_page =
                        page.min(recovery_root_pages(preview.approval.root()).saturating_sub(1));
                }
            }
            Message::CancelRecoveryPreview(preview) => {
                if !self.root_edited
                    && self.model.recovery_slot_available()
                    && self.model.current_recovery_preview(&preview)
                {
                    self.model.cancel_recovery_preview(&preview);
                    self.recovery.root_page = 0;
                }
            }
            Message::RestoreRecovery(preview) => {
                if !self.root_edited {
                    if let Some(request) = self.model.begin_restore_recovery(&preview) {
                        self.recovery.root_page = 0;
                        self.synchronize_choices();
                        return recovery_command(request);
                    }
                }
            }
            Message::RecoveryDone(request, result) => {
                // Matching completion releases physical capacity inside Model.
                // Invalidated results cannot publish or reset current controls.
                if self.model.complete_recovery(request, result) {
                    self.recovery.root_page = 0;
                    self.synchronize_choices();
                }
            }
            Message::NativeCheckDone => {
                if self.model.scan.is_none()
                    && !self.model.scan_requested
                    && self.model.receipt_worker.is_none()
                    && self.model.recovery_job.is_none()
                {
                    self.exit = true;
                }
            }
        }
        if self.close_requested {
            // Logical publication may be stale, but only a matching physical
            // completion can release the writer/post-inspection reservation.
            self.exit = !self.model.restoring();
            return Command::none();
        }
        self.followup_command()
    }
    fn synchronize_choices(&mut self) {
        self.choices = (0..self
            .model
            .inventory
            .as_ref()
            .map_or(0, |r| r.installations.len()))
            .map(|_| button::State::new())
            .collect();
    }
}
impl Application for Preview {
    type Executor = iced::executor::Default;
    type Message = Message;
    type Flags = Flags;
    fn new(flags: Flags) -> (Self, Command<Message>) {
        let mut model = Model::new(flags.root);
        let command = if model.root.as_os_str().is_empty() {
            Command::none()
        } else {
            model.begin_scan().map_or_else(Command::none, scan_command)
        };
        (Self::from_model(model, flags.native_check), command)
    }
    fn title(&self) -> String {
        "Patchloom | Inventory preview".into()
    }
    fn subscription(&self) -> Subscription<Message> {
        let close = iced_native::subscription::events_with(|event, _status| match event {
            iced_native::Event::Window(iced_native::window::Event::CloseRequested) => {
                Some(Message::CloseRequested)
            }
            _ => None,
        });
        if self.native_check {
            Subscription::batch(vec![
                close,
                iced_futures::time::every(std::time::Duration::from_millis(200))
                    .map(|_| Message::NativeCheckDone),
            ])
        } else {
            close
        }
    }
    fn mode(&self) -> iced::window::Mode {
        if self.native_check {
            iced::window::Mode::Hidden
        } else {
            iced::window::Mode::Windowed
        }
    }
    fn should_exit(&self) -> bool {
        self.exit
    }
    fn update(&mut self, message: Message, _: &mut Clipboard) -> Command<Message> {
        self.dispatch(message)
    }
    // Modified 2026-10-28; original GPLv3 applies.
    fn view(&mut self) -> Element<'_, Message> {
        let restoring = self.model.restoring();

        // Do not offer editable scope controls during restore or post-inspection.
        // Dispatch independently rejects these actions for the same interval.
        let root_input: Element<'_, Message> = if restoring {
            Text::new(clip(
                &format!("{:?}", self.model.root.as_os_str()),
                ERROR_CHARS,
            ))
            .width(Length::Fill)
            .into()
        } else {
            TextInput::new(
                &mut self.root_input_state,
                "Copied World of Warcraft folder",
                &self.root_input,
                Message::RootInputChanged,
            )
            .width(Length::Fill)
            .into()
        };

        let mut open = Button::new(&mut self.open_root, Text::new("Open"));
        if !restoring {
            open = open.on_press(Message::OpenRoot);
        }

        let mut rescan = Button::new(&mut self.rescan, Text::new("Rescan folder"));
        if !restoring && !self.root_edited && !self.model.root.as_os_str().is_empty() {
            rescan = rescan.on_press(Message::Rescan);
        }

        let mut content = Column::new()
            .spacing(14)
            .push(Text::new("Patchloom").size(32))
            .push(Text::new("WoW addon inventory").size(22))
            .push(Text::new(
                "Inventory and receipt checks do not change files. Restore originals requires a reviewed plan and confirmation.",
            ).size(16))
            .push(Text::new(
                "Development build. Enter a copied WoW folder path, then click Open.",
            ))
            .push(Row::new().spacing(12).push(root_input).push(open))
            .push(rescan);

        if self.close_requested && restoring {
            content = content.push(Text::new(
                "Patchloom will close when the current operation finishes.",
            ));
        }

        if !self.root_edited && !self.model.root.as_os_str().is_empty() {
            content = content.push(
                Text::new(clip(
                    &format!("{:?}", self.model.root.as_os_str()),
                    ERROR_CHARS,
                ))
                .size(14),
            );
        }
        if let Some(status) = self.model.scan_status() {
            content = content.push(Text::new(status));
        }
        if let Some(status) = self.model.receipt_status() {
            content = content.push(Text::new(status));
        }
        if let Some(status) = self.model.release_status() {
            content = content.push(Text::new(status));
        }
        if let Some(error) = &self.model.error {
            content = content
                .push(Text::new("We couldn't read this WoW folder.").size(22))
                .push(Text::new(
                    "Check that the copied World of Warcraft folder is complete and readable.",
                ))
                .push(Text::new(clip(error, ERROR_CHARS)).size(14));
        }

        if let Some(report) = &self.model.inventory {
            if report.installations.is_empty() {
                content =
                    content.push(Text::new("No supported WoW versions found in this folder."));
            }
            for issue in &report.issues {
                content = content.push(Text::new(clip(&issue_text(issue), ERROR_CHARS)).size(15));
            }
            for (installation, state) in report.installations.iter().zip(&mut self.choices) {
                let label = format!(
                    "{}  {}",
                    product_label(installation.product),
                    clip(&installation.version, SOURCE_FIELD_CHARS)
                );
                let mut choice = Button::new(state, Text::new(label));
                if !restoring && !self.root_edited {
                    choice = choice.on_press(Message::Select(installation.product));
                }
                content = content.push(choice);
            }

            if let Some(installation) = self.model.installation() {
                content = content
                    .push(
                        Text::new(format!(
                            "{} {}",
                            product_label(installation.product),
                            clip(&installation.version, SOURCE_FIELD_CHARS)
                        ))
                        .size(22),
                    )
                    .push(
                        Text::new(clip(
                            &format!("{:?}", installation.directory.as_os_str()),
                            ERROR_CHARS,
                        ))
                        .size(14),
                    )
                    .push(Text::new(format!(
                        "Addon folders: {}",
                        installation.addons.len()
                    )));
                if installation.addons.is_empty() {
                    content = content.push(Text::new("No addon folders found."));
                }
                for addon in &installation.addons {
                    let status = match addon.issue.as_deref() {
                        None => "Detected",
                        Some(issue) if issue.contains("Ambiguous") => {
                            "Needs review: more than one matching addon file."
                        }
                        Some(_) => "Needs review: addon metadata couldn't be matched.",
                    };
                    content = content.push(
                        Row::new()
                            .spacing(20)
                            .push(
                                Text::new(clip(&addon.addon, FOLDER_CHARS))
                                    .width(Length::FillPortion(1)),
                            )
                            .push(Text::new(status).width(Length::FillPortion(3))),
                    );
                }

                content = content
                    .push(Text::new("GitHub release check").size(18))
                    .push(Text::new(
                        "Enter a public GitHub owner/repository. Check release contacts GitHub.",
                    ));
                if !restoring {
                    content = content.push(TextInput::new(
                        &mut self.repository_input,
                        "owner/repository",
                        &self.model.repository,
                        Message::RepositoryChanged,
                    ));
                }
                match self.model.release_retry_at {
                    Some(RetryDeadline::Blocked) => {
                        content = content.push(Text::new(
                            "The server's wait cannot be represented safely. \
                             Release checks remain paused for this preview session. \
                             Local inventory and receipt checks remain available.",
                        ));
                    }
                    Some(RetryDeadline::At(deadline)) => {
                        let remaining = deadline.saturating_duration_since(Instant::now());
                        if !remaining.is_zero() {
                            let seconds = remaining
                                .as_secs()
                                .saturating_add(u64::from(remaining.subsec_nanos() != 0));
                            content = content.push(Text::new(format!(
                                "Release checks are paused. At this refresh, about \
                                 {seconds} seconds remained. Early clicks send no \
                                 request. After the wait, click Check release again. \
                                 Local inventory and receipt checks remain available."
                            )));
                        }
                    }
                    None => {}
                }

                let mut check = Button::new(&mut self.check_release, Text::new("Check release"));
                // Keep the existing manual-click behavior during a displayed wait.
                // begin_release remains authoritative and refuses early requests.
                if !self.root_edited && !restoring && self.model.release_inputs_ready() {
                    check = check.on_press(Message::CheckRelease);
                }
                content = content
                    .push(check)
                    .push(Text::new(format!(
                        "Selected WoW version: {}",
                        product_label(installation.product)
                    )))
                    .push(Text::new(
                        "This pilot checks the latest stable release. It requires a matching release.json manifest and asset digests.",
                    ))
                    .push(Text::new(
                        "A compatible release does not establish whether an addon is installed, owned or up to date. This check does not download addon ZIPs, install addons or adopt existing folders.",
                    ));
                if !self.model.repository.is_empty() && !valid_repository(&self.model.repository) {
                    content = content.push(Text::new(
                        "Enter owner/repository using letters, numbers, periods, hyphens or underscores. Each part must be 1 to 100 characters and cannot be . or ..",
                    ));
                }
                if let Some(status) = self.model.release_status() {
                    content = content.push(Text::new(status));
                } else {
                    match &self.model.release_result {
                        Some(Ok(summary)) => {
                            content = content
                                .push(Text::new(format!(
                                    "Repository: {}",
                                    clip(&summary.repository, SOURCE_FIELD_CHARS)
                                )))
                                .push(Text::new(format!(
                                    "{} compatible release: {}",
                                    product_label(summary.product),
                                    summary.version
                                )));
                        }
                        Some(Err(error)) => {
                            content = content
                                .push(Text::new("We couldn't check this release."))
                                .push(Text::new(clip(error, ERROR_CHARS)).size(14))
                                .push(Text::new(
                                    "Review the error before trying Check release again. Any active rate-limit wait still applies. Local inventory and receipt checks remain available.",
                                ));
                        }
                        None => {
                            content = content.push(Text::new("No release has been checked."));
                        }
                    }
                }

                content = content
                    .push(Text::new("Ownership receipt").size(18))
                    .push(Text::new(
                    "Enter a receipt ID to check its recorded folders against this WoW version.",
                ));
                if !restoring {
                    content = content.push(TextInput::new(
                        &mut self.receipt_input,
                        "Receipt ID",
                        &self.model.receipt_id,
                        Message::ReceiptIdChanged,
                    ));
                }
                let mut inspect = Button::new(&mut self.inspect, Text::new("Check receipt"));
                if !self.root_edited && !restoring && self.model.can_inspect_receipt() {
                    inspect = inspect.on_press(Message::InspectReceipt);
                }
                content = content.push(inspect).push(Text::new(
                    "Receipts record matching file contents. They do not authenticate the source or authorize changes.",
                ));
                if let Some(preview) = &self.model.receipt {
                    content = content
                        .push(Text::new("Receipt matches these folders:"))
                        .push(Text::new(folder_summary(&preview.folders)))
                        .push(Text::new(source_summary(&preview.source)));
                } else if let Some(error) = &self.model.receipt_error {
                    content = content
                        .push(Text::new(
                            "We couldn't verify this receipt. It may be missing, invalid, or out of date.",
                        ))
                        .push(Text::new(clip(error, ERROR_CHARS)).size(14));
                } else if let Some(status) = self.model.receipt_status() {
                    content = content.push(Text::new(status));
                } else {
                    content = content.push(Text::new("No receipt has been checked."));
                }
            } else if !report.installations.is_empty() {
                content =
                    content.push(Text::new("Choose a WoW version to view its addon folders."));
            }
        }

        // Keep recovery outside the inventory branch: restore admission clears
        // inventory, but progress, outcome and post-inspection must stay visible.
        // RecoveryControls pages the complete canonical approval root and both
        // complete folder slices. Host summaries above are not approval scope.
        content = content.push(self.install_controls.view(&self.model, self.root_edited));
        content = content.push(self.recovery.view(&self.model, self.root_edited));

        // Reuse existing neutral project surfaces without changing approved art.
        let palette = ajour_core::theme::Theme::dark().palette;
        Container::new(Scrollable::new(&mut self.scroll).padding(24).push(content))
            .style(Surface(palette))
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }
}
struct Surface(ajour_core::theme::ColorPalette);
impl container::StyleSheet for Surface {
    fn style(&self) -> container::Style {
        container::Style {
            background: Some(self.0.base.background.into()),
            text_color: Some(self.0.bright.surface),
            ..Default::default()
        }
    }
}
