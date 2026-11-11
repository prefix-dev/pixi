//! Engine-tracked environment-variable lookup.
//!
//! [`EnvVarsKey`] holds a snapshot of process env vars (single-write at
//! engine construction), and [`EnvVar`] projects one variable out by
//! name. Depending on `EnvVar(name)` records a fine-grained edge in the
//! dependency graph, distinct from any other variable name.
//!
//! # Example
//!
//! ```
//! use std::{collections::HashMap, sync::Arc};
//! use pixi_compute_engine::ComputeEngine;
//! use pixi_compute_env_vars::{EnvVar, EnvVarsKey};
//!
//! let engine = ComputeEngine::new();
//! let mut snapshot = HashMap::new();
//! snapshot.insert("MY_VAR".to_owned(), "hello".to_owned());
//! engine.inject(EnvVarsKey, Arc::new(snapshot));
//!
//! # tokio_test::block_on(async {
//! let value = engine.compute(&EnvVar("MY_VAR".into())).await.unwrap();
//! assert_eq!(value.as_deref(), Some("hello"));
//! # });
//! ```

use std::{
    collections::HashMap,
    fmt::{self, Display, Formatter},
    sync::Arc,
};

use pixi_compute_engine::{ComputeCtx, InjectedKey, Key};

/// Injected snapshot of environment variables.
///
/// Populated at engine construction (typically from
/// [`std::env::vars`]). Single-write per engine. Tests inject a custom
/// map without mutating process state. Consumers depend on individual
/// variables through [`EnvVar`] so the dep graph records the specific
/// names a Key reads.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub struct EnvVarsKey;

impl Display for EnvVarsKey {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str("EnvVars")
    }
}

impl InjectedKey for EnvVarsKey {
    type Value = Arc<HashMap<String, String>>;
}

/// One environment variable read from the [`EnvVarsKey`] snapshot.
///
/// Value is `None` when the variable is not set.
#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub struct EnvVar(pub String);

impl Display for EnvVar {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "EnvVar({})", self.0)
    }
}

impl Key for EnvVar {
    type Value = Option<String>;

    async fn compute(&self, ctx: &mut ComputeCtx) -> Self::Value {
        let vars = ctx.compute(&EnvVarsKey).await;
        vars.get(&self.0).cloned()
    }
}
