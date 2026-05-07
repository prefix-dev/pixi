//! Typestate: `on_queued` at construction, `on_started` from `start`, `on_finished` on drop.

/// Reporter reference + id carried around inside the lifecycle.
pub(crate) struct Active<'r, R: ?Sized, Id> {
    pub reporter: &'r R,
    pub id: Id,
}

/// Flavor plug-in for [`ReporterLifecycle`]. Each implementor wires a
/// concrete reporter trait and id type into the generic typestate.
pub(crate) trait LifecycleKind: 'static {
    /// The reporter trait object; GAT so the implicit `'static` bound
    /// on `dyn Trait` does not leak in.
    type Reporter<'r>: ?Sized + 'r;
    type Id: Copy;
    type Env: ?Sized;

    /// Fire `on_queued` and build the active handle. Returns `None`
    /// when no reporter is attached.
    fn queue<'r>(
        reporter: Option<&'r Self::Reporter<'r>>,
        env: &Self::Env,
    ) -> Option<Active<'r, Self::Reporter<'r>, Self::Id>>;

    fn on_started<'r>(active: &Active<'r, Self::Reporter<'r>, Self::Id>);
    fn on_finished<'r>(active: Active<'r, Self::Reporter<'r>, Self::Id>);
}

/// Queued state.
pub(crate) struct ReporterLifecycle<'r, K: LifecycleKind> {
    active: Option<Active<'r, K::Reporter<'r>, K::Id>>,
}

/// Started state: `on_finished` fires on drop.
pub(crate) struct StartedReporterLifecycle<'r, K: LifecycleKind> {
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
    pub fn queued(reporter: Option<&'r K::Reporter<'r>>, env: &K::Env) -> Self {
        Self {
            active: K::queue(reporter, env),
        }
    }

    /// The reporter id allocated in [`Self::queued`], if a reporter was
    /// attached.
    pub fn id(&self) -> Option<K::Id> {
        self.active.as_ref().map(|a| a.id)
    }

    pub fn start(mut self) -> StartedReporterLifecycle<'r, K> {
        let active = self.active.take();
        if let Some(a) = &active {
            K::on_started(a);
        }
        StartedReporterLifecycle { active }
    }
}
