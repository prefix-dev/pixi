//! Typestate that wraps a per-key reporter trait into the
//! `on_queued -> on_started -> on_finished` sequence.
//!
//! See [`LifecycleKind`] for the trait that pins the reporter type and
//! id type, and [`ReporterLifecycle`] for the state machine.

/// Reporter reference plus its allocated id, carried inside an active
/// lifecycle.
pub struct Active<'r, R: ?Sized, Id> {
    pub reporter: &'r R,
    pub id: Id,
}

/// Plug-in that pins a concrete reporter trait + id type into the
/// generic [`ReporterLifecycle`] typestate.
///
/// Implementors describe how to fire the three reporter callbacks
/// (`on_queued`, `on_started`, `on_finished`) for one specific
/// reporter trait.
pub trait LifecycleKind: 'static {
    /// The reporter trait object. GAT so the implicit `'static` bound
    /// on `dyn Trait` does not leak in.
    type Reporter<'r>: ?Sized + 'r;
    /// The id type the reporter allocates in `on_queued`.
    type Id: Copy;
    /// The environment / spec passed to `on_queued`.
    type Env: ?Sized;

    /// Fire `on_queued` and build the active handle. Returns `None`
    /// when no reporter is attached.
    fn queue<'r>(
        reporter: Option<&'r Self::Reporter<'r>>,
        env: &Self::Env,
    ) -> Option<Active<'r, Self::Reporter<'r>, Self::Id>>;

    /// Fire `on_started` for the active reporter.
    fn on_started<'r>(active: &Active<'r, Self::Reporter<'r>, Self::Id>);

    /// Fire `on_finished` for the active reporter, consuming the
    /// active handle.
    fn on_finished<'r>(active: Active<'r, Self::Reporter<'r>, Self::Id>);
}

/// Queued state. Created by [`ReporterLifecycle::queued`]; transitions
/// to [`StartedReporterLifecycle`] via [`ReporterLifecycle::start`].
pub struct ReporterLifecycle<'r, K: LifecycleKind> {
    active: Option<Active<'r, K::Reporter<'r>, K::Id>>,
}

/// Started state: `on_finished` fires automatically on drop.
pub struct StartedReporterLifecycle<'r, K: LifecycleKind> {
    active: Option<Active<'r, K::Reporter<'r>, K::Id>>,
}

impl<K: LifecycleKind> Drop for StartedReporterLifecycle<'_, K> {
    fn drop(&mut self) {
        if let Some(active) = self.active.take() {
            K::on_finished(active);
        }
    }
}

impl<'r, K: LifecycleKind> ReporterLifecycle<'r, K> {
    /// Allocate the lifecycle, firing `on_queued` if a reporter is
    /// attached.
    pub fn queued(reporter: Option<&'r K::Reporter<'r>>, env: &K::Env) -> Self {
        Self {
            active: K::queue(reporter, env),
        }
    }

    /// The reporter id allocated in [`Self::queued`], if a reporter
    /// was attached.
    pub fn id(&self) -> Option<K::Id> {
        self.active.as_ref().map(|a| a.id)
    }

    /// Fire `on_started` and transition to the started state.
    /// `on_finished` will fire when the returned guard is dropped.
    pub fn start(mut self) -> StartedReporterLifecycle<'r, K> {
        let active = self.active.take();
        if let Some(a) = &active {
            K::on_started(a);
        }
        StartedReporterLifecycle { active }
    }
}
